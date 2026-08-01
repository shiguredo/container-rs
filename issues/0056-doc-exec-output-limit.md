# ドキュメント: Linux exec の出力上限超過時の挙動を明記する

- Created: 2026-08-01
- Completed: {YYYY-MM-DD}
- Branch: feature/update-exec-output-limit-doc
- Polished: {YYYY-MM-DD}

## 目的

Linux (Docker Engine API) 経路の `ContainerAsync::exec` が出力上限 (64 MiB) を超えたときに、クライアント側が早期にエラーを返し、コンテナ内の exec プロセスは停止せず継続して exit code が取得できない、という挙動が利用者向けドキュメントに記載されていない。利用者が出力上限の存在と超過時のトレードオフを事前に知る手段が無いため、明記して誤解を防ぐ。

## 現状

- `DockerClient::exec` (`src/core/client/docker_client.rs`) の start レスポンス読み出しには 64 MiB の蓄積上限 (`EXEC_OUTPUT_BODY_LIMIT`) がある。蓄積が上限を超えた時点で `output exceeds 67108864 bytes limit` のエラー (`ClientError::Other`) を返し、読み取りを早期に打ち切る
- 上限は demux 前の multiplexed stream 全体 (stdout + stderr の合計、フレームヘッダ込み) に適用される。境界は「ちょうど 64 MiB は成功、超はエラー」
- 上限超過時、コンテナ内の exec プロセスは停止せず継続動作し、exit code は取得されない (OOM 防止のトレードオフとして許容されている)
- macOS (XPC) 経路の `read_file_to_vec` (`src/core/client/xpc_client.rs`) も stdout / stderr 各 64 MiB の上限でエラーを返す
- `docs/TESTCONTAINERS.md` の exec 関連セクション (12.2 `ExecResult` 等) には出力上限と超過時の挙動の記述が無い。`ContainerAsync::exec` (`src/core/containers/async_container.rs`) の rustdoc の Linux 節にも記述が無い

## 設計方針

- `docs/TESTCONTAINERS.md` の exec 関連セクション (12.2 `ExecResult` の備考欄または直下の注記) に次を明記する
  - Linux の exec 出力には 64 MiB の蓄積上限があり、超過時は即座にエラーを返す
  - 上限超過時はコンテナ内の exec プロセスが継続し、exit code は取得できない
  - Linux は stdout + stderr の合計 (multiplexed stream 全体) で 64 MiB、macOS は stdout / stderr 各 64 MiB の非対称があること
- `ContainerAsync::exec` の rustdoc の Linux 節にも同旨を追記する (公開 API の利用者がシグネチャのドキュメントから読み取れるように)
- ドキュメント変更のみのため `CHANGES.md` には反映しない

## 完了条件

- [ ] `docs/TESTCONTAINERS.md` に Linux exec の出力上限 (64 MiB) と超過時の挙動 (早期エラー・プロセス継続・exit code 未取得) が明記されていること
- [ ] `ContainerAsync::exec` の rustdoc に出力上限と超過時の挙動が追記されていること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
