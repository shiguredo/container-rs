# バグ: Linux exec の出力読み出しにサイズ上限が無い

- Created: 2026-07-31
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-exec-output-limit
- Polished: {YYYY-MM-DD}

## 目的

Linux (Docker Engine API) の exec 出力読み出しにサイズ上限が無く、大量出力で OOM になり得るのを修正する。

## 現状

`src/core/client/docker_client.rs` の `exec` メソッドは `POST /exec/{id}/start` (Detach: false) のレスポンスボディを `read_http11_response` で全蓄積する。コードコメントに「テスト用途では出力は小さい前提のため全蓄積で十分」とあるが、公開 API `ContainerAsync::exec` 経由で任意のコマンドが実行可能であり、大量出力コマンド (例: `yes | head -c 1G`) で OOM になり得る。

macOS (XPC) 経路の `read_file_to_vec` には 64 MiB の上限が設定済みだが、Linux 経路には同等の保護が無い。バックエンド間で OOM 耐性が非対称である。

## 設計方針

`exec` のレスポンスボディ蓄積に上限を設ける。`read_http11_response` の `ResponseAccumulator` にサイズ上限を渡せるようにするか、exec 専用の読み出し経路で上限をチェックする。上限値は macOS 経路と整合させ 64 MiB とする。超過時はエラーを返す。

## 完了条件

- [ ] Linux exec の出力読み出しにサイズ上限が設定されること
- [ ] 上限超過時にエラーが返ること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
