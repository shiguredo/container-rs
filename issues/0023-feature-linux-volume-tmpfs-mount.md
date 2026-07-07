# 機能追加: Linux で Volume/Tmpfs マウントを対応する

- Priority: Medium
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/linux-volume-tmpfs-mount
- Polished:

## 目的

Linux (Docker Engine API) バックエンドで Volume マウントと Tmpfs マウントを実装する。現状は Bind マウントのみ対応している。

## 優先度根拠

Volume マウントはデータ永続化テスト、Tmpfs マウントは一時領域の性能テストで必要。Bind マウントで代替可能なケースが多いが、本家 testcontainers-rs との互換性のためにも対応が望ましい。Medium。

## 現状

- `Mount::volume_mount` / `Mount::tmpfs_mount` は Linux の create 時に `"volume mount is not implemented on Linux"` / `"tmpfs mount is not implemented on Linux"` の明示エラー
- Bind マウントは `HostConfig.Binds` に `source:target:ro|rw` で反映済み
- Docker Engine API では `HostConfig.Mounts` に `Type: "volume"` / `Type: "tmpfs"` で設定する

## 設計方針

- `CreateContainerBody` の `HostConfig` に `Mounts` 配列を追加する
- Volume マウント: `{ "Type": "volume", "Source": name, "Target": container_path, "ReadOnly": bool }`
- Tmpfs マウント: `{ "Type": "tmpfs", "Target": container_path, "TmpfsOptions": { "SizeBytes": N, "Mode": M } }`
- `MountTmpfsOptions` の `size_bytes` / `mode` を `TmpfsOptions` に映射する
- 既存の Bind マウントは `HostConfig.Binds` のまま維持するか、`Mounts` に統一するかを検討する

## 完了条件

- [ ] Linux で `Mount::volume_mount` が Docker の HostConfig.Mounts に反映されること
- [ ] Linux で `Mount::tmpfs_mount` が Docker の HostConfig.Mounts に反映されること
- [ ] Tmpfs の `size_bytes` / `mode` オプションが反映されること
- [ ] 統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
