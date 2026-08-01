# ドキュメント: Linux exec の出力上限超過時の挙動を明記する

- Created: 2026-08-01
- Completed: 2026-08-02
- Branch: feature/update-exec-output-limit-doc
- Polished: 2026-08-02

## 目的

Linux (Docker Engine API) 経路の `ContainerAsync::exec` が出力上限 (64 MiB) を超えたときに、クライアント側が早期にエラーを返し、exit code が取得できない、という挙動が利用者向けドキュメントに記載されていない。利用者が出力上限の存在と超過時のトレードオフを事前に知る手段が無いため、明記して誤解を防ぐ。

## 現状

- `DockerClient::exec` (`src/core/client/docker_client.rs`) の start レスポンス読み出しには 64 MiB の蓄積上限 (`EXEC_OUTPUT_BODY_LIMIT`) がある。蓄積が上限を超えた時点で `output exceeds 67108864 bytes limit` のエラー (`ClientError::Other`) を返し、読み取りを早期に打ち切る。境界は「ちょうど 64 MiB は成功、超はエラー」
- 上限は demux 前の multiplexed stream 全体 (stdout + stderr の合計、フレームヘッダ込み) に適用される
- 上限超過時はクライアント側がエラーを返すため exec の exit code は取得されない (OOM 防止のトレードオフとして 0055 の設計判断で許容されている。コンテナ内のプロセスが継続する点は実測されていない)
- macOS (XPC) 経路の `read_file_to_vec` (`src/core/client/xpc_client.rs`) も stdout / stderr 各 64 MiB の上限でエラーを返す
- `docs/TESTCONTAINERS.md` の exec 関連セクション (12.2 `ExecResult` 等)、`skills/shiguredo-container/SKILL.md` の exec リファレンス、`ContainerAsync::exec` (`src/core/containers/async_container.rs`) の rustdoc の Linux 節のいずれにも出力上限と超過時の挙動の記述が無い

## 設計方針

- 次を更新する
  - `docs/TESTCONTAINERS.md` の 12.2 `ExecResult` の冒頭パラグラフ直下の注記に明記する。同期版 `SyncExecResult` (12.4) も同じ出力上限に遭遇するため、12.2 の注記でカバーする
  - `ContainerAsync::exec` の rustdoc の Linux 節に同旨を追記する (公開 API の利用者がシグネチャのドキュメントから読み取れるように)。同期 API の `Container::exec` (`src/core/containers/sync_container.rs`) と `SyncExecResult` の rustdoc には追記しない (12.2 の注記でカバーする)
  - `skills/shiguredo-container/SKILL.md` の `ExecCommand` / `ExecResult` / `CmdWaitFor` セクション (exec リファレンス) に同旨を追記する
- 明記する内容
  - Linux の exec 出力には 64 MiB の蓄積上限があり、超過時は即座にエラーを返す。エラーメッセージを載せる場合は close-delimited (実経路) の文言 `output exceeds 67108864 bytes limit` を使う (Content-Length 宣言時の上限超過はデコーダ側で文言が異なるため)
  - 上限超過時はクライアント側がエラーを返すため exit code は取得できない
  - Linux は stdout + stderr の合計 (multiplexed stream 全体、フレームヘッダ込み) で 64 MiB、macOS は stdout / stderr 各 64 MiB の非対称があること
  - コンテナ内のプロセス継続は実測されていないため断言しない
- `CHANGES.md` には反映しない (`.md` ファイルの変更は changelog 規約で非対象。rustdoc 追記も機能変更ではない)

## 完了条件

- [ ] `docs/TESTCONTAINERS.md` の 12.2 `ExecResult` に、Linux exec の出力上限 (64 MiB)、超過時の挙動 (早期エラー・exit code 未取得)、macOS との非対称 (stdout / stderr 各 64 MiB)、同期版 `SyncExecResult` も同じ制約を共有する旨、コンテナ内のプロセス継続は断言しないこと、が明記されていること
- [ ] `ContainerAsync::exec` の rustdoc に出力上限・超過時の挙動・macOS との非対称が追記されていること
- [ ] `skills/shiguredo-container/SKILL.md` の `ExecCommand` / `ExecResult` / `CmdWaitFor` セクションに同旨が追記されていること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

- `docs/TESTCONTAINERS.md` の 12.2 `ExecResult` の冒頭パラグラフ直下に「出力上限」の注記を追加した。Linux は demux 前の multiplexed stream 全体 (stdout + stderr の合計、フレームヘッダ込み) で 64 MiB かつ蓄積超過の時点で即座にエラー、macOS は stdout / stderr 各 64 MiB かつエラーはプロセス終了後に返る、の非対称を明記し、上限超過時に exit code を取得できないこととコンテナ内のプロセス継続は実測されていないことを共通の記述とした。あわせて 12.4 `SyncExecResult` の備考に 12.2 の注記への逆参照を追加した
- `ContainerAsync::exec` の rustdoc の Linux 節に出力上限・超過時の挙動・macOS との非対称を追記した (同期版の rustdoc には追記しない方針は 12.2 の注記でカバーする)
- `skills/shiguredo-container/SKILL.md` の `ExecCommand` / `ExecResult` / `CmdWaitFor` セクションに同旨を追記した
- 設計方針の「`CHANGES.md` には反映しない」は規約解釈の誤りだった。`.md` ファイルの変更分は非対象だが、rustdoc 追記はコード内のドキュメント追加であり `### misc` に記載する対象のため、`[UPDATE]` エントリを追加した
