# バグ: XPC 辞書作成の失敗 (NULL 返却) が未チェックで C 側クラッシュ経路になる

- Created: 2026-08-12
- Completed: 2026-08-14
- Branch: feature/fix-xpc-dictionary-null-check
- Polished: 2026-08-12

## 目的

XPC 辞書の作成失敗 (メモリ枯渇等で NULL が返る) が未チェックのまま C 関数に渡り、プロセスがクラッシュする経路をなくす。

## 現状

`src/xpc/conn.rs` の `send_with_timeout` は `xpc_bridge_create_dictionary()` の戻り値に NULL チェックをせず、そのまま `xpc_bridge_dictionary_set_string` 等に渡す。

```rust
let msg = unsafe { xpc_bridge_create_dictionary() };
let rk = ROUTE_KEY;
unsafe {
    xpc_bridge_dictionary_set_string(msg, rk.as_ptr(), rv.as_ptr());
}
```

- `src/xpc_bridge.c` の `xpc_bridge_create_dictionary` は `xpc_dictionary_create` の結果をそのまま返し (失敗時 NULL)、`xpc_bridge_dictionary_set_string` 側に NULL ガードは無い
- 同関数内の `xpc_bridge_dictionary_set_fd` 失敗時は既に「作成済み msg を解放して Err を返す」パターンが実装済み (conn.rs の `KeyValue::Fd` 分岐) で、辞書作成失敗の分岐だけが欠落している
- `XpcConn::connect` (conn.rs) には接続の NULL チェックがあり、辞書側だけが未チェックの非対称
- 途中失敗時の解放は `KeyValue::Fd` 分岐で対応済みであり、本修正で追加の解放処理は不要 (辞書作成失敗時は msg が NULL のため解放対象が無い)

## 設計方針

- `xpc_bridge_create_dictionary()` の戻り値が NULL なら、`ClientError::Xpc` でエラーメッセージ (例: "failed to create XPC dictionary") を返して早期リターンする (チェックは辞書作成の直後・最初の C 関数呼び出しの前に挿入する)
- 辞書作成失敗 (NULL) 時は解放対象が無いため `xpc_bridge_release` は呼ばない (`xpc_bridge_release` は C 側で NULL ガード済みだが、呼ぶ意味がない)。その後の set 失敗 (`KeyValue::Fd` 分岐) は既存実装のまま
- C 側 (`xpc_bridge.c`) の `xpc_bridge_create_dictionary` に NULL を返す経路がある旨のコメントを残す

## 完了条件

- 辞書作成失敗 (NULL) 時に Rust 側でエラーを返し、NULL が C 関数に渡らないこと (NULL の再現はモック・スタブ禁止のためテスト不能。コードレビューで担保する)
- 通常経路 (辞書作成成功時) の挙動が変わらないこと (既存の conn.rs 単体テストは `send_with_timeout` を呼ばないため、通常経路は macOS 統合テストとコードレビューで担保する)
- 設計方針のとおり C 側 (`xpc_bridge.c`) に NULL を返す経路がある旨のコメントが追加されること
- `CHANGES.md` に `[FIX]` エントリが記載されること

## 解決方法

`src/xpc/conn.rs` の `XpcConn::send_with_timeout` を修正した。

- `xpc_bridge_create_dictionary()` の戻り値に NULL チェックを追加し、NULL なら `ClientError::Xpc("failed to create XPC dictionary")` で早期リターンするようにした (チェックは辞書作成の直後・最初の C 関数呼び出しの前に挿入)
- 辞書作成失敗 (NULL) 時は解放対象が無いため `xpc_bridge_release` は呼ばない (設計方針どおり)。後続の set 失敗 (`KeyValue::Fd` 分岐) の処理は既存実装のまま
- C 側 (`src/xpc_bridge.c`) に NULL を返す経路がある旨のコメントを残し、契約を `src/xpc_bridge.h` に明記した (既存の `xpc_bridge_dictionary_set_fd` と同じヘッダ契約パターン)
- NULL の再現はモック・スタブ禁止のためテスト不能であり、完了条件どおりコードレビューで担保した (通常経路は既存の macOS 統合テストと全テスト通過で担保)
- `CHANGES.md` の `## develop` に `[FIX]` エントリを追記した
