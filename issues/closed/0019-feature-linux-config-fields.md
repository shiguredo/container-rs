# 機能追加: Linux で Config 未反映項目を対応する (hostname/open_stdin)

- Priority: Low
- Created: 2026-07-21
- Completed: 2026-08-01
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-config-fields
- Polished: 2026-07-29

## 目的

Linux (Docker Engine API) バックエンドで `ContainerRequest` に保存されているが Docker の create JSON に未反映の Config 項目を対応する。

## 現状

- `with_hostname` / `with_open_stdin`: `ContainerRequest` には保存されるが、`linux_unsupported_request_reason` (`src/runners/async_runner.rs`) が `build_container_config` の呼び出しより前にこれらを**明示エラーで拒否**している ("not implemented on Linux")
- Linux 用 `build_container_config` (`src/runners/async_runner.rs`) は `ContainerConfig` にこれらをマッピングしていない
- `CreateContainerBody` (`src/core/client/docker_client.rs`) の `to_json_string` に `Hostname` / `OpenStdin` の出力はない
- `tests/container_linux.rs` の `unsupported_image_ext_fails_fast_on_start` テストが `with_hostname` の Err を明示的にアサートしている

## 設計方針

- `linux_unsupported_request_reason` (`src/runners/async_runner.rs`) から `hostname` / `open_stdin` の 2 ガードを削除する
- `ContainerConfig` (`src/core/client.rs`) に `hostname: Option<String>` / `open_stdin: Option<bool>` を追加する
- `build_container_config` (`src/runners/async_runner.rs`) で `ContainerRequest` から値をマッピングする
- `CreateContainerBody` (`src/core/client/docker_client.rs`) に `hostname` / `open_stdin` フィールドを追加し、`from_config` で `ContainerConfig` から値を受け渡し、`to_json_string` で `Hostname` / `OpenStdin` を JSON に出力する。`Hostname` は `Some` 時のみ出力、`OpenStdin` は `Some(true)` のみ出力し `None` / `Some(false)` は省略する (Docker のデフォルトは `false`)
- `ContainerConfig` のフィールド追加に伴い、テストヘルパー `config_with_mounts` (`src/core/client/docker_client.rs`) にも 2 フィールドのデフォルト値を追加する
- hostname のフォールバックは Docker に任せる (macOS のような container_name / コンテナ ID へのフォールバックは実装しない)

## 完了条件

- [ ] Linux で `with_hostname` が Docker の `Hostname` に反映されること
- [ ] Linux で `with_open_stdin` が Docker の `OpenStdin` に反映されること
- [ ] `tests/container_linux.rs` の `unsupported_image_ext_fails_fast_on_start` から `with_hostname` の Err 期待を削除すること (当該テストは hostname のみ検証しているため、テスト全体を削除する)
- [ ] 統合テストが追加されていること (最低限: `build_container_config` の出力に各フィールドが反映されることの検証)
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の関連箇所が実装済みに更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

- `linux_unsupported_request_reason` から hostname / open_stdin の 2 ガードを削除した
- `ContainerConfig` に `hostname: Option<String>` / `open_stdin: Option<bool>` を追加し `build_container_config` で映射した
- `CreateContainerBody` に 2 フィールドを追加し `to_json_string` で Hostname / OpenStdin を JSON 出力した
- `unsupported_image_ext_fails_fast_on_start` テストを削除した（hostname のみ検証のため）
- docs/TESTCONTAINERS.md と skills/shiguredo-container/SKILL.md を更新した
- CHANGES.md に [ADD] エントリを追加した
