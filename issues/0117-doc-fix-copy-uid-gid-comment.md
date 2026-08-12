# ドキュメント: CopyTargetOptions の型コメントが chown 実装と矛盾するのを修正する

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/doc-fix-copy-uid-gid-comment
- Polished: {YYYY-MM-DD}

## 目的

`src/core/copy.rs` の `CopyTargetOptions` の型コメントが現在の実装 (chown による uid / gid 反映) と食い違っており、読者を誤導するのを修正する。

## 現状

`src/core/copy.rs` の `CopyTargetOptions` のコメント:

```
macOS (Apple container XPC) では `containerCopyIn` が `fileMode` のみを受け付けるため、
`mode` のみが反映され、`uid` / `gid` は反映できない。
```

- 実装は chown で反映している: `async_runner.rs` のコピー処理は「uid/gid が非ゼロの場合、chown で所有者を変更する」(`chown_after_copy` 相当) を実装済み
- docs 側 (`docs/TESTCONTAINERS.md`) は「macOS: コピー後 chown で反映 (非ゼロの場合のみ)」と正しく記載しており、型コメントのみが古い
- 本 issue の対応はコメント修正のみ (コード変更なし)

## 設計方針

- 型コメントを実装に合わせて「mode は XPC で反映、uid / gid はコピー後の chown で反映 (非ゼロの場合のみ)」と書き換える

## 完了条件

- `CopyTargetOptions` のコメントが実装・docs と一致すること
- コード変更がないこと (ビルド・テストは通るままで良い)
