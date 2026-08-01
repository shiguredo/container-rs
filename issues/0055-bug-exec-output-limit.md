# バグ: Linux exec の出力読み出しにサイズ上限が無い

- Created: 2026-07-31
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-exec-output-limit
- Polished: 2026-08-01

## 目的

Linux (Docker Engine API) の exec 出力読み出しにサイズ上限が無く、大量出力で OOM になり得るのを修正する。

## 現状

`src/core/client/docker_client.rs` の `exec` メソッドは `POST /exec/{id}/start` (Detach: false) のレスポンスボディを `read_http11_response` で全蓄積する。コードコメントに「テスト用途では出力は小さい前提のため全蓄積で十分」とあるが、公開 API `ContainerAsync::exec` 経由で任意のコマンドが実行可能であり、大量出力コマンド (例: `yes | head -c 1G`) で OOM になり得る。closed 0017 の「exec の出力は有界 (プロセス終了で EOF) なため全蓄積で十分」という判断は、時間的有界性をサイズ的有界性と混同していた。

macOS (XPC) 経路の `read_file_to_vec` には 64 MiB の上限が設定済みだが、Linux 経路 (exec 出力) には同等の保護が無い。バックエンド間で OOM 耐性が非対称である。

## 設計方針

`exec` のレスポンスボディ蓄積に上限を設ける。前提として、`ResponseAccumulator::new` は既に `max_body_bytes: Option<usize>` パラメータを持つ (`src/core/client/http_decode.rs` の `new`。`http_strategy` 側で 1 MiB 上限の使用実績あり)。ただし現行の `max_body_bytes` は上限超過時に**切り詰めて続行するだけ**でエラーを返さない (`drain_body`)。切り詰めのまま exec に適用すると multiplexed stream がフレーム途中で切断され、`demux_exec_stream` が不完全フレームを静かに捨てて出力が欠損するため、超過時はエラーにする。

実装は次の 2 つを組み合わせる。

1. `read_http11_response` に上限パラメータを追加し、exec の start レスポンス経路には 64 MiB を渡す。他の呼び出し経路 (`request` / `request_with_extra_headers` / `request_with_content_type` / `remove_blocking` / `wait_blocking`。`pull_image` の進捗 JSON や `copy_from` の tar を含む) は無制限 (`None`) のままにし、影響を与えない。`exec` は `request` 経由で呼ばれるため、`request` に上限パラメータを追加して exec start のみ `Some` を渡すか、exec start 専用の読み出し経路を設ける
2. `ResponseAccumulator` に上限超過をエラーにするモードを追加し、exec 経路では蓄積が上限を**超えた時点で**即座にエラーを返す（早期アボート。ストリーム完了を待つ方式では `yes` のような無限出力コマンドがハングし続けるため。境界は後述のとおり「64 MiB ちょうどは成功」なので、判定は `>` で行う）

注意: 既存の `max_body_bytes` は `http_strategy` 側 (wait 戦略) が 1 MiB 上限の「切り詰め続行」で利用しており、その切り詰め動作はテストで固定されている (`src/core/wait/http_strategy.rs` の `response_body_is_limited_to_one_mebibyte`)。そのため既存の `max_body_bytes` の挙動は変えず、exec 経路専用のエラーモードを追加する。

- 上限値は macOS 経路と整合させ 64 MiB とする。ただし macOS の `read_file_to_vec` は stdout / stderr 各ストリーム別 64 MiB (合計最大 128 MiB) なのに対し、Linux は multiplexed ボディ全体で 64 MiB のため、実効上限は Linux の方が厳しくなる（stdout + stderr の合計が 64 MiB を超えるとエラー）。この非対称は許容する
- 境界は macOS と同じ「64 MiB ちょうどは成功、64 MiB 超はエラー」とする (`read_file_to_vec` の `>` 判定に合わせる)
- エラーは `ClientError::Other` 系とし、上限値と理由を含む英語のメッセージにする（macOS 側の `output exceeds ... bytes limit` に合わせる）
- 早期アボートでソケットを切断した場合、コンテナ内の exec プロセスは停止せず継続し、exit code は取得されない（OOM 防止の必然的なトレードオフとして許容する）

## 完了条件

- [ ] Linux exec の出力読み出しにサイズ上限が設定されること
- [ ] 上限超過時にエラーが返ること（蓄積が 64 MiB を超えた時点で即座にエラー。無限出力コマンドでもハングしない）
- [ ] `ResponseAccumulator` の単体テストで、上限超過 → エラー・64 MiB ちょうど成功・64 MiB + 1 失敗の境界値を検証できること（`http_decode.rs` の既存ユニットテスト方式で小さな上限を渡して検証。統合テストで 64 MiB 超を流すのは CI 負荷が高いため行わない）
- [ ] 他の `read_http11_response` 呼び出し経路 (pull / copy / create 等) の挙動が変わらないこと
- [ ] `CHANGES.md` に `[FIX]` エントリが記載されること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
