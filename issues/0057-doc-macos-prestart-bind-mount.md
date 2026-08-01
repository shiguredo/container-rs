# ドキュメント: macOS で起動前にファイルを見せる場合は `Mount::bind_mount` を使うことを明記する

- Created: 2026-08-01
- Completed: {YYYY-MM-DD}
- Branch: feature/update-macos-copy-to-doc-with-bind-mount
- Polished: {YYYY-MM-DD}

## 目的

macOS (Apple container) で初期プロセスが起動時に読むファイルを投入したい利用者に対し、`with_copy_to` は start 後投入 (XPC `containerCopyIn` が running 必須) のため間に合わない。実測により `Mount::bind_mount` が `containerCreate` 時点で materialize され、起動前のファイル可視化に使えることが確認できた (既存パス配下の子ファイル bind も成立)。この推奨手順をドキュメントに明記して利用者の誤解を防ぐ。

## 現状

- `with_copy_to` の rustdoc (`src/core/image/image_ext.rs`) は macOS の投入タイミングを「start 後・起動前契約なし・起動待ち等が別途必要になり得る」と明記するが、代替手段 (`Mount::bind_mount`) への誘導が無い
- `docs/TESTCONTAINERS.md` / `skills/shiguredo-container/SKILL.md` / `README.md` にも同様に代替手段の記述が無い
- `Mount::bind_mount` (XPC では `virtiofs`) は `containerCreate` の段階で `ContainerCfg.mounts` に載り、`bootstrap` / `start_process` より前に materialize される (`src/core/client/container_cfg.rs` の `mount_cfg`)

## 実測済みの事実 (調査で確認済み)

- `with_copy_to` の macOS 経路は `containerCreate` 直後・`containerBootstrap` 後・`containerStartProcess` 前のいずれでも `XPC error invalidState: container ... is not running` で失敗する (Apple container 1.2.0 でも状態ゲートは緩和されない)
- `Mount::bind_mount` は次の形で起動前ファイル可視化が成立する (Apple container 1.2.0 で実測)
  - 新規パスへの単一ファイル bind (host file → `/data/payload.txt`)
  - ディレクトリ全体の bind (host dir → `/etc/mosquitto` 等)
  - 既存 non-empty ディレクトリ配下の既存ファイルへの単一ファイル bind (host file → `/etc/motd` 等)。差し替えたファイル以外 (`/etc/passwd` 等) は無傷

## 設計方針

- 次を更新する
  - `with_copy_to` の rustdoc (`src/core/image/image_ext.rs`): macOS の節に「起動時にファイルを見せたい場合は `Mount::bind_mount` を使うこと」を追記
  - `docs/TESTCONTAINERS.md`: 該当箇所に macOS の起動前ファイル可視化の推奨手順を追記
  - `skills/shiguredo-container/SKILL.md`: 同旨を追記
  - `README.md`: macOS の注意書きに同旨を追記
- 明記する内容
  - ホスト側のファイル / ディレクトリは利用者側で用意し、`Mount::bind_mount` の host_path は絶対パスかつ実ファイル / 実ディレクトリを渡す前提
  - `CopyDataSource::Data` (インメモリ bytes) の起動前投入は対象外。必要なら利用者側で一時ファイル (`tempfile` クレート等) に書き出して bind する
  - virtiofs はコンテナ稼働中にホスト側ファイルを書き換えるとコンテナ内の見え方が即時変わる。`with_copy_to` のスナップショット投入とは意味論が異なる旨を明記する

## 完了条件

- [ ] `with_copy_to` の rustdoc に macOS の起動前ファイル可視化の推奨手順 (`Mount::bind_mount`) が明記されていること
- [ ] `docs/TESTCONTAINERS.md` / `skills/shiguredo-container/SKILL.md` / `README.md` に同旨が追記されていること
- [ ] `Mount::bind_mount` の制約 (host_path は絶対パス・実ファイル / 実ディレクトリ必須、`CopyDataSource::Data` は対象外で tempfile が必要、virtiofs の即時反映の意味論) が明記されていること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
