# バグ: 非 UTF-8 の XPC 応答が空文字に置換され JSON パースエラーと誤診断される

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-xpc-non-utf8-response
- Polished: {YYYY-MM-DD}

## 目的

XPC の応答データが非 UTF-8 だったときに、実データを破棄して「invalid index JSON」等の誤診断エラーに化けるのをなくし、UTF-8 エラーであることを明示する。

## 現状

`src/core/client/xpc_client.rs` の `containerList` 処理は、応答データの UTF-8 変換失敗を空文字に置換している。

```rust
let text = std::str::from_utf8(&data).unwrap_or("");
let parsed = nojson::RawJson::parse(text).map_err(|e| ClientError::Json(e.to_string()))?;
```

- 非 UTF-8 バイト列が `""` に置換され、`RawJson::parse("")` の失敗が `ClientError::Json` として報告される。UTF-8 が原因である情報が失われる
- 同クレート内の他経路は非対称に正しい: `image_config.rs` は「index is not UTF-8」、`xpc_client.rs` の `imageDescriptions` 処理も同様に UTF-8 エラーを明示している
- エラーにはなるため「握り潰して成功する」わけではないが、原因の特定が難しい誤診断になる

## 設計方針

- `from_utf8` の失敗を `ClientError::Other` (または既存のパターンに合わせた型) で「応答が UTF-8 でない」ことを明示するエラーに変換する
- 他経路 (`image_config.rs` 等) の文言と揃える

## 完了条件

- 非 UTF-8 応答に対して UTF-8 エラーであることが分かるエラー文言が返ること
- 正常な応答のパース挙動が変わらないこと
