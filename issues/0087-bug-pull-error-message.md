# バグ: イメージ pull 失敗時に daemon のエラーメッセージが捨てられ、認証失敗等の診断ができない

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-pull-error-message
- Polished: {YYYY-MM-DD}

## 目的

Linux の `pull_image` で 4xx / 5xx 応答のボディに載っている daemon のエラーメッセージ (認証失敗 `unauthorized` / `denied` 等) をエラーに含め、診断可能にする。

## 現状

- `src/core/client/docker_client.rs` の `pull_image` は、ステータスコード >= 400 のときボディを読まずに `failed to pull image {descriptor}: {status}` とだけ返す
- pull 失敗時のボディには daemon の JSON エラー (`{"message":"..."}` / `{"errorDetail":...}`) が載るが、`check_pull_stream_errors` は `error` / `errorDetail` フィールドのみを見るため、`{"message":...}` 形式のエラーは拾われない
- レジストリ認証失敗 (プライベートレジストリの資格情報誤り等) は最も起きやすい運用エラーであり、原因が一切読めない
- 既に `parse_daemon_error_message` (同じファイル内) が `{"message":...}` の抽出実装を持つため、容易に改善できる

## 設計方針

- 4xx / 5xx 時にボディの `message` を拾い、`failed to pull image {descriptor}: {status}: {message}` 形式で包む
- ボディが無い・非 JSON の場合は現行の文言を維持する

## 完了条件

- 認証失敗の pull でエラーメッセージに daemon の `message` が含まれること (単体テスト)
- 既存の pull エラーテストの期待値が維持されること

## 解決方法

- `src/core/client/docker_client.rs` の `pull_image` のエラーパスで `parse_daemon_error_message` を呼び、メッセージを付与する
- 単体テストに `{"message":"..."}` 形式の失敗応答のケースを追加する
