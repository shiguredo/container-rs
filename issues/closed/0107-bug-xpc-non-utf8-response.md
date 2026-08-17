# バグ: 非 UTF-8 の XPC 応答が空文字に置換され JSON パースエラーと誤診断される

- Created: 2026-08-12
- Completed: 2026-08-14
- Branch: feature/fix-xpc-non-utf8-response
- Polished: 2026-08-12

## 目的

XPC の応答データが非 UTF-8 だったときに、実データを破棄して JSON パースエラー (例: `unexpected EOS`) に化けるのをなくし、UTF-8 エラーであることを明示する。

## 現状

`src/core/client/xpc_client.rs` の `with_first_container` (containerList 処理。`container_state` / `bridge_ip_address` / `gateway_ip_address` の 3 経路に波及) は、応答データの UTF-8 変換失敗を空文字に置換している。

```rust
let text = std::str::from_utf8(&data).unwrap_or("");
let parsed = nojson::RawJson::parse(text).map_err(|e| ClientError::Json(e.to_string()))?;
```

- 非 UTF-8 バイト列が `""` に置換され、`RawJson::parse("")` の失敗が `ClientError::Json` (nojson の `unexpected EOS at byte position 0`) として報告される。UTF-8 が原因である情報が失われ、原因の特定が難しい誤診断になる (エラーにはなるため「握り潰して成功する」わけではない)
- 同クレート内の他経路は非対称に正しい: `image_config.rs` は「index is not UTF-8」、`xpc_client.rs` の `imageDescriptions` 処理 (`match_image_descriptor`) も同様に UTF-8 エラーを明示している
- なお `src/xpc/conn.rs` の `RawReply::json_error` にも同じ `unwrap_or("")` パターンが残っており、非 UTF-8 のエラー応答が `XPC error unparseable` に化ける (本 issue の対象に含める。一方 `RawReply::string` の非 UTF-8 は `None` を返すだけで JSON パース誤診断には化けないため対象外)

## 設計方針

- `from_utf8` の失敗を `ClientError::Json` で「応答が UTF-8 でない」ことを明示するエラーに変換する (既存の `image_config.rs` / `match_image_descriptor` と同じ型・文言パターン。`ClientError::Other` は既存 5 箇所のパターンと非対称になるため不採用)
- 文言は既存パターンに合わせ `containerList response is not UTF-8: {e}` とする (`imageDescriptions is not UTF-8` の先例)
- `RawReply::json_error` (conn.rs) も同じパターンのため対象に含める。型は既存どおり `ClientError::Xpc` のまま文言のみ変更する (`ClientError::Json` に変えると `is_not_found_error` のプレフィクス判定が壊れる)。文言は `XPC error unparseable (response is not UTF-8)` 等の形にする

## 完了条件

- 非 UTF-8 応答に対してエラー文言に `is not UTF-8` が含まれること (with_first_container 側は `ClientError::Json`、json_error 側は `ClientError::Xpc` のまま)
- 正常な応答のパース挙動が変わらないこと
- 非 UTF-8 応答の検証は単体テスト可能な形に分離して検証すること (両経路とも。`match_image_descriptor_rejects_non_utf8` の先例)
- 修正で陳腐化する `RawReply::json_error` の doc コメント (「`Error::Other` を作る」とあるが実態は `Xpc`) が更新されること
- `CHANGES.md` に `[FIX]` エントリが記載されること

## 解決方法

`src/core/client/xpc_client.rs` と `src/xpc/conn.rs` を修正した。

- `with_first_container` の UTF-8 変換 + Parsing を `parse_container_list` に分離し、非 UTF-8 応答を `ClientError::Json("containerList response is not UTF-8: {e}")` で明示するようにした (従来は `unwrap_or("")` で空文字化し `unexpected EOS` と誤診断していた)
- `RawReply::json_error` を `parse_xpc_error_response` に分離し、非 UTF-8 応答を `ClientError::Xpc("XPC error unparseable (response is not UTF-8)")` で明示するようにした (従来は `unparseable` のみ)
- 分離した 2 関数は非 UTF-8・JSON 破損・正常 JSON の 3 分岐を単体テストで検証 (計 6 テスト追加)。`is_not_found_error` のプレフィクス判定への影響なし (非 UTF-8 文言は notFound プレフィクスと一致しない)
- `json_error` の doc コメントを「`Xpc` エラーを作る」に更新した
- `CHANGES.md` の `## develop` に `[FIX]` エントリを追記した
