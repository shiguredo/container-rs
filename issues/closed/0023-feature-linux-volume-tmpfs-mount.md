# 機能追加: Linux で Volume/Tmpfs マウントを対応する

- Priority: Medium
- Created: 2026-07-21
- Completed: 2026-08-01
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-volume-tmpfs-mount
- Polished: 2026-07-29

## 目的

Linux (Docker Engine API) バックエンドで Volume マウントと Tmpfs マウントを実装する。現状は Bind マウントのみ対応している。macOS (XPC) バックエンドでは Volume/Tmpfs ともに実装済みであり、Linux 側のみ未対応。

## 現状

- `Mount::volume_mount` / `Mount::tmpfs_mount` は Linux の create 時に `CreateContainerBody::from_config` (`src/core/client/docker_client.rs`) の `match m.mount_type()` 分岐で `"volume mount is not implemented on Linux"` / `"tmpfs mount is not implemented on Linux"` の明示エラー (`ClientError::Configuration`) を返す。`linux_unsupported_request_reason` (`src/runners/async_runner.rs`) にはマウントのガードは**ない** (エラーは `from_config` 内で発生する)
- Bind マウントは `HostConfig.Binds` に `source:target:ro|rw` で反映済み
- `HostConfig` (`src/core/client/docker_client.rs`) に `Mounts` フィールドは存在しない
- `docker_client.rs` には Volume/Tmpfs がエラーになることを検証するテスト (`from_config_volume_mount_returns_configuration_error` / `from_config_tmpfs_mount_returns_configuration_error`) が存在する
- Docker Engine API では `HostConfig.Mounts` に `Type: "volume"` / `Type: "tmpfs"` で設定する

## 設計方針

- `CreateContainerBody::from_config` (`src/core/client/docker_client.rs`) の Volume/Tmpfs エラー分岐を `Mounts` 構築に置き換える
- `HostConfig` (`src/core/client/docker_client.rs`) に `mounts` フィールドを追加し、`HostConfig::to_json_string` に `"Mounts"` 配列を出力する。`mounts` が空の場合は `Mounts` キーを省略する
- Volume マウント: `{ "Type": "volume", "Source": name, "Target": container_path, "ReadOnly": bool }`。`ReadOnly` は `AccessMode` から bool に変換する (`ReadOnly` → `true`, `ReadWrite` → `false`)。Docker は存在しない名前付きボリュームを自動作成する
- Tmpfs マウント: `{ "Type": "tmpfs", "Target": container_path, "ReadOnly": bool, "TmpfsOptions": { "SizeBytes": N, "Mode": M } }`。`Source` は不要 (tmpfs にソースは存在しない)。`TmpfsOptions` は `MountTmpfsOptions` が `Some` の場合のみ出力し、`size_bytes` / `mode` は個別に `Some` の場合のみ出力する
- 既存の Bind マウントは `HostConfig.Binds` のまま維持する (`Mounts` への統一は行わない。将来統一する場合は別途リファクタリング issue を起票する)
- `ContainerConfig` / `build_container_config` / `config_with_mounts` の変更は不要 (`config.mounts` は既に流通済み)

## 完了条件

- [ ] Linux で `Mount::volume_mount` が Docker の HostConfig.Mounts に反映されること
- [ ] Linux で `Mount::tmpfs_mount` が Docker の HostConfig.Mounts に反映されること
- [ ] Tmpfs の `size_bytes` / `mode` オプションが反映されること
- [ ] `docker_client.rs` の既存エラー検証テスト (`from_config_volume_mount_returns_configuration_error` / `from_config_tmpfs_mount_returns_configuration_error`) を Mounts 反映の検証に置き換えること
- [ ] 単体テストが追加されていること (最低限: `from_config` が Volume/Tmpfs で成功し、`to_json_string` の出力に `Mounts` エントリが含まれることの検証。デーモン不要)
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の関連箇所が実装済みに更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

- `CreateContainerBody::from_config` の Volume/Tmpfs エラー分岐を Mounts 構築に置き換えた
- `HostConfig` に `mounts` フィールドを追加し `to_json_string` で Mounts 配列を JSON 出力した
- Volume: Type=volume + Source + Target + ReadOnly、Tmpfs: Type=tmpfs + Target + ReadOnly + TmpfsOptions (SizeBytes/Mode)
- 既存のエラー検証テストを Mounts 反映の検証テストに置き換えた
- docs/TESTCONTAINERS.md と skills/shiguredo-container/SKILL.md を更新した
- CHANGES.md に [ADD] エントリを追加した
