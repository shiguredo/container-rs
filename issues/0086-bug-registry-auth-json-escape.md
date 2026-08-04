# バグ: レジストリ認証の JSON エスケープが制御文字を無視し、資格情報に改行等が含まれると認証が失敗する

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-registry-auth-json-escape
- Polished: {YYYY-MM-DD}

## 目的

`X-Registry-Auth` ヘッダ用 JSON のエスケープがバックスラッシュとダブルクォートのみで、制御文字 (改行・タブ等) をそのまま通すため、特殊文字を含む資格情報で不正 JSON になり認証が失敗する問題を修正する。

## 現状

- `src/core/client/registry_auth.rs` の `escape_json_value` は `\` と `"` のみをエスケープする
- config.json の `auth` フィールドは base64 デコード後に `username:password` に分割され、この関数でエスケープして `{"username":"...","password":"...","serveraddress":"..."}` を組み立てる
- パスワード等に改行・タブ・制御文字が含まれると、生成される JSON の構文が壊れ、daemon 側でパースに失敗して認証が通らない (JSON インジェクション自体は quote / backslash のエスケープで防がれているが、構文破壊は防げない)
- 同じファイル内で使うべき参照実装として、`src/core/client/docker_client.rs` の `escape_json` は `< 0x20` を `\uXXXX` 化しており、処理が不統一

## 設計方針

- `escape_json_value` を `docker_client.rs` の `escape_json` と同等の実装 (制御文字の `\uXXXX` 化を含む) に置き換える

## 完了条件

- 改行・タブを含む資格情報でも `X-Registry-Auth` の JSON が構文として有効になること (単体テスト)
- 既存のエスケープテストが引き続き通ること

## 解決方法

- `src/core/client/registry_auth.rs` の `escape_json_value` を制御文字対応に修正する (または共通ヘルパー化して `docker_client.rs` の `escape_json` と統合する)
- 制御文字入り資格情報の単体テストを追加する
