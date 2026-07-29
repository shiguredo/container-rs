# 機能追加: Linux で with_platform を対応する

- Priority: Low
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-platform
- Polished: 2026-07-29

## 目的

Linux (Docker Engine API) バックエンドで `with_platform` を Docker の pull / create に反映する。

## 現状

- `with_platform`: `ContainerRequest` には保存されるが、`linux_unsupported_request_reason` (`src/runners/async_runner.rs`) が `build_container_config` の呼び出しより前に**明示エラーで拒否**している ("with_platform() is not implemented on Linux")
- Linux 用 `build_container_config` (`src/runners/async_runner.rs`) は `ContainerConfig` に platform をマッピングしていない
- `DockerClient::pull_image` (`src/core/client/docker_client.rs`) は `descriptor` のみ受け取り、`platform` クエリパラメータを付与しない。macOS の `XpcClient::pull_image` は `platform_arch` を受け取る
- `DockerClient::create_container` (`src/core/client/docker_client.rs`) も `platform` クエリパラメータを付与しない
- macOS では `normalize_platform` (`src/core/client/container_cfg.rs`) で正規化し、XPC の `rosetta` / `ociPlatform` / `platform.architecture` に反映している。`normalize_platform` は `container_cfg.rs` にあり、このモジュールは `#[cfg(target_os = "macos")]` で macOS 限定コンパイルである
- Docker Engine API では `POST /images/create` と `POST /containers/create` のどちらも `platform` は**クエリパラメータ** (`?platform=linux/amd64`) であり、JSON ボディのフィールドではない

## 設計方針

- `linux_unsupported_request_reason` (`src/runners/async_runner.rs`) から `platform` のガードを削除する
- `DockerClient::pull_image` (`src/core/client/docker_client.rs`) のシグネチャに `platform: Option<&str>` を追加し、`POST /images/create` に `platform` クエリパラメータを付与する。呼び出し元 (`AsyncRunner::pull_image`、`resolve_or_pull_linux`、`resolve_image_descriptor` 内部の pull) すべてに platform を浸透させる
- `DockerClient::create_container` (`src/core/client/docker_client.rs`) に `platform` クエリパラメータを追加する。`ContainerConfig` (`src/core/client.rs`) に `platform: Option<String>` フィールドを追加し、`build_container_config` (`src/runners/async_runner.rs`) で `ContainerRequest` から値をマッピングする
- platform の正規化は macOS の `normalize_platform` をそのまま共有しない (macOS 限定コンパイルかつ Apple Silicon 前提のため)。Linux 用には `os/arch[/variant]` 形式のバリデーションのみ行い、不正な形式は `None` として無視する (macOS と同じフォールバック方針)
- macOS と同様にネットワークの自動作成は行わない (事前に `docker network create` が必要)
- 存在しないネットワークを指定した場合は Docker 側がエラーを返す (現状の `create_container` のエラー伝播で十分)

## 完了条件

- [ ] Linux で `with_platform("linux/amd64")` が pull / create に反映されること
- [ ] 不正な platform 文字列は無視されること (macOS と同じ挙動)
- [ ] 統合テストが追加されていること (最低限: `build_container_config` の出力に platform が反映されることの検証。amd64 ホスト上での arm64 指定は QEMU/binfmt_misc が必要なため、CI 環境で実行できない場合はローカルテストのみでよい)
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の関連箇所が実装済みに更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
