# 機能追加: macOS で with_copy_to の uid/gid を対応する

- Priority: Low
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/macos-copy-uid-gid
- Polished:

## 目的

macOS (Apple Container) バックエンドで `CopyTargetOptions` の `uid` / `gid` をコンテナ内のファイルに反映する。現状は XPC `containerCopyIn` が uid/gid をサポートしないため無視されている。

## 優先度根拠

uid/gid の指定は、コンテナ内の特定ユーザーでファイルを読み書きするテストで必要。ただし root で動作するコンテナが大多数であり、需要は限定的。Low。

## 現状

- `CopyTargetOptions` には `uid` / `gid` フィールドがあるが、XPC `containerCopyIn` はこれを受け付けない
- `copy_in` は `mode` (fileMode) のみ反映し、uid/gid は無視している
- コピー後のファイルはコンテナ内のデフォルトユーザー (通常は root) の所有になる

## 設計方針

- `containerCopyIn` 後に exec で `chown {uid}:{gid} {path}` を実行するフォールバック方式を採用する
- uid/gid が未指定 (None) の場合は chown をスキップする
- chown の失敗は warn ログのみで、エラーにはしない (権限不足で失敗し得るため)

## 完了条件

- [ ] `uid` / `gid` 指定時にコピー後のファイルの所有者が変更されること
- [ ] uid/gid 未指定時は chown が実行されないこと
- [ ] 単体テストまたは統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
