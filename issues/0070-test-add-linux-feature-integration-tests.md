# テスト: Linux の機能追加 [ADD] 項目の統合テストを追加する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/add-linux-feature-integration-tests
- Polished: {YYYY-MM-DD}

## 目的

CHANGES.md の develop で「Linux で対応」と謳う機能群に統合テストが無く、回帰検出の穴になっているのを塞ぐ。macOS 側の `xpc_alpine_with_*` 群との非対称を解消する。

## 現状

以下の Linux 実装に統合テストが皆無 (または単体テストのみ):

- `pause` / `unpause` (`ContainerAsync::pause` / `unpause`) — 構築経路自体が 0 テスト
- `with_network` (NetworkingConfig)・`with_platform`・`with_cap_add` / `with_cap_drop`・`with_shm_size`・`with_readonly_rootfs`・`with_hostname`・`with_open_stdin`・`with_host` (ExtraHosts)・`with_init`・`with_privileged`・`with_user`・`with_working_dir`・`with_label(s)`
- `src/core/client/docker_client.rs` の `CreateContainerBody::from_config` の単体テストは Bind / Volume / Tmpfs の 3 種のみで、HostConfig / Config / NetworkingConfig のフィールド反映を検証していない

macOS 側は `xpc_alpine_with_*` 群で同一機能を実機検証しており、Linux だけ未検証のまま [ADD] と記載されている。

## 設計方針

`docker inspect` またはコンテナ内 exec で検証できる範囲から追加する。最低でも `pause` / `unpause` と HostConfig 反映 2〜3 種から始める。

## 完了条件

- `pause` / `unpause` を含む主要な [ADD] 項目が Linux 統合テストで検証される
- CI (`test-linux-docker` ジョブ) で実行される

## 解決方法

- `with_hostname` → exec `hostname` で検証
- `with_readonly_rootfs` → コンテナ内 `touch` の失敗で検証
- `pause` / `unpause` → `is_running` の遷移で検証
- `with_cap_add` / `with_shm_size` 等 → `docker inspect` の JSON で検証
