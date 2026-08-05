# バグ: レジストリ認証の JSON エスケープが制御文字を無視し、資格情報に改行等が含まれると認証が失敗する

- Created: 2026-08-04
- Completed: 2026-08-05
- Branch: feature/fix-registry-auth-json-escape
- Polished: 2026-08-04

## 目的

`X-Registry-Auth` ヘッダ用 JSON のエスケープがバックスラッシュとダブルクォートのみで、制御文字 (改行・タブ等) をそのまま通すため、特殊文字を含む資格情報で不正 JSON になり認証が失敗する問題を修正する。

## 現状

- `src/core/client/registry_auth.rs` の `escape_json_value` は `\` と `"` のみをエスケープする
- config.json の `auth` フィールドは base64 デコード後に `username:password` に分割され、この関数でエスケープして `{"username":"...","password":"...","serveraddress":"..."}` を組み立てる (identitytoken 経路でも同じ関数を使う)
- パスワード等に改行・タブ・制御文字が含まれると、生成される JSON の構文が壊れ、daemon 側でパースに失敗して認証が通らない (JSON インジェクション自体は quote / backslash のエスケープで防がれているが、構文破壊は防げない。制御文字 (0x00-0x1F) は JSON 文字列内でエスケープ必須)
- 同一の親モジュール (`core::client`) 配下の参照実装として、`src/core/client/docker_client.rs` の `escape_json` は `\b` / `\f` / `\n` / `\r` / `\t` の短縮エスケープと、それ以外の `< 0x20` の `\uXXXX` 化を行っており、処理が不統一

## 設計方針

- `escape_json_value` を `docker_client.rs` の `escape_json` と同じエスケープロジック (制御文字のエスケープを含む) に修正する
- 返り値は現行どおり引用符なし (呼び出し側の `format!` で包む形式) を維持する (`escape_json` は引用符付きを返すため、そのまま流用すると二重引用符になる)
- `docker_client.rs` の `escape_json` との統合 (共通ヘルパー化) は本 issue のスコープ外とする

## 完了条件

- 制御文字 (改行・タブ等の `< 0x20` の代表ケース。短縮エスケープ (`\n` 等) と `\uXXXX` 化の両系統を含む) を含む資格情報 (username / password / identitytoken) でも `X-Registry-Auth` の JSON が構文として有効になること (単体テスト)
- 既存の `extract_auth_entry` テストが引き続き通ること

## 解決方法

- `src/core/client/registry_auth.rs` の `escape_json_value` を、`docker_client.rs` の `escape_json` と同じエスケープロジック (制御文字の短縮エスケープ `\b` / `\f` / `\n` / `\r` / `\t` と、それ以外の `< 0x20` の `\uXXXX` 化) に修正した。返り値は現行どおり引用符なし (呼び出し側の `format!` で包む) を維持する
- `docker_client.rs` の `escape_json` との統合 (共通ヘルパー化) は本 issue のスコープ外とした (両者の重複は既知のドリフト源であり、別 issue として追跡が必要)
- テスト: `escape_json_value` の直接テスト (短縮エスケープ全種 + `\u0001` + 引用符/バックスラッシュ)、nojson 往復テスト (境界値 `\u0000` / `\u001f`・空文字列を含む)、auth フィールド経路 (base64) と identitytoken 経路 (JSON エスケープ表記) の制御文字入り資格情報テストを追加した
