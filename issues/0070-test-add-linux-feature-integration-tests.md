# テスト: Linux の機能追加 [ADD] 項目の統合テストを追加する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/add-linux-feature-integration-tests
- Polished: 2026-08-02

## 目的

CHANGES.md の develop で「Linux で対応」と謳う機能群のうち、統合テストが無い項目の回帰検出の穴を塞ぐ。

## 現状

CHANGES.md の develop の [ADD] 項目のうち、以下の Linux 実装に統合テストが皆無 (または単体テストのみ):

- `pause` / `unpause` (`ContainerAsync::pause` / `unpause`) — 実行経路自体が 0 テスト
- `with_network` (NetworkingConfig)・`with_cap_add` / `with_cap_drop`・`with_shm_size`・`with_readonly_rootfs`・`with_hostname`・`with_open_stdin`・`with_host` (ExtraHosts)・`Mount::tmpfs_mount` (HostConfig.Mounts)
- `src/core/client/docker_client.rs` の `CreateContainerBody::from_config` の単体テストは Bind / Volume / Tmpfs の 3 種のみで、HostConfig / Config / NetworkingConfig のフィールド反映を検証していない

macOS 側は `with_hostname` / `with_readonly_rootfs` / `with_open_stdin` / `with_platform` / `with_privileged` / `with_user` / `with_host` / `with_network` / bind / tmpfs を実機検証しているが、Linux 側には統合テストが無い。`with_cap_add` / `with_cap_drop` / `with_shm_size` は macOS 側にもテストが無い (両 OS 未検証)。Linux の Volume マウントの統合テストは 0069 が担当する。Healthcheck / ExitWaitStrategy は統合テスト済み、X-Registry-Auth は資格情報が必要で統合テストが困難なため、本 issue の対象外とする。

## 設計方針

検証はコンテナ内 exec または外部 docker CLI (`docker inspect -f` の go-template) で行う (ライブラリ API には inspect 全体を取得する手段が無いため。外部 CLI は既存テストに使用実績がある)。対象は上記の列挙に限定し、`with_platform` (ホストと異なる arch の指定は binfmt なしでは start できない。同じ arch を指定しても platform クエリの反映有無を観測する手段が無いため CI で検証できない)・`with_init` / `with_privileged` / `with_user` / `with_working_dir` / `with_label(s)` (develop の [ADD] 項目ではないため)・通常の bind (rw) は対象外とする。

- `pause` / `unpause`: Docker は pause 中も `State.Running` が true のままのため、`is_running()` では検証できない。外部 docker CLI の inspect (`State.Paused`) で検証する
- `with_hostname`: exec `hostname` で検証する (Config の Hostname フィールド)
- `with_readonly_rootfs`: コンテナ内 `touch` の失敗 (exec の exit code が 0 以外) で検証する (対象パスは `/readonly_probe` 等のルート直下。tmpfs 等で上書きされるパスは避ける)
- `with_network`: デフォルトの `bridge` では NetworkingConfig 未指定時と区別できないため、`docker network create` で作成したカスタムネットワークを指定して検証する (後始末では `docker network rm` を行う)
- その他 (`with_cap_add` / `with_cap_drop` / `with_shm_size` / `with_open_stdin` / `with_host` / `Mount::tmpfs_mount`): 外部 docker CLI の inspect で検証する (フィールド: `HostConfig.CapAdd` / `HostConfig.CapDrop` / `HostConfig.ShmSize` / `Config.OpenStdin` / `HostConfig.ExtraHosts` / `HostConfig.Mounts`。`{{json ...}}` で出力を確認する)

`from_config` の HostConfig / Config / NetworkingConfig のフィールド反映は、上記の統合テスト (実 Docker 経由) で実質的にカバーされる。単体テストの拡充は行わない。

テストで発見された既存実装のバグは、本 issue では修正せず bug カテゴリの別 issue を起票する。

## 完了条件

- 上記の列挙項目 (`pause` / `unpause`・`with_network`・`with_cap_add` / `with_cap_drop`・`with_shm_size`・`with_readonly_rootfs`・`with_hostname`・`with_open_stdin`・`with_host`・`Mount::tmpfs_mount`) の統合テストが追加され、検証される
- `RUN_CONTAINER_TESTS=1 cargo test --all-features --test container_linux` が pass すること (CI (`test-linux-docker` ジョブの `cargo test --all-features`) でも実行される。Linux テストはゲートなしで実行される)

## 解決方法

- `with_hostname` → exec `hostname` で検証 (テスト名は既存の命名に合わせて `alpine_with_hostname` 系)
- `with_readonly_rootfs` → コンテナ内 `touch` の失敗で検証 (`alpine_with_readonly_rootfs` 系)
- `pause` / `unpause` → 外部 docker CLI の inspect (`State.Paused`) で検証。1 つのテストで pause 後と unpause 後の `State.Paused` の遷移を検証する (`alpine_pause_unpause` 系)
- `with_network` → `docker network create` のカスタムネットワーク (テスト名・PID を含む一意名) を指定し、inspect の `NetworkSettings.Networks` で確認 (`alpine_with_network` 系)
- `with_cap_add` → `NET_ADMIN` を指定し、inspect の `HostConfig.CapAdd` で確認 (`alpine_with_cap_add` 系)
- `with_cap_drop` → inspect の `HostConfig.CapDrop` で確認 (`alpine_with_cap_drop` 系)
- `with_shm_size` → 128 MiB を指定し、inspect の `HostConfig.ShmSize` で確認 (`alpine_with_shm_size` 系)
- `with_open_stdin` / `with_host` / `Mount::tmpfs_mount` → inspect の各フィールドで確認 (`alpine_with_open_stdin` / `alpine_with_host` / `alpine_with_tmpfs_mount` 系)
- `tests/container_linux.rs` を変更するため、同一ファイルを変更する 0065 / 0066 / 0069 とマージ順に注意する
