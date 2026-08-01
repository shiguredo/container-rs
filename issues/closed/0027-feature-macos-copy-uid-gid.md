# 機能追加: macOS で with_copy_to の uid/gid を対応する

- Priority: Low
- Created: 2026-07-21
- Completed: 2026-08-01
- Model: qwen3.8-max-preview
- Branch: feature/add-macos-copy-uid-gid
- Polished: 2026-07-29

## 目的

macOS (Apple Container) バックエンドで `CopyTargetOptions` の `uid` / `gid` をコンテナ内のファイルに反映する。現状は XPC `containerCopyIn` が uid/gid をサポートしないため反映されていない。

## 現状

- `CopyTargetOptions` (`src/core/copy.rs`) には `uid: u32` / `gid: u32` フィールドがある (デフォルトは `0`)。`Option<u32>` ではないため「未指定」を `None` で表現することはできない
- XPC `containerCopyIn` (`src/core/client/xpc_client.rs` の `copy_in`) は `fileMode` のみ受け付け、uid/gid に相当するパラメータは存在しない
- `copy_to_sources` (`src/runners/async_runner.rs`) は `src.target.mode` のみ渡し、`src.target.uid` / `src.target.gid` は参照していない
- コピー後のファイルはコンテナ内のデフォルトユーザーの所有になる (推定。Apple Container の `containerCopyIn` 内部実装に依存)
- `src/core/copy.rs` の rustdoc に「macOS (XPC) では非対応」と明記されている

## 設計方針

- `containerCopyIn` 後に exec で `chown {uid}:{gid} {path}` を実行するフォールバック方式を採用する。`copy_to_sources` (`src/runners/async_runner.rs`) 内で `XpcClient::exec` を直接呼ぶ (既存の `apply_extra_hosts` と同じパターン)
- `uid == 0 && gid == 0` の場合は chown をスキップする (デフォルトが 0:0 であり、containerCopyIn の挙動と同一で no-op のため。`Option<u32>` への変更は公開 API の破壊的変更になるため行わない)
- ディレクトリ一括投入の場合は `chown -R {uid}:{gid} {path}` を実行し、配下のファイル・ディレクトリすべてに適用する (Linux 側の tar ヘッダ uid/gid と粒度を揃える)
- chown の失敗は warn ログのみで、エラーにはしない (権限不足で失敗し得るため)
- `XpcClient::exec` の `ProcCfg` は root 権限 (`UserId { uid: 0, gid: 0 }`) で実行されるため、chown は root 権限で実行される

## 完了条件

- [ ] `uid` / `gid` 指定時 (0:0 以外) にコピー後のファイルの所有者が変更されること
- [ ] `uid == 0 && gid == 0` の場合は chown が実行されないこと
- [ ] ディレクトリ一括投入時に配下のファイル・ディレクトリすべてに chown が適用されること
- [ ] chown 失敗時は warn ログのみでエラーにならないこと
- [ ] 統合テストが追加されていること (`tests/container_macos.rs` に追加)
- [ ] `src/core/copy.rs` の rustdoc («macOS (XPC) では非対応») が実態に合わせて更新されること
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の関連箇所が実装済みに更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

- `copy_to_sources` に `chown_after_copy` ヘルパを追加した
- コピー後に uid/gid が非ゼロの場合、exec で `chown {uid}:{gid} {path}` を実行する
- ディレクトリ一括投入時は `chown -R` を実行する
- chown 失敗時は warn ログのみでエラーにしない
- `src/core/copy.rs` の rustdoc を実態に合わせて更新した
- docs/TESTCONTAINERS.md を更新した
- CHANGES.md に [ADD] エントリを追加した
