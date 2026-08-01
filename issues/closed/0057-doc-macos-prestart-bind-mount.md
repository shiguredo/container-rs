# ドキュメント: macOS で起動前にファイルを見せる場合は `Mount::bind_mount` を使うことを明記する

- Created: 2026-08-01
- Completed: 2026-08-02
- Branch: feature/update-macos-copy-to-doc-with-bind-mount
- Polished: 2026-08-02

## 目的

macOS (Apple container) で初期プロセスが起動時に読むファイルを投入したい利用者に対し、`with_copy_to` は start 後投入 (XPC `containerCopyIn` が running 必須) のため間に合うことが保証されない (レース依存)。実測により `Mount::bind_mount` で起動前のファイル可視化が成立することが確認できた (既存パス配下の子ファイル bind も成立。`containerCreate` 時点での materialize はコード構造からの推論)。この推奨手順をドキュメントに明記して利用者の誤解を防ぐ。

## 現状

- `with_copy_to` の rustdoc (`src/core/image/image_ext.rs`) は macOS の投入タイミングを「start 後・起動前契約なし・起動待ち等が別途必要になり得る」と明記するが、代替手段 (`Mount::bind_mount`) への誘導が無い
- `docs/TESTCONTAINERS.md` / `skills/shiguredo-container/SKILL.md` / `README.md` にも同様に代替手段の記述が無い

## 実測済みの事実 (調査で確認済み。実測環境・手順の詳細は 0045 の `## macOS 実測結果` を参照)

- `with_copy_to` の macOS 経路は `containerCreate` 直後・`containerBootstrap` 後 (start 前) のどちらでも `XPC error invalidState: container ... is not running` で失敗し、Apple container 1.2.0 でも状態ゲートは緩和されない
- `Mount::bind_mount` は次の形で起動前ファイル可視化が成立する (Apple container 1.2.0 で実測)
  - 新規パスへの単一ファイル bind (host file → `/data/payload.txt`)
  - 新規パスへのディレクトリ全体の bind (host dir → `/etc/mosquitto` 等。`/etc/mosquitto` は alpine に存在しないため実質新規パス。既存ディレクトリ全体の差し替えは未実測)
  - 既存 non-empty ディレクトリ配下の既存ファイルへの単一ファイル bind (host file → `/etc/motd` 等)。差し替えたファイル以外 (`/etc/passwd` 等) は無傷

## 設計方針

- 次を更新する (誘導先は次の 4 箇所に置く。それ以外の `with_copy_to` 記述への追記は不要。既存の記述は残したまま誘導を追記する)
  - `with_copy_to` の rustdoc (`src/core/image/image_ext.rs`): macOS の節に「起動時にファイルを見せたい場合は `with_mount(Mount::bind_mount(host_path, container_path))` を使うこと」を追記する。制約の完全形はここに書く
  - `docs/TESTCONTAINERS.md`: `ImageExt::with_copy_to` の行 (備考欄) に誘導と要点を追記する
  - `skills/shiguredo-container/SKILL.md`: copy の説明節 (`with_copy_to` の起動前投入は Linux のみの公開契約、の記述付近) に誘導と要点を追記する
  - `README.md`: macOS の注意書き (29 行付近) に誘導と絶対パス必須の要点のみ追記する
- 明記する内容 (rustdoc は完全形、docs / SKILL / README は誘導と要点)
  - ホスト側のファイル / ディレクトリは利用者側で用意し、`Mount::bind_mount` の host_path は絶対パスかつ実ファイル / 実ディレクトリを渡す前提 (相対パスは apiserver 側の cwd で解決されるため。symlink は未検証のため対象外)
  - `CopyDataSource::Data` (インメモリ bytes) の起動前投入は対象外。必要なら利用者側で一時ファイル (`tempfile` クレート等) に書き出して bind する。コンテナ稼働中はその一時ファイルを削除 (unlink) しないこと (virtiofs はホスト側ファイルを直接共有するため)
  - virtiofs はコンテナ稼働中にホスト側ファイルを書き換えるとコンテナ内の見え方が変わり得る (即時反映は実測されていないため断言しない)。既定は ReadWrite のため、読み取り専用にしたい場合は `with_access_mode(AccessMode::ReadOnly)` を指定する。`with_copy_to` のスナップショット投入とは意味論が異なる旨を明記する
- `CHANGES.md` には反映しない (`.md` ファイルの変更は changelog 規約で非対象。rustdoc 追記も機能変更ではない)

## 完了条件

- [ ] `with_copy_to` の rustdoc に macOS の起動前ファイル可視化の推奨手順 (`with_mount(Mount::bind_mount(...))`) が明記されていること
- [ ] `docs/TESTCONTAINERS.md` の `with_copy_to` 行 / `skills/shiguredo-container/SKILL.md` の copy 説明節に誘導と要点が、`README.md` の macOS 注意書きに誘導と絶対パス必須の要点が追記されていること
- [ ] 設計方針の「明記する内容」が `with_copy_to` の rustdoc に完全形で明記されていること
- [ ] 回帰検証のみとして、`cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること (新規テストの追加はしない)

## 解決方法

- `with_copy_to` の rustdoc (`src/core/image/image_ext.rs`) の macOS の節に、起動前にファイルを見せたい場合は `with_mount(Mount::bind_mount(host_path, container_path))` を使うこと (virtiofs として起動前に見えるようになる) と、制約の完全形 (host_path は絶対パスかつ実ファイル / 実ディレクトリ前提、`CopyDataSource::Data` の起動前投入は対象外で tempfile が必要かつコンテナ稼働中の unlink 禁止、virtiofs の共有意味論と `with_access_mode(AccessMode::ReadOnly)` 指定) を追記した
- `docs/TESTCONTAINERS.md` の `with_copy_to` 行の備考欄に誘導と要点 (`with_copy_to` の rustdoc 参照つき)、`skills/shiguredo-container/SKILL.md` の copy 説明節に誘導と要点、`README.md` の macOS 注意書きに誘導と絶対パス必須の要点のみを追記した
- 設計方針の「`CHANGES.md` には反映しない」は規約解釈の誤りだった。`.md` ファイルの変更分は非対象だが、rustdoc 追記はコード内のドキュメント追加であり `### misc` に記載する対象のため、`[UPDATE]` エントリを追加した (0056 と同じ扱い)
