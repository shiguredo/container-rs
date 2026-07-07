# その他: Cargo.toml メタデータ整備と MSRV 検証ジョブ追加

- Priority: Medium
- Created: 2026-07-22
- Completed:
- Model: Claude Sonnet 4

## 目的

crates.io での検索性・パブリッシュ品質を確保し、宣言した MSRV が CI で検証されるようにする。

## 優先度根拠

OSS 公開後の発見可能性と利用者体験に直結する。機能的な問題ではないが、公開前に整えておくべきメタデータのため Medium。

## 現状

- `keywords` / `categories` / `exclude` / `readme` / `documentation` が未設定
- CI に MSRV (`rust-version = "1.88.0"`) の検証ジョブがない
- `cargo package` 時に `canary.py`、`Makefile`、`prek.toml`、`.github/`、`issues/`、`docs/`、`pbt/` 等がクレート tarball に含まれる

## 設計方針

- Cargo.toml に以下を追加:
  - `keywords = ["container", "docker", "macos", "testing", "integration-test"]`
  - `categories = ["development-tools::testing"]`
  - `readme = "README.md"`
  - `documentation = "https://docs.rs/shiguredo_container"`
  - `exclude = [".github/", "issues/", "docs/", "pbt/", "canary.py", "Makefile", "prek.toml", "clippy.toml", ".markdownlint.jsonc", ".qwen/"]`
- CI に MSRV 検証ジョブを追加: `rustup toolchain install 1.88.0 && cargo +1.88.0 check --all-features`
- Makefile に `doc` ターゲットを追加: `cargo doc --no-deps --all-features`

## 完了条件

- [ ] Cargo.toml に keywords / categories / readme / documentation / exclude が設定されていること
- [ ] CI に MSRV 検証ジョブがあること
- [ ] `cargo package --verify` が pass すること
- [ ] Makefile に doc ターゲットがあること
