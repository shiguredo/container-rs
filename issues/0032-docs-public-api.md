# ドキュメント: 公開 API の doc コメント付与と crate doc 拡充

- Priority: Low
- Created: 2026-07-22
- Completed:
- Model: Claude Sonnet 4

## 目的

docs.rs で公開 API が裸のシグネチャだけで表示されないよう、全公開アイテムに doc コメントを付与し、crate レベル doc を拡充する。

## 優先度根拠

OSS の利用者体験に直結するが、機能・安全性には影響しないため Low。1.0 公開までに整える。

## 現状

- crate レベル doc (`//!`) が 5 行のみ。feature 一覧・プラットフォーム差・環境変数の記載がない
- `ImageExt` の 26 メソッド中 20 メソッドに doc コメントがない
- `ContainerAsync` の 12 公開メソッドに doc コメントがない
- `Container` の 17 公開メソッドに doc コメントがない
- `ContainerRequest` の 28 アクセサに doc コメントがない
- `GenericImage` の 4 メソッドに doc コメントがない
- `WaitFor` の 8 コンストラクタに doc コメントがない
- `ExecCommand` / `CmdWaitFor` / `HealthWaitStrategy` / `ExitWaitStrategy` に doc コメントがない
- `blocking` / `http_wait_plain` feature 条件付き re-export に feature 要件の記載がない
- 古いモジュール doc に「bollard」「reqwest」への言及が残っている

## 設計方針

- crate レベル doc にクイックスタート例・feature 一覧・プラットフォーム差・環境変数 (`TESTCONTAINERS_COMMAND`) の節を追加する
- 全公開型・トレイト・メソッドに `///` で一行以上の説明を付ける
- feature 条件付き API に `# Feature` 節を追加する
- プラットフォーム差がある API（`get_bridge_ip_address` 等）に未対応プラットフォームを明記する
- 古いモジュール doc の「bollard」「reqwest」言及を削除する
- 最終的に `#![warn(missing_docs)]` を有効化する

## 完了条件

- [ ] crate レベル doc に feature 一覧・プラットフォーム差・環境変数の記載があること
- [ ] 全公開アイテムに doc コメントがあること
- [ ] `#![warn(missing_docs)]` が有効化され、警告ゼロでビルドできること
- [ ] 古いモジュール doc の bollard / reqwest 言及が削除されていること
- [ ] `cargo doc --all-features --no-deps` が警告なしでビルドできること
