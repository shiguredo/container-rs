# バグ: Runtime 内 Drop でコンテナ削除の完了を timeout 内で待つ

- Priority: High
- Created: 2026-07-29
- Completed: 2026-07-31
- Model: Composer
- Branch: feature/fix-runtime-drop-await-remove
- Polished: 2026-07-30
- Reporter: @voluntas

## 目的

tokio Runtime 内で `ContainerAsync` / `Container` を Drop したとき、削除スレッドの完了を timeout 内で待ち、プロセス終了直後の孤立コンテナ発生を抑制する。
利用側の手動クリーンアップ (CLI 強制削除や専用 Guard) を不要にし、本家 testcontainers-rs と同様に「保持するだけで掃除される」体験に戻す。

## 優先度根拠

利用者フィードバック (mqtt-rs 移行)。Runtime 内 Drop が削除スレッドを join しないため、テストプロセス終了時に孤立コンテナが溜まる。利用側は CLI 強制削除と Guard で自衛せざるを得ない。`0035` で契約文書化、`0042` で明示 `rm_blocking` を追加予定だが、いずれも「Drop だけで確実削除」にはならない。ライブラリ側が解決すべき最大ギャップであり High。

## 現状

- Runtime 内 Drop (`impl Drop for ContainerAsync` の `Handle::try_current()` が `Ok` の分岐): `std::thread::spawn` で `remove_blocking` を実行するが **join しない** (fire-and-forget)。コメントは「async Drop からの join は deadlock し得る」としている
- `remove_blocking` 自体は macOS / Linux とも tokio Runtime に依存しない同期実装 (macOS: `XpcClient::remove_blocking` は同期 XPC 呼び出し、Linux: `DockerClient::remove_blocking` は `UnixStream` 同期 I/O)
- Runtime 外 Drop は呼び出しスレッドで同期実行し試行終了まで待つ
- sync `Container` の Drop は `drop(self.inner.take())` で `ContainerAsync::Drop` に委譲するため、同じ fire-and-forget 経路を通る
- `0035` (closed): 上記を「完了非保証」契約として README / rustdoc / TESTCONTAINERS / SKILL に固定。挙動変更はスコープ外だった
- `0042` (open): `rm_blocking()` 追加。Drop の fire-and-forget は維持し、timeout 付き join は「0049 で対応する」と明記

## 設計方針

1. Runtime 内 Drop で削除専用スレッドの完了を **timeout 付きで待つ**。`std::thread::JoinHandle` に timeout 版は存在しないため、`mpsc::channel` + `recv_timeout` で実現する (超過後も削除スレッドは裏で走り続ける。現行の fire-and-forget と同じ「最終的には消えるかもしれない」挙動であり、スレッドの abort / cancel は行わない)。`remove_blocking` は Runtime 非依存のため、tokio ワーカー上の Drop から待機しても Runtime 待ち合いの deadlock にはならない前提で進める。なお `recv_timeout` は呼び出しスレッド（= tokio ワーカー）を最大 timeout 値ブロックする。multi-threaded Runtime では他のワーカーが肩代わりするが、current-thread Runtime では Runtime 全体が停止する。テストライブラリの Drop 場面（テスト末尾）では実害が小さいため設計変更はしないが、rustdoc にこの影響を明記する
2. timeout 超過時は現行同様に諦めて `tracing::error` し、Drop 自体は panic しない（呼び出し側に Result は返せないため）
3. timeout 値は定数化する。既存の Linux ログタスク完了 polling（最大 1 秒、20 回 × 50ms）はログタスクの待ち時間でありコンテナ削除の待ち時間ではない。コンテナ削除は Docker Desktop 負荷時や XPC 混雑時に 1 秒を超え得るため、5 秒を初期値とし、実測で調整する
4. `0035` で固定した「Runtime 内 Drop は完了非保証」契約を撤回し、README / `docs/TESTCONTAINERS.md` / `skills/shiguredo-container/SKILL.md` / rustdoc (`async_container.rs` と `sync_container.rs` の両方) を「timeout 内で完了を待つ。超過時は best-effort」に更新する。`async_container.rs` の Linux ログタスク polling コメント（「Runtime 内では削除完了を保証しないのと同型で」）も根拠が崩れるため書き直す
5. `0042` の `rm_blocking` とは並立する。明示削除と成否 `Result` が必要な経路は `rm` / `rm_blocking`、何もしない Drop での掃除は本 issue。`0042` より先に実装すること（`0042` 側も「0049 が先に実装されると rustdoc の文言は書き直しになり得る」と認めている）
6. Linux 向け reaper / watchdog 拡張はスコープ外

## 完了条件

- Runtime 内で `ContainerAsync` を Drop した直後（timeout 内）に、1 ショットの不在確認が通る統合テストが Linux (`tests/container_linux.rs`) と macOS (`tests/container_macos.rs`) の両方で pass する。timeout 値はテスト環境の削除レイテンシの P99 を上回ること
- 利用側が CLI 強制削除や専用 Guard なしで Drop のみ掃除できることが文書上も明確である
- `0035` 由来の「完了非保証」記述が README / TESTCONTAINERS / SKILL / rustdoc (`async_container.rs` と `sync_container.rs` の両方) から除去または書き換えられている
- `CHANGES.md` に `[CHANGE]` エントリが記載されること (Drop の挙動が fire-and-forget から最大 N 秒ブロックに変わる利用者から観測可能な動作変更のため)
- `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass する

## 解決方法

`impl Drop for ContainerAsync` の Runtime 内分岐を、`std::thread::spawn` + `mpsc::channel` + `recv_timeout` で timeout 付き待機する形へ変更した。

1. `DROP_REMOVE_TIMEOUT` 定数（5 秒）を `async_container.rs` に追加し、Runtime 内 Drop で削除スレッドの完了を `mpsc::recv_timeout` で待機するように変更した。timeout 超過時は `tracing::error` で記録し、削除スレッドは裏で走り続ける（best-effort）
2. 契約文書 4 箇所（README / `docs/TESTCONTAINERS.md` / `skills/shiguredo-container/SKILL.md` / rustdoc）を「timeout 内で完了を待つ。超過時は best-effort」に更新した。`rm()` の rustdoc（macOS 版・Linux 版・sync 版の 3 箇所）も新契約に揃えた
3. `async_container.rs` の Linux ログタスク polling コメントを書き直した
4. `sync_container.rs` の impl 内コメントを実装（専用 std スレッド + mpsc::recv_timeout）に揃えた
5. Linux 統合テスト `alpine_drop_inside_runtime_removes_container` をポーリング (`wait_until_absent`) から 1 ショット不在確認 (`assert_absent_once`) に変更し、未使用の `wait_until_absent` ヘルパーを削除した
6. macOS 統合テスト `xpc_alpine_drop_inside_runtime_removes_container` を新規追加した
7. `CHANGES.md` に `[CHANGE]` エントリを追加した

変更ファイル: `src/core/containers/async_container.rs`、`src/core/containers/sync_container.rs`、`tests/container_linux.rs`、`tests/container_macos.rs`、`README.md`、`docs/TESTCONTAINERS.md`、`skills/shiguredo-container/SKILL.md`、`CHANGES.md`
