# その他: Cargo.toml メタデータ整備と MSRV 検証ジョブ追加

- Priority: Medium
- Created: 2026-07-22
- Completed:
- Model: Claude Sonnet 4
- Branch: feature/update-cargo-metadata
- Polished: 2026-07-29

## 目的

crates.io での検索性・パブリッシュ品質を確保し、宣言した MSRV が CI で検証されるようにする。

## 現状

- `keywords` / `categories` が未設定
- `readme` / `documentation` は未設定だが、Cargo の既定値 (`README.md` 自動検出、`https://docs.rs/<crate-name>`) で動作している
- CI に MSRV (`rust-version = "1.93.0"`) の検証ジョブがない
- `Cargo.toml` には `include = ["/LICENSE", "/README.md", "/build.rs", "/src/**"]` が既に設定されており、tarball の内容はホワイトリストで制御済み。`exclude` の追加は `include` 存在下では効果がないため不要

## 設計方針

- Cargo.toml に以下を追加:
  - `keywords = ["container", "docker", "macos", "testing", "integration-test"]`
  - `categories = ["development-tools::testing"]`
  - `readme` / `documentation` は Cargo の既定値で動作するため追加不要 (明示しても害はないが冗長)
- CI に MSRV 検証ジョブを追加: `rustup toolchain install 1.93.0 && cargo +1.93.0 check --all-features`。実行 OS は Linux (`ubuntu-24.04`) とする (macOS は self-hosted のため MSRV 検証には不向き。`build.rs` の C コンパイルは macOS ターゲット時のみ)

## 完了条件

- [ ] Cargo.toml に keywords / categories が設定されていること
- [ ] CI に MSRV (1.93.0) 検証ジョブがあること
- [ ] `cargo package` が pass すること
