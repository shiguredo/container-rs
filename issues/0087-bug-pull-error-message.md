# バグ: イメージ pull 失敗時に daemon のエラーメッセージが捨てられ、認証失敗等の診断ができない

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-pull-error-message
- Polished: 2026-08-04

## 目的

Linux の `pull_image` で 4xx / 5xx 応答のボディに載っている daemon のエラーメッセージ (認証失敗 `unauthorized` / `denied` 等) をエラーに含め、診断可能にする。

## 現状

- `src/core/client/docker_client.rs` の `pull_image` は、ステータスコード >= 400 のときボディを読まずに `failed to pull image {descriptor}: {status}` とだけ返す
- 4xx / 5xx のボディには daemon の JSON エラー (`{"message":"..."}` 形式。認証失敗は 401 + `{"message":"unauthorized: ..."}`) が載るが、`pull_image` はこのパスでボディを読まずに捨てている (200 系のストリーム経路の `check_pull_stream_errors` は `error` / `errorDetail` を見るが、`{"message":...}` 形式は 200 ストリームに載らないため本 issue では変更しない)
- 既に `parse_daemon_error_message` (同じファイル内) が `{"message":...}` の抽出実装を持つ (0065 で導入済み)

## 設計方針

- 4xx / 5xx 時にボディの `message` を拾い、`failed to pull image {descriptor}: {status}: {message}` 形式で包む
- ボディが無い・非 JSON・`message` 欠落・`message` が空文字列の場合は現行の文言 (`failed to pull image {descriptor}: {status}`) を維持する (`{"errorDetail":...}` 形式は `message` フィールドを持たないため現行文言に落ちる)
- 4xx / 5xx パスと 200 系ストリームパスのエラー形式は不揃いのままとする (ストリームパスは既存の挙動を変えない)

## 完了条件

- 4xx / 5xx 応答の pull でエラーメッセージに daemon の `message` が含まれること (単体テスト)
- 既存の pull エラーテスト (`tests/pull_image_linux.rs`。メッセージ非空のみを検証) が引き続き通ること

## 解決方法

- `src/core/client/docker_client.rs` の `pull_image` の 4xx / 5xx エラーパスで、エラー文言の組み立てを純粋関数で行う
- エラー文言の組み立て (パース (`parse_daemon_error_message` の呼び出し) とフォールバックを関数内に閉じる) を純粋関数として切り出し、単体テストで検証できるようにする (0065 の `classify_archive_404` と同パターン。`pull_image` は実ソケット接続のため、そのままでは単体テストできない)。`parse_daemon_error_message` は空 message を `Some("")` で返すため、空文字列のフィルタは純粋関数側で行う
- 単体テストに `{"message":"..."}` 形式の失敗応答のケースを追加する (ボディ無し・非 JSON・空 message・`{"errorDetail":{"message":"..."}}` 形式のフォールバックも検証する)
