# バグ: exec の `container_ready_conditions` にタイムアウトが無く、ログ FD が無い場合に永久待ちになり得る

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-exec-ready-conditions-timeout
- Polished: 2026-08-04

## 目的

`ContainerAsync::exec` の `ExecCommand::container_ready_conditions` 待機が `startup_timeout` の外側で実行され、ログ FD が取得できていない場合に永久待ちになる経路を塞ぐ。

## 現状

- `src/core/containers/async_container.rs` の `exec` は `block_until_ready(container_ready_conditions)` をタイムアウトなしで呼ぶ (start 側は `run_ready_sequence` が `startup_timeout` でラップしており非対称)
- `containerLogs` 失敗時 (`log_source = None`) のコンテナに対して exec の ready_conditions に `WaitFor::Log` を置くと、空リーダー + `exit_code_hint` 未観測のまま `LogWaitStrategy` のポーリングが永久に回る (コンテナ生存中は `exit_code_hint` が `None` のままのため。コンテナが終了すれば `DRAIN_GRACE` 後に `EndOfStream` エラーで終わる)。start 側は `ready_conditions_require_log_fds` + `log_fd_required_error` で事前に明示エラーにしているが、exec 経路はそのガードが無い
- rustdoc には「無期限に待ち得る」と明記されているが、テストのハング温床であり、`startup_timeout` と非対称

## 設計方針

- exec の ready_conditions 待機には start と同じ `startup_timeout` (`ContainerRequest::startup_timeout` の値。未設定 (None) の場合は start 側と同じ既定値 60 秒) を適用する。既定値の定数は start 側 (`async_runner.rs` の `DEFAULT_STARTUP_TIMEOUT`) と共有し、二重定義による値のドリフトを避ける。`ExecCommand` にタイムアウト設定 API は追加しない (本家互換の API 面を保ち、統合テストでも `with_startup_timeout` で短縮できる)。`startup_timeout` は本 issue では「exec の ready_conditions 待機にも適用される待機タイムアウト」として扱う
- タイムアウト時のエラーは `WaitContainerError::StartupTimeout` を流用する (新規バリアントの追加による公開 API の破壊を避ける。Display 文言は既存のまま)
- `log_source` が `None` (ログ取得元が無い。macOS はログ FD、Linux はログストリーム) かつ `WaitFor::Log` を含む場合は、start 側と同じ明示エラーにする (macOS / Linux 共通。`log_source` の状態で判定するため OS 非依存で実装できる)。エラー文言は start 側 (`log_fd_required_error`) と同じ趣旨とするが、exec 時点では `containerLogs` の失敗原因を保持していないため原因を含まず、macOS / Linux 両方に通じる OS 中立の文言にする
- 本家 testcontainers-rs と同じ構造 (タイムアウトなし) から、ハング温床を優先して本家と異なる挙動 (タイムアウト付き) に変更する
- 注意: start 経路の `exec_before_ready` / `exec_after_start` で `container_ready_conditions` を持つ `ExecCommand` にもタイムアウトと事前エラーが適用される (従来は `startup_timeout` の外側で無制限だった)。既定では条件なし (`ExecCommand::new`) のため影響はない。事前エラーはコンテナ内コマンドを実行せずに start 失敗 → コンテナ削除になる波及もある。また、ready 待機は呼び出しごとにタイムアウトを持つため、start 経路は最悪 (exec の ready 待機の数 + 1) × `startup_timeout` を消費し得る

## 完了条件

- ログ FD が無い (`log_source = None`) コンテナへの exec (`container_ready_conditions` に `WaitFor::Log`) が永久待ちせず、明示エラーになること (クレート内単体テスト。`ContainerAsync` を `ContainerLogSource::None` で直接構築して検証する。統合テストでは `containerLogs` の失敗を決定論的に再現できないため対象外)
- exec の ready_conditions がタイムアウトで打ち切られること (macOS の統合テスト。`with_startup_timeout` で短縮した値を使い、コンテナの init を長寿命コマンド (`tail -f` 等) にしたコンテナに対するマッチしない `WaitFor::Log` 待機で検証する)
- rustdoc (`src/core/containers/async_container.rs` の `exec`) の「無期限に待ち得る」記述と、`docs/TESTCONTAINERS.md` の exec / LogWaitStrategy の記述が実装に合わせて更新されること

## 解決方法

- `src/core/containers/async_container.rs` の `exec` で、事前エラー (ログ FD 欠如 + `WaitFor::Log`) の検査をコンテナ内コマンド実行 (`c.exec`) の**前**に行い、ready_conditions 待機を `startup_timeout` 付きに変更する (検査をコマンド実行の前に置くことで、単体テストで XPC を呼ばずに検証できる)
- `src/core/containers/async_container.rs` (または `src/runners/async_runner.rs`) の `#[cfg(test)]` モジュールに、ログ FD 欠如 + exec Log 待機のテストを追加する
- `tests/container_macos.rs` にタイムアウト打ち切りの統合テストを追加する
