# テスト: macOS の LogConsumer FD 解放検証テストが並列実行で失敗するのを修正する

- Created: 2026-08-05
- Completed: 2026-08-12
- Branch: feature/fix-macos-log-consumer-fd-count-flaky
- Polished: 2026-08-12

## 目的

CI (test-apple-container) の cargo test 並列実行時に `xpc_alpine_log_consumer_stops_after_natural_exit` が失敗するのを止める。

## 現状

- `tests/container_macos.rs` の `test_container_xpc::xpc_alpine_log_consumer_stops_after_natural_exit` は、プロセス全体の FD 数 (`open_fd_count`) を基準に「配信タスクの dup FD が解放された」ことを検証する
- 検証は「基準値から 2 以上減る」ことだが、cargo test は同一バイナリ内のテストを並列実行するため、並列実行される他テストがコンテナを start / teardown すると FD 数が増減し、誤失敗 (減らずにタイムアウト) と誤成功 (他テストの teardown が FD を閉じる) の両方が起き得る。実際に FD を握り続けるのは、他テストの実行中コンテナが保持する containerLogs の 2 FD に加え、配信タスクが同期 dup した 2 FD の計 4 FD (いずれも ContainerAsync の Drop かタスク break まで保持される)
- 実際の CI 失敗実績:
  - 2026-08-04: `after_start=53, now=57` (4 増)
  - 2026-08-05: `after_start=53, now=56` (3 増)
- テスト単独実行では失敗しない (コードコメントにも単独実行を想定した記述がある)

## 設計方針

- 検証対象は「コンテナ自然終了後に LogConsumer 配信タスクが停止し dup FD が解放されること」であり、FD 数の減少はその観測手段
- プロセス全体の FD 数を並列実行テストで検証するのは本質的に不安定であるため、テストを単独バイナリに分離して他テストの FD 干渉を排除する (`tests/container_sync_drop_macos.rs` の「最終 drop 検証専用バイナリ」と同様のパターン)
- 代替案として検討した「FD 数全体ではなくタスク固有の FD のみを追跡する方法」は不採用とする (タスクの dup FD は公開 API から観測できず (`ContainerLogSource` は `pub(crate)`)、追跡には公開 API の追加が必要になるため、テストのフレーク修正としては過大なスコープになる)

## 完了条件

- [ ] `xpc_alpine_log_consumer_stops_after_natural_exit` が専用テストバイナリに分離され、`tests/container_macos.rs` から削除されていること
- [ ] 分離後の専用バイナリで `RUN_CONTAINER_TESTS=1 cargo test --all-features --test container_macos_log_consumer_fd xpc_alpine_log_consumer_stops_after_natural_exit` が連続 3 回 pass すること
- [ ] 分離後も FD 解放検証の意味 (タスク break で 2 減) が保たれること
- [ ] develop の CI (test-apple-container) が安定して通過する (2 回以上連続で成功する。マージ後に確認する補助条件)
- [ ] `CHANGES.md` の `### misc` に `[UPDATE]` エントリ (担当者行つき。例: 「macOS の LogConsumer FD 解放検証テストを専用バイナリに分離する」) が追加されていること

## 解決方法

- `xpc_alpine_log_consumer_stops_after_natural_exit` を `tests/container_macos_log_consumer_fd.rs` (新規テストバイナリ) に移動する (移動でありコピーではない。`tests/container_macos.rs` からは削除する)
- 新バイナリの構成:
  - `#![cfg(target_os = "macos")]` で macOS ゲートする (async テストのみで `blocking` feature は不要。`container_sync_drop_macos.rs` の `#![cfg(all(target_os = "macos", feature = "blocking"))]` とは異なる)
  - バイナリ内テストは 1 本のみとし、このバイナリへのテスト追加は禁止する (コンテナを start / teardown するテストは、FD 数検証を行わない場合でも FD 数を増減させて干渉するため。`container_sync_drop_macos.rs` の「同期 API を使う他のテストはこのバイナリに置かないこと」相当の規約コメントを付ける)
  - `mod helpers;` + `skip_if_ci()` ガードを引き継ぐ
  - `open_fd_count` とそのコメントをテストと共に移設し、並列実行を前提としたコメント (「他のテストがコンテナを start / teardown すると…」) は、このバイナリが FD 数検証専用であるという文脈に合わせて書き直す
  - `Cargo.toml` / `.github/workflows/ci.yml` の変更は不要 (tests/ 直下の .rs は自動ディスカバリで独立バイナリになり、CI の `cargo test --all-features` が自動実行する)
- 分離時に、テストコメントの「コンテナ終了時には wait スレッドの XPC 接続 FD も 1 つ閉じる」の記述を正確化する (XPC 接続は mach サービス接続であり、FD を消費するかは libxpc 実装依存のため、FD 消費を前提にしない言い回しにする。「タスク break なしでは 2 減に届かない (誤成功しない)」の結論はどちらでも成立する)
- `tests/container_macos.rs` を変更するため、同一ファイルを変更する 0058 / 0059 / 0068 / 0069 / 0095 とマージ順に注意する
- `CHANGES.md` の `### misc` に `[UPDATE]` エントリを追加する

## 解決方法 (実装)

- `xpc_alpine_log_consumer_stops_after_natural_exit` と `open_fd_count` を `tests/container_macos.rs` から削除し、新規バイナリ `tests/container_macos_log_consumer_fd.rs` へ移動した (移動でありコピーではない。`super::helpers::` → `helpers::` の参照修正のみ)
- 新バイナリは `#![cfg(target_os = "macos")]` でゲートし、`mod helpers;` + `skip_if_ci()` ガードを引き継いだ。バイナリ内テストは 1 本のみで、モジュール doc に「コンテナを start / teardown するテストは FD 数を増減させて干渉するため、このバイナリにテストを追加しないこと」の規約コメントを付けた。cargo test はテストバイナリごとに別プロセスで実行されるため、FD 表の干渉が構造的に消える旨も記録した
- `open_fd_count` のコメントを「並列実行の他テストが干渉する」前提から「このバイナリは FD 数検証専用で干渉を受けない」文脈に書き直した
- テストコメントの XPC 接続 FD の記述を正確化した (XPC 接続が FD を消費するかは libxpc 実装依存のため判定の根拠にしない。FD を消費しても 1 減にとどまり、タスク break なしでは 2 減に届かず誤成功しない)
- マーカー待ちの assert メッセージに受信済みログ (`{captured:?}`) を含めるようにした (MutexGuard の await 跨ぎを避けるブロック構造)
- `Cargo.toml` / `.github/workflows/ci.yml` は無変更 (自動ディスカバリ + CI の `cargo test --all-features` が自動実行する)
- `CHANGES.md` の `### misc` に `[UPDATE]` エントリ (担当者行つき) を追加した
- 検証: 専用バイナリで `RUN_CONTAINER_TESTS=1 cargo test --all-features --test container_macos_log_consumer_fd xpc_alpine_log_consumer_stops_after_natural_exit` を連続 3 回 pass (完了条件 2)。macOS 側 321 件・Linux 側 268 件全 pass
