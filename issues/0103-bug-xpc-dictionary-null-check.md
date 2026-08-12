# バグ: XPC 辞書作成の失敗 (NULL 返却) が未チェックで C 側クラッシュ経路になる

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-xpc-dictionary-null-check
- Polished: {YYYY-MM-DD}

## 目的

XPC 辞書の作成失敗 (メモリ枯渇等で NULL が返る) が未チェックのまま C 関数に渡り、プロセスがクラッシュする経路をなくす。

## 現状

`src/xpc/conn.rs` の `send_with_timeout` は `xpc_bridge_create_dictionary()` の戻り値に NULL チェックをせず、そのまま `xpc_bridge_dictionary_set_string` 等に渡す。

```rust
let msg = unsafe { xpc_bridge_create_dictionary() };
unsafe {
    xpc_bridge_dictionary_set_string(msg, rk.as_ptr(), rv.as_ptr());
}
```

- `src/xpc_bridge.c` の `xpc_bridge_create_dictionary` は `xpc_dictionary_create` の結果をそのまま返し (失敗時 NULL)、`xpc_bridge_dictionary_set_string` 側に NULL ガードは無い
- 同関数内の `xpc_bridge_dictionary_set_fd` 失敗時は既に「作成済み msg を解放して Err を返す」パターンが実装済み (conn.rs の `KeyValue::Fd` 分岐) で、辞書作成失敗の分岐だけが欠落している
- `XpcConn::connect` (conn.rs) には接続の NULL チェックがあり、辞書側だけが未チェックの非対称
- 同一辞書に複数の set を行うため、途中失敗時の解放漏れも同時に考慮する必要がある

## 設計方針

- `xpc_bridge_create_dictionary()` の戻り値が NULL なら、`ClientError::Xpc` を返して早期リターンする
- 途中失敗時は既存の `KeyValue::Fd` 分岐と同じく `xpc_bridge_release(msg)` を呼んでから Err を返す (リークさせない)
- C 側 (`xpc_bridge.c`) の `xpc_bridge_create_dictionary` に NULL を返す経路がある旨のコメントを残す

## 完了条件

- 辞書作成失敗 (NULL) 時に Rust 側でエラーを返し、NULL が C 関数に渡らないこと
- 通常経路 (辞書作成成功時) の挙動が変わらないこと
