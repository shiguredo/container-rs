# ドキュメント: 公開 API の doc コメント付与と crate doc 拡充

- Priority: Low
- Created: 2026-07-22
- Completed:
- Model: Claude Sonnet 4
- Branch: feature/update-docs-public-api
- Polished: 2026-07-29

## 目的

docs.rs で公開 API が裸のシグネチャだけで表示されないよう、全公開アイテムに doc コメントを付与し、crate レベル doc を拡充する。

## 現状

- crate レベル doc (`src/lib.rs` の `//!`) は 7 行のみ。feature 一覧・プラットフォーム差・環境変数 (`TESTCONTAINERS_COMMAND`) の記載がない。また「Linux では一部 ImageExt・exec 出力等が未実装」の記述は 0015/0017/0044 等の実装により陳腐化している
- `ImageExt` (`src/core/image/image_ext.rs`) の 28 メソッド中 21 メソッドに doc コメントがない
- `ContainerAsync` (`src/core/containers/async_container.rs`) の 20 公開メソッド中 12 に doc コメントがない
- `Container` (`src/core/containers/sync_container.rs`) の 20 公開メソッド中 16 に doc コメントがない
- `ContainerRequest` (`src/core/containers/request.rs`) の 29 アクセサ全てに doc コメントがない
- `GenericImage` (`src/images/generic.rs`) の 4 メソッドに doc コメントがない
- `WaitFor` (`src/core/wait/mod.rs`) の 10 コンストラクタ中 8 に doc コメントがない
- `HealthWaitStrategy` / `ExitWaitStrategy` に doc コメントがない（`ExecCommand` / `CmdWaitFor` は一部 doc あり）
- `ContainerState` (`src/core/image.rs`) の 3 メソッド、`PortMapping` (`src/core/containers/request.rs`) の 2 メソッドに doc コメントがない
- `blocking` / `http_wait_plain` feature 条件付き re-export (`src/lib.rs`、`src/core.rs`、`src/core/wait/mod.rs`) に feature 要件の記載がない
- 古いモジュール doc に「bollard」「reqwest」への言及が残っている（`src/core/client/xpc_client.rs`、`src/core/env.rs`、`src/core/wait/http_strategy.rs`）。ただし `http_strategy.rs` の記述は本家との設計差異を説明する現役のドキュメントであり、一律削除ではなく言い換え（「本家」への参照に変更等）を検討する
- 0028（公開 API 面の semver 固定回避）が未実装のため、0028 で `pub(crate)` になるアイテムへの doc 付与は無駄になる。0028 の完了後に実装すること

## 設計方針

- crate レベル doc にクイックスタート例・feature 一覧・プラットフォーム差・環境変数 (`TESTCONTAINERS_COMMAND`) の節を追加する。既存の陳腐化した記述も修正する
- 全公開型・トレイト・メソッドに `///` で一行以上の説明を付ける。対象は `pub` アイテム（`pub(crate)` は対象外）。trait 実装メソッド・re-export 自体は `#![warn(missing_docs)]` の警告対象外だが、必要に応じて doc を付ける。enum variant は警告対象である
- feature 条件付き API に `# Feature` 節を追加する
- プラットフォーム差がある API（`get_bridge_ip_address` 等）に未対応プラットフォームを明記する
- 古いモジュール doc の「bollard」「reqwest」言及を整理する。設計根拠として有効な記述は「本家 testcontainers-rs」との参照に言い換え、単なる古い残留は削除する
- doc 付与完了後に `#![warn(missing_docs)]` を有効化する

## 完了条件

- [ ] crate レベル doc に feature 一覧・プラットフォーム差・環境変数の記載があること
- [ ] 全公開アイテムに doc コメントがあること
- [ ] `#![warn(missing_docs)]` が有効化され、警告ゼロでビルドできること
- [ ] 古いモジュール doc の bollard / reqwest 言及が整理されていること
- [ ] `cargo doc --all-features --no-deps` が警告なしでビルドできること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
