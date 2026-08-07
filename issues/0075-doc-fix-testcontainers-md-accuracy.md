# ドキュメント: TESTCONTAINERS.md の実装乖離・無文書化を修正する

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/doc-fix-testcontainers-md-accuracy
- Polished: {YYYY-MM-DD}
- Updated: 2026-08-07

## 目的

精度を標榜する比較ドキュメント `docs/TESTCONTAINERS.md` に残る実装乖離と、本家 testcontainers-rs 0.27.3 のソース突き合わせで判明した無文書化の型差分を修正する。

## 現状

- **7 章 (sync `Container`) の `pause` / `unpause` 行が虚偽**: 本家 0.27.3 には `sync_container.rs` に `pub async fn pause` / `unpause` (内部で `block_on` する同期実装) が存在するが、shiguredo の `src/core/containers/sync_container.rs` には**存在しない** (Linux でも)。「Docker: 対応」は誤りで、「なし」か「`ContainerAsync` のみ」の注記が必要
- **14 章 (Mount) の Docker 列が実装と矛盾**: `target()`「Volume/Tmpfs は create 時に明示エラー」、`with_size_bytes` / `with_size` / `with_mode` / `tmpfs_options`「未反映」、`MountType::Volume` / `Tmpfs`「明示エラー」、`MountTmpfsOptions`「未反映」はすべて誤り。`src/core/client/docker_client.rs` の `CreateContainerBody::from_config` は Volume / Tmpfs を `HostConfig.Mounts` に、`SizeBytes` / `Mode` を `TmpfsOptions` に反映済み (単体テストあり)。bind_mount の「対応 (HostConfig.Mounts に反映)」とも自書矛盾
- **`HttpWaitStrategy::with_request_timeout` が無文書化**: `src/core/wait/http_strategy.rs` の `with_request_timeout` は本家 0.27.3 に**存在しない shiguredo 拡張**だが、10.3 の表・CHANGES.md develop・shiguredo 拡張一覧のいずれにも記載が無い
- **「本家との型不整合の注意点」節に未記載の型差分が 5 件** (本家 0.27.3 ソースで確認済み):
  - `WaitContainerError::StartupTimeout`: 本家はユニットバリアント、shiguredo は `{ id, timeout }` 構造体 (match が壊れる)
  - `Healthcheck::with_interval` / `with_timeout` / `with_start_period` / `with_start_interval` / `with_retries`: 本家は `impl Into<Option<Duration>>` / `impl Into<Option<u32>>` (None で Docker 既定に戻せる)、shiguredo は `Duration` / `u64`
  - `LogFrame::StdOut` / `StdErr` のペイロードと `bytes()`: 本家は `Bytes` / `&Bytes`、shiguredo は `Vec<u8>` / `&[u8]`
  - `MountType`: 本家は `PartialEq` 導出済み、shiguredo はなし (`mount.mount_type() == MountType::Bind` がコンパイル不可)
  - `CopyFileFromContainer`: 本家は Send 境界のない生 `async fn`、shiguredo は `Sized + Send` + `Pin<Box<dyn Future + Send>>` を要求 (実装側が非 Send な場合に移行不可)
- **6.1 節に `pause` 行が欠落**: `ContainerAsync::pause` (Linux のみ) は実在するのに `unpause` 行だけが載っている
- **サマリ件数は 5b1ba69 (2026-08-04) で「参考値」扱いに更新済み** (現在は対応 289 / 部分対応 18 / 未実装 0 / XPC 制約 4 / なし 59 / shiguredo 拡張 23 / 内部型 5 / 内訳合計 393)。0073 の厳密再集計 (shiguredo 拡張 22・内訳合計 415) は未実施のまま (0073 も open)。本 issue の件数修正も参考値方針に合わせて行う

## 設計方針

- 実装 (公開 API 面) と本家 0.27.3 のソースを突き合わせ、誤った判定セル・備考を修正する
- 型差分は「本家との型不整合の注意点」節に追記し、「意図的差分」と「実装差 (要検討)」を区別する (LogFrame / EndOfStream の `Vec<u8>` は依存最小方針による意図的差分、`CopyFileFromContainer` の Send 要求は移行可否に影響する実装差)
- 判定の揺れ (WaitFor::Http の「部分対応」と 10.3 の「対応」、get_host の 6.2「部分対応」と 7 章「対応」) も併せて統一する

## 完了条件

- 7 章の `pause` / `unpause` 行が実装と一致する (行ごと削除または「なし / ContainerAsync のみ」の注記)
- 14 章の Docker 列が実装と一致する (反映済み項目の「対応」化)
- `with_request_timeout` が 10.3 の表と CHANGES.md develop に記載される
- 「本家との型不整合の注意点」に上記 5 件が追記される
- 6.1 節に `pause` 行が追加され、サマリの件数が「参考値」注記 (5b1ba69) と矛盾しないこと

## 解決方法

- 7 章の `pause` / `unpause` 行を削除し、6.1 節に `async fn pause` 行を追加する (本家: あり / Apple: なし / Docker: 対応。備考は「`ContainerAsync` のみ。sync `Container` には無い」)
- 14 章の `target()` / `with_size_bytes` / `with_size` / `with_mode` / `tmpfs_options` / `MountType::Volume` / `MountType::Tmpfs` / `MountTmpfsOptions` / `impl Default for MountTmpfsOptions` の判定を「対応」に修正し、「明示エラー」「未反映」の記述を除去する
- 10.3 の表に `with_request_timeout` の行を追加し、CHANGES.md develop に [ADD] として追記する
- 「本家との型不整合の注意点」に 5 件を追記する (表の形式に揃える)
- サマリの件数と列挙の整合を確認し、「参考値」注記と矛盾しない形に更新する
