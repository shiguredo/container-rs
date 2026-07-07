# 規約: テストの英語ログメッセージ日本語化と issue 番号言及の削除

- Priority: Medium
- Created: 2026-07-22
- Completed:
- Model: Claude Sonnet 4

## 目的

AGENTS.md の「テストのログメッセージは全て日本語にすること」と shiguredo-issues の「issue 番号をソースコードに持ち込まないこと」の規約違反を修正する。

## 優先度根拠

規約違反であり OSS 公開前に整えるべきだが、機能・安全性には影響しないため Medium。

## 現状

### 英語ログメッセージ (tests/test_container_macos.rs)

- `skip_unless_rosetta()` の `eprintln!` が英語（:34-38）
- 英語の assert メッセージが 30 件以上（`"container should be running"`, `"exit code should be 42"` 等）
- スキップメッセージのプレフィックスが `"SKIP:"` と `"スキップ:"` で混在

### issue 番号言及 (src/core/containers/sync_container.rs:363)

```rust
/// future を実行し、panic しないこと (blocking の block_on panic 回帰テスト)。
```

## 設計方針

- 全 assert メッセージ・eprintln を日本語に統一する
- スキップメッセージのプレフィックスを `"スキップ:"` に統一する
- issue 番号言及を理由そのものに置き換える: 「(同一 Runtime 再入検出の回帰テスト)」等

## 完了条件

- [ ] tests/ 配下の全ログメッセージ・assert メッセージが日本語になっていること
- [ ] スキップメッセージのプレフィックスが統一されていること
- [ ] ソースコード内に issue 番号への言及がなくなること
- [ ] `cargo test --all-features` が pass すること
