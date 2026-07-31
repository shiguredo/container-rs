# testcontainers-rs / Apple Container / Docker Engine API の比較

本表は [testcontainers-rs](https://github.com/testcontainers/testcontainers-rs) 0.27.3 の公開 API を基準に、本クレートの 2 バックエンドを並べて比較する。

| 列 | 意味 |
|:--|:--|
| 本家 | testcontainers-rs 0.27.3 (bollard 経由の Docker Engine) |
| Apple Container | 本クレートの macOS 実装 (XPC)。メイン対象 |
| Docker Engine API | 本クレートの Linux 実装 (`DockerClient` + unix socket)。ライフサイクル・ログ関連・copy・bridge IP 取得は配線済み、ネットワーク詳細等は未実装 |

判定ルール (Apple Container / Docker Engine API 列):

- **対応**: シグネチャあり、当該バックエンドで動作する
- **部分対応**: シグネチャあり、動作するが挙動が限定的 (固定値、空リーダー、Bind のみ反映など)
- **未配線**: 下位クライアント (`DockerClient` 等) に実装があるが、公開 API (`ContainerAsync` 等) から未接続で明示エラーになる
- **未反映**: リクエストには保存されるが、当該バックエンドの設定構築で無視される
- **未実装**: シグネチャはあるが、常にエラー / stub / 機能として成立しない
- **未実装 (XPC 制約)**: Apple Container 側に route / 仕様が無く実装不能 (Apple 列専用)
- **なし**: 本クレートにシグネチャ自体が無い
- **shiguredo 拡張**: 本家に無く本クレートが追加した API

本家列はシグネチャの有無のみを示す (**あり** / **なし**)。

## Apple Container (XPC) の参照情報

XPC route 一覧 (`Sources/Services/ContainerAPIService/Client/XPC+.swift`, `XPCRoute`):

`containerList, containerCreate, containerBootstrap, containerCreateProcess, containerStartProcess, containerWait, containerDelete, containerStop, containerDial, containerResize, containerKill, containerState, containerLogs, containerEvent, containerStats, containerDiskUsage, containerCopyIn, containerCopyOut, containerExport, pluginLoad/Get/Restart/Unload/List, networkCreate/Delete/List, volumeCreate/Delete/List/Inspect, ping, installKernel, getDefaultKernel`

`containerList` の各要素 (`ContainerSnapshot`) が持つフィールド (`Sources/ContainerResource/Container/`):

- `configuration.id: String`
- `configuration.image: ImageDescription`
- `configuration.mounts: [Filesystem]`
- `configuration.publishedPorts: [PublishPort]` — `hostAddress`, `hostPort`, `containerPort`, `proto` (`tcp` | `udp`)
- `configuration.publishedSockets, labels, sysctls, networks, dns, rosetta, initProcess, platform, resources, runtimeHandler, virtualization, ssh, readOnly, useInit, capAdd, capDrop, shmSize, stopSignal, creationDate`
- `status: RuntimeStatus` — `unknown` | `stopped` | `running` | `stopping`
- `networks: [Attachment]` — `network, hostname, ipv4Address (CIDRv4), ipv4Gateway, ipv6Address? (CIDRv6), macAddress?, mtu?, variant?`
- `startedDate: Date?`

`containerWait` レスポンス: `exitCode: Int64`。

`containerLogs` レスポンス: `logs: [FileHandle]` (stdin/stdout/stderr の FD)。

## Docker Engine API (Linux) の現状

`DockerClient` (`src/core/client/docker_client.rs`) は `/var/run/docker.sock` 向けに pull / create / start / stop / remove / exec / inspect / logs / archive (copy) を実装済みである。`ContainerAsync` の Linux 分岐はライフサイクル系 (`ports` / `exec` / `stop` / `is_running` / `rm` / Drop / `start` 再起動 / `container_state`)、ログ関連 (`stdout` / `stderr` / `stdout_to_vec` / `stderr_to_vec` / `WaitFor::Log` / `with_log_consumer`)、copy (`copy_file_from` / `with_copy_to`)、ヘルスチェック (`with_health_check` / `WaitFor::Healthcheck`) を配線済みである。

ログは `GET /containers/{id}/logs` を `spawn_blocking` 内の `UnixStream` で叩き、multiplex フレームを demux して stdout / stderr 別の共有バッファ (ストリームあたり 8 MiB、上限超過時は先頭から drop) へ書き込む。`stdout` / `stderr` のリーダーはこの共有バッファを独立オフセットで読む。

その結果:

- `AsyncRunner::pull_image` / `AsyncRunner::start` は動く (既定の空 ready 条件なら完結する)
- `ports` / `exec` (exit code + stdout / stderr + Env) / `stop` / `is_running` / `rm` / Drop 削除 / `start` 再起動 / `container_state` は公開 API から利用できる
- `stdout` / `stderr` / `stdout_to_vec` / `stderr_to_vec` は demux 済みログを返す。`WaitFor::Log` (`message_on_stdout` / `message_on_stderr` / `message_on_either_std`) と `with_log_consumer` も成立する。`follow=true` は 8 MiB リングで上限超過時は先頭 drop して `warn` ログを出し読み進める。`follow=false` は呼び出しごとに新規 HTTP セッションを張る
- `copy_file_from` は `GET /containers/{id}/archive` の tar を自前 ustar パーサで展開して返す (source は絶対パス必須・ファイル専用)。`with_copy_to` は create 後・start 前に `PUT /containers/{id}/archive?path=/` へ自前 ustar を投入する (親ディレクトリ自動作成・ディレクトリ一括投入対応。配下 regular file の `mode` / `uid` / `gid` は反映。中間 directory の mode は `0o755`。コピー後 mtime は epoch)。Linux: create 後・start 前に PUT /archive。macOS: start_process 後の containerCopyIn（レースあり。親作成は `createParents`）。起動前投入は Linux のみの公開契約
- `ExitWaitStrategy` は未実装エラーのまま
- `ImageExt` の一部 (`with_network` / `with_platform` / `with_cap_add` / `with_shm_size` / `with_readonly_rootfs` / `with_open_stdin` / `with_hostname` / `with_host` / `with_ssh` 等) は start 時に明示エラー (黙って無視しない)。`with_init` は HostConfig.Init に配線済み。`with_health_check` は Config.Healthcheck に配線済みで `WaitFor::Healthcheck` も成立する

README の Linux 注意書きと合わせて読むこと。残ギャップは exec の Env・ネットワーク詳細などである。

## サマリ (Apple Container)

| 状態 | 件数 |
|:--|--:|
| 対応 | 276 |
| 部分対応 | 24 |
| 未実装 (実装可能) | 0 |
| 未実装 (XPC 制約) | 3 |
| なし | 94 |
| shiguredo 拡張 (本家に無い追加 API) | 12 |
| 内部型/内部関数 (対象外) | 4 |

内訳合計: 409 API (判定対象。対象外 4 は含まない。feature ゲート表の「備考」列も集計外)

判定内訳の傾向 (Apple Container):

- **対応** (276): 基本的な `Image` / `ImageExt` / `AsyncRunner` / `SyncRunner` / `ContainerRequest` / `WaitFor` / `LogConsumer` / `Mount` / `ContainerPort` / `Error` / `GenericImage` / `Healthcheck` 型はほぼ揃っている
- **部分対応** (24): シグネチャあり + 動作するが XPC の情報不足 / 型不一致 / 挙動制約付き (例: `get_host` = `localhost` 固定、`exit_code` = 観測済みキャッシュのみ など)
- **未実装 (実装可能)** (0): 現状、判定「未実装 (実装可能)」の行は無い
- **未実装 (XPC 制約)** (3): `WaitFor::Healthcheck` / `healthcheck()` (ヘルス待機) と `with_health_check` (start 時明示エラー)。Apple container 側の仕様として存在しないため実装不能。`pause` / `unpause` はシグネチャ自体を削除済みのため「なし」に分類
- **なし** (94): 大半は build 系、feature 系、bollard 由来の詳細エラー型、未配線の `CgroupnsMode` setter など
- **shiguredo 拡張** (12): `ImageExt::with_init` / `with_ssh`、`ContainerAsync::container_state`、`Container::container_state`、`ClientError::Xpc*` / `ImageNotFound` / `ContainerNotFound` / `Json` / `Other`、`ContainerRequest` の `init` / `ssh` accessor

## サマリ (Docker Engine API)

件数の厳密集計より、現状の読み方を優先する。

- **対応に近いもの**: トレイト / リクエスト型の定義面、`pull_image`、ライフサイクル (`start` / `stop` / `rm` / Drop / `ports` / `is_running` / `container_state` / `exec` の exit code + stdout / stderr)、ログ関連 (`stdout` / `stderr` / `stdout_to_vec` / `stderr_to_vec` / `WaitFor::Log` / `message_on_*` / `with_log_consumer`、8 MiB リングで先頭 drop)、copy (`copy_file_from` / `with_copy_to`。Linux は親ディレクトリ自動作成・ディレクトリ投入対応)、ヘルスチェック (`Healthcheck` / `with_health_check` / `WaitFor::Healthcheck`)、一部の create JSON 反映 (`with_cmd` / `with_mapped_port` / `with_init` 等)
- **未配線・未実装が残るもの**: `ExitWaitStrategy`、exec の Env 本対応
- **未実装 (start 時 fail-fast)**: `with_network` / `with_platform` / `with_cap_*` / `with_shm_size` / `with_readonly_rootfs` / `with_open_stdin` / `with_hostname` / `with_host` / `with_ssh` など、Linux 設定構築に載らない ImageExt

Linux 列の残ギャップは、本表で本家 / Apple / 自前 Docker の差を同時に見せるためのものである。

## 1. `Image` トレイト (`core::image::Image`)

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `name(&self) -> &str` | あり | 対応 | 対応 | Docker: トレイト定義は OS 共通 |
| `tag(&self) -> &str` | あり | 対応 | 対応 | Docker: トレイト定義は OS 共通 |
| `ready_conditions(&self) -> Vec<WaitFor>` | あり | 対応 | 対応 | Docker: トレイト定義は OS 共通 |
| `env_vars(&self) -> impl IntoIterator<Item = (impl Into<Cow<'_, str>>, impl Into<Cow<'_, str>>)>` | あり | 対応 | 対応 | Docker: トレイト定義は OS 共通 |
| `mounts(&self) -> impl IntoIterator<Item = &Mount>` | あり | 対応 | 対応 | Docker: トレイト定義は OS 共通 |
| `copy_to_sources(&self) -> impl IntoIterator<Item = &CopyToContainer>` | あり | 対応 | 対応 | `AsyncRunner::start` で XPC `containerCopyIn` を呼ぶ（start_process 後） / Docker: `copy_to_sources_linux` が create 後・start 前に `PUT /containers/{id}/archive` を自前 ustar で叩く。起動前投入は Linux のみの公開契約 |
| `entrypoint(&self) -> Option<&str>` | あり | 対応 | 対応 | Docker: トレイト定義は OS 共通 |
| `cmd(&self) -> impl IntoIterator<Item = impl Into<Cow<'_, str>>>` | あり | 対応 | 対応 | Docker: トレイト定義は OS 共通 |
| `expose_ports(&self) -> &[ContainerPort]` | あり | 対応 | 対応 | Docker: トレイト定義は OS 共通。未マッピングの expose は create 時に `HostPort=0` の `PortBindings` に載せる。macOS: 事前に空きホストポートを割当 |
| `exec_after_start(&self, cs: ContainerState) -> Result<Vec<ExecCommand>>` | あり | 対応 | 対応 | Docker: トレイト定義は OS 共通 |
| `exec_before_ready(&self, cs: ContainerState) -> Result<Vec<ExecCommand>>` | あり | 対応 | 対応 | Docker: トレイト定義は OS 共通 |

## 2. `ImageExt` トレイト (`core::image::image_ext::ImageExt`)

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `with_cmd(self, cmd)` | あり | 対応 | 対応 |  |
| `with_name(self, name)` | あり | 対応 | 対応 |  |
| `with_tag(self, tag)` | あり | 対応 | 対応 |  |
| `with_container_name(self, name)` | あり | 対応 | 対応 |  |
| `with_platform(self, platform)` | あり | 対応 | 未実装 | `"linux/amd64"` / `"amd64"` で XPC `rosetta`・`imagePull` の `ociPlatform`・`containerCreate` の `platform.architecture` に反映。`"linux/arm64"` / `"arm64"` も architecture / pull に反映。それ以外は無視 / Docker: start 時に明示エラー (設定構築に未配線) |
| `with_network(self, network)` | あり | 部分対応 | 未実装 | XPC `containerCreate` の `networks[0].network` に反映。本家と違いネットワークの自動作成は行わないため、事前に `container network create` で作成が必要。存在しない場合は起動時エラー / Docker: start 時に明示エラー (設定構築に未配線) |
| `with_label(self, k, v)` | あり | 対応 | 対応 |  |
| `with_labels(self, labels)` | あり | 対応 | 対応 |  |
| `with_env_var(self, k, v)` | あり | 対応 | 対応 |  |
| `with_host(self, key, value)` | あり | 部分対応 | 未実装 | `ExtraHost::Addr` はコンテナ起動後に `exec` で `/etc/hosts` へ追記する。`ExtraHost::HostGateway` は起動時に明示エラー / Docker: start 時に明示エラー (設定構築に未配線) |
| `with_hostname(self, hostname)` | あり | 対応 | 未実装 | 明示 hostname → container_name → id の優先で `networks[0].options.hostname` に反映 / Docker: start 時に明示エラー (設定構築に未配線) |
| `with_mount(self, mount)` | あり | 対応 | 部分対応 | Bind/Volume/Tmpfs を XPC の `virtiofs/volume/tmpfs` にマップ / Docker: Bind は HostConfig.Binds に `ro`/`rw` 付きで反映。Volume/Tmpfs は create 時に明示エラー |
| `with_copy_to(self, target, source)` | あり | 対応 | 対応 | シグネチャは一致。コピー処理は XPC `containerCopyIn` で実行されるが、`CopyDataSource::Data` は一時ファイル経由。`mode` はフィールド代入で `fileMode` に反映、`uid` / `gid` は XPC 非反映。投入は start_process 後（起動前契約なし）。親作成は `createParents`。ホストディレクトリの再帰投入可（Apple container 1.1.0 で実測） / Docker: create 後・start 前に自前 ustar で `path=/` へ投入。親ディレクトリ自動作成・ディレクトリ一括投入対応。`mode` / `uid` / `gid` は tar ヘッダ + `copyUIDGID=true` で regular file に反映（中間 directory の mode は `0o755`）。コピー後 mtime は epoch。起動前投入は Linux のみの公開契約 |
| `with_mapped_port(self, host_port, container_port)` | あり | 対応 | 対応 | `publishedPorts` に反映 |
| `with_exposed_host_port(self, port)` (feature) | あり | なし | なし | `host-port-exposure` feature、Rust 側で SSH tunnel 実装が必要 |
| `with_exposed_host_ports(self, ports)` (feature) | あり | なし | なし | 同上 |
| `with_ulimit(self, name, soft, hard)` | あり | なし | なし | XPC `ContainerCfg` には rlimits はある (プロセスごと) が ulimit 全体は無い |
| `with_privileged(self, privileged)` | あり | 部分対応 | 対応 | `true` の場合、XPC `ContainerCfg.capAdd` に `["ALL"]` を設定。Docker の privileged と完全には等価でない（デバイスアクセス等は未対応） |
| `with_cap_add(self, capability)` | あり | 対応 | 未実装 | XPC `capAdd` に反映 / Docker: start 時に明示エラー (設定構築に未配線) |
| `with_cap_drop(self, capability)` | あり | 対応 | 未実装 | XPC `capDrop` に反映 / Docker: start 時に明示エラー (設定構築に未配線) |
| `with_cgroupns_mode(self, mode)` | あり | なし | なし | XPC には該当項目なし |
| `with_userns_mode(self, mode)` | あり | なし | なし | XPC には該当項目なし |
| `with_shm_size(self, bytes)` | あり | 対応 | 未実装 | XPC `ContainerCfg.shmSize` に反映 / Docker: start 時に明示エラー (設定構築に未配線) |
| `with_startup_timeout(self, timeout)` | あり | 対応 | 対応 | `container_req.startup_timeout()` を `AsyncRunner::start` で参照し、`None` の場合は `DEFAULT_STARTUP_TIMEOUT` (60 秒) を使用 |
| `with_working_dir(self, dir)` | あり | 対応 | 対応 | XPC `initProcess.workingDirectory` に反映 |
| `with_log_consumer(self, consumer)` | あり | 対応 | 対応 | macOS: `containerLogs` の FD から行単位で `LogFrame` を配信。Linux: demux 済み共有バッファから行単位で `LogFrame` を配信 (行末 `\n` / `\r` 剥がし、終端後の非改行残余は破棄) |
| `with_host_config_modifier(self, modifier)` | あり | なし | なし | 元の crate が bollard の型を引数に取る API のため、shiguredo では未対応 |
| `with_reuse(self, reuse)` (feature) | あり | なし | なし | feature = `reusable-containers`。shiguredo には型も feature も無し (21 章参照) |
| `with_user(self, user)` | あり | 部分対応 | 対応 | 数値 `uid` / `uid:gid` は XPC `initProcess.user.id` に、名前形式は `user.raw.userString` として渡される。Apple container 実行環境によっては非 root UID が機能しない |
| `with_readonly_rootfs(self, readonly)` | あり | 対応 | 未実装 | XPC トップレベル `readOnly` に反映。個別マウントの AccessMode とは別 / Docker: start 時に明示エラー (設定構築に未配線) |
| `with_security_opt(self, opt)` | あり | なし | なし | XPC には該当項目なし |
| `with_ready_conditions(self, conds)` | あり | 対応 | 対応 | `ContainerRequest::ready_conditions` オーバーライドが有効 (8 章参照) |
| `with_health_check(self, hc)` | あり | 未実装 (XPC 制約) | 対応 | Linux は Config.Healthcheck に配線。macOS は start 時に `Err("with_health_check() is not supported on macOS")` |
| `with_device_requests(self, reqs)` (feature) | あり | なし | なし | feature = `device-requests`。Apple container は GPU/デバイスマッピング未対応 |
| `with_open_stdin(self, open)` | あり | 対応 | 未実装 | XPC `initProcess.terminal` に反映。Docker の OpenStdin と terminal は同義ではない / Docker: start 時に明示エラー (設定構築に未配線) |
| `with_init(self)` | なし | shiguredo 拡張 | 対応 | XPC `useInit` に反映 / Docker: HostConfig.Init として create JSON に反映 |
| `with_ssh(self)` | なし | shiguredo 拡張 | 未実装 | XPC `ssh` に反映 / Docker: start 時に明示エラー (設定構築に未配線) |

## 3. `AsyncRunner` トレイト (`runners::AsyncRunner`)

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `async fn start(self) -> Result<ContainerAsync<I>>` | あり | 対応 | 対応 | XPC `containerCreate` → `containerBootstrap` → `containerStartProcess` → `containerCopyIn`。create / bootstrap / start_process / copy 失敗時の `remove` は Keep 尊重。ログ FD 取得失敗かつ `WaitFor::Log` ありの経路だけは Keep ゲート無しで `remove` し明示エラー (10.1 のログ待機とも関連) / Docker: resolve → pull → create → copy (`PUT /archive`) → start → `container_state` → ready まで完結。logs ストリーム (`?follow=true`) を起動し Log 待機 / `with_log_consumer` に対応。起動失敗時は Log 待機 / consumer 使用なら fail-fast + remove、それ以外は warn + 空リーダー |
| `async fn pull_image(self) -> Result<ContainerRequest<I>>` | あり | 対応 | 対応 | XPC `imagePull` / Docker: DockerClient::pull_image を直接呼ぶ |

## 4. `SyncRunner` トレイト (`runners::SyncRunner`, feature = `blocking`)

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `fn start(self) -> Result<Container<I>>` | あり | 対応 | 対応 | tokio runtime を lazy 生成 / Docker: async と同じ経路で完結 |
| `fn pull_image(self) -> Result<ContainerRequest<I>>` | あり | 対応 | 対応 | Docker: DockerClient::pull_image を直接呼ぶ |

## 5. `AsyncBuilder` / `SyncBuilder` トレイト (`runners`)

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `AsyncBuilder::build_image` | あり | なし | なし | 現時点では未対応。方針は 20 章参照 |
| `AsyncBuilder::build_image_with` | あり | なし | なし | 同上 |
| `SyncBuilder::build_image` (feature blocking) | あり | なし | なし | 同上 |
| `SyncBuilder::build_image_with` (feature blocking) | あり | なし | なし | 同上 |
| `GenericBuildableImage::new/with_dockerfile/with_dockerfile_string/with_file/with_data` | あり | なし | なし | 20 章参照 |
| `BuildableImage` トレイト | あり | なし | なし | 20 章参照 |
| `BuildContextBuilder::default/with_dockerfile/with_dockerfile_string/with_file/with_data/collect/as_copy_to_container_collection` | あり | なし | なし | 20 章参照 |
| `BuildImageOptions::new/with_skip_if_exists/with_no_cache/with_build_arg/with_build_args` | あり | なし | なし | 20 章参照 |

## 6. `ContainerAsync<I>` のメソッド (`core::containers::async_container::ContainerAsync`)

本家では `impl Deref for ContainerAsync<I> { type Target = RawContainer; ... }` により、RawContainer のメソッドも透過的に呼べる。本クレートは `Deref` せず、必要メソッドを `ContainerAsync` に直接載せる方式。以下は本家の `ContainerAsync` + `RawContainer` を合わせた比較。

### 6.1 ContainerAsync の直接メソッド

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `image(&self) -> &I` | あり | 対応 | 対応 |  |
| `async fn start(&self) -> Result<()>` | あり | 対応 | 対応 | 停止済みなら Docker `start`。macOS は bootstrap + start_process。`exec_after_start` を実行 / Docker: start_container 配線済み |
| `async fn stop_with_timeout(&self, secs: Option<i32>) -> Result<()>` | あり | 対応 | 対応 | macOS: `Some(0)` は即時 SIGKILL、`Some(t)` (`t < 0`) は長時間 SIGTERM、`None` は 30 秒 SIGTERM / Docker: `None`・負値は `t=30`、`Some(t>=0)` は `t={t}`。404 は冪等成功 |
| `async fn pause(&self) -> Result<()>` | あり | なし | なし | XPCRoute に `containerPause` なし。stub で常にエラーを返す方針は採らず、シグネチャ自体を削除済み / Docker: Docker Engine には pause API はあるが未導入 |
| `async fn unpause(&self) -> Result<()>` | あり | なし | なし | 同上 / Docker: 同上 |
| `async fn is_running(&self) -> Result<bool>` | あり | 対応 | 対応 | `XpcClient::container_state` の `running` を返す / Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `async fn container_state(&self) -> Result<ContainerState>` | なし | shiguredo 拡張 | 対応 | XPC `containerState`。本家 0.27 に無し。`ContainerState::from_container` は本メソッドへ委譲 / Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `async fn exit_code(&self) -> Result<Option<i64>>` | あり | 部分対応 | 部分対応 | バックグラウンド wait の観測済みキャッシュのみ。未観測のときは停止後も `None` (都度 `containerWait` はしない) / Docker: バックグラウンド wait スレッド (`POST /containers/{id}/wait?condition=not-running`) の観測済みキャッシュのみ。未観測のときは停止後も `None` |
| `async fn copy_file_from<T>(&self, path, target: T) -> Result<T::Output>` | あり | 対応 | 対応 | XPC `containerCopyOut` でホスト上の一時ファイルに書き出し、`CopyFileFromContainer` に流し込む / Docker: `GET /containers/{id}/archive` の tar を自前 ustar パーサで展開し先頭 regular file を渡す。ディレクトリは `IsDirectory`、source は絶対パス必須 |
| `async fn rm(mut self) -> Result<()>` | あり | 対応 | 対応 | XPC `containerDelete` / Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `fn rm_blocking(mut self) -> Result<()>` | なし | shiguredo 拡張 | shiguredo 拡張 | `block_on` を使わず `remove_blocking` (同期 I/O) を直接呼び出す。tokio Runtime 内の同期コンテキスト (Drop ガードや `spawn_blocking` 内) から呼んでも deadlock しない。`force=true`、404 は冪等成功、`keep` でも削除する |

### 6.2 RawContainer のメソッド (本家では Deref 経由、shiguredo では ContainerAsync に直接)

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `id(&self) -> &str` | あり | 対応 | 対応 |  |
| `async fn ports(&self) -> Result<Ports>` | あり | 対応 | 対応 | `XpcClient::container_state` が `containerList` の `configuration.publishedPorts` をパースして返す / Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `async fn get_host_port_ipv4(&self, port) -> Result<u16>` | あり | 対応 | 対応 | `self.ports()` から `map_to_host_port_ipv4` で解決 / Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `async fn get_host_port_ipv6(&self, port) -> Result<u16>` | あり | 対応 | 対応 | 同上（IPv6 マッピング側） / Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `async fn get_bridge_ip_address(&self) -> Result<IpAddr>` | あり | 対応 | 対応 | `containerList` の `networks[0].ipv4Address` (CIDRv4) から抽出 / Docker: inspect の `NetworkSettings.Networks` 先頭エントリの `IPAddress` から取得 |
| `async fn get_host(&self) -> Result<Host>` | あり | 部分対応 | 部分対応 | macOS の Apple container は基本的にホスト側からの接続で `127.0.0.1` (`localhost`) が正しい。`Host` は `shiguredo_container::core::host::Host` / Docker: localhost 固定 |
| `async fn exec(&self, cmd: ExecCommand) -> Result<ExecResult>` | あり | 部分対応 | 対応 | XPC は stdout/stderr/Env 付き。Docker は stdout/stderr/Env 付き (AttachStdout/AttachStderr + multiplexed stream demux。Env はコンテナ env を inspect で取得し exec 分で上書きマージ) |
| `async fn start(&self) -> Result<()>` | あり | 対応 | 対応 | 停止済みなら Docker `start`。macOS は bootstrap + start_process。`exec_after_start` を実行 / Docker: start_container 配線済み |
| `async fn stop(&self) -> Result<()>` | あり | 対応 | 対応 | `stop_with_timeout(None)` のエイリアス。timeout 30 秒固定 / Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `async fn stop_with_timeout(&self, secs: Option<i32>) -> Result<()>` | あり | 対応 | 対応 | macOS: `Some(0)` は即時 SIGKILL、`Some(t)` (`t < 0`) は長時間 SIGTERM、`None` は 30 秒 SIGTERM / Docker: `None`・負値は `t=30`、`Some(t>=0)` は `t={t}`。404 は冪等成功 |
| `fn stdout(&self, follow: bool) -> Pin<Box<dyn AsyncBufRead + Send>>` | あり | 対応 | 対応 | `containerLogs` から取得した stdout FD を非同期に読む。`follow=true` は追記ポーリング (init 終了 / Drop で EOF) / Docker: demux 済み共有バッファを独立オフセットで読む。`follow=true` は 8 MiB リング (上限超過で先頭 drop、`warn` ログのみ)、`follow=false` は呼び出しごとに新規 HTTP セッション |
| `fn stderr(&self, follow: bool) -> Pin<Box<dyn AsyncBufRead + Send>>` | あり | 対応 | 対応 | `containerLogs` から取得した stderr FD を非同期に読む。`follow=true` は追記ポーリング (init 終了 / Drop で EOF)。Apple の 2 本目 FD は bootlog であり、アプリの stderr は stdout 側に混流する。このため macOS で stderr メッセージ待機 (`WaitFor::message_on_stderr` 等) を使うと起動待ちがタイムアウトする / Docker: demux が STREAM_TYPE で分離するため本当に stderr のみ。8 MiB リング (上限超過で先頭 drop) |
| `async fn stdout_to_vec(&self) -> Result<Vec<u8>>` | あり | 対応 | 対応 | `stdout` リーダーから全文読み出す / Docker: `?follow=false&tail=all` の 1-shot 取得で全ログを読み切る |
| `async fn stderr_to_vec(&self) -> Result<Vec<u8>>` | あり | 対応 | 対応 | `stderr` リーダーから全文読み出す / Docker: `?follow=false&tail=all` の 1-shot 取得で全ログを読み切る |
| `Drop` impl | あり | 部分対応 | 対応 | Runtime 内は専用 std スレッドで `remove_blocking` を実行し `DROP_REMOVE_TIMEOUT` (5 秒) を上限に完了を待つ (timeout 超過時は best-effort、削除スレッドは裏で継続)、Runtime 外は呼び出しスレッドで `remove_blocking` を同期実行 (試行終了まで待つが成功は非保証)。いずれも失敗は `tracing::error` のみで呼び出し側には届かない。常に `force=true`、404 は冪等成功。Keep ゲートは Drop のみで、明示 `rm` / `rm_blocking` は Keep でも削除する |

## 7. `Container<I>` (sync 版, feature = `blocking`)

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `id(&self) -> &str` | あり | 対応 | 対応 |  |
| `image(&self) -> &I` | あり | 対応 | 対応 |  |
| `ports(&self) -> Result<Ports>` | あり | 対応 | 対応 | `ContainerAsync::ports` に委譲 (6.2 参照) / Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `get_host_port_ipv4(&self, port) -> Result<u16>` | あり | 対応 | 対応 | 同上 / Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `get_host_port_ipv6(&self, port) -> Result<u16>` | あり | 対応 | 対応 | 同上 / Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `get_bridge_ip_address(&self) -> Result<IpAddr>` | あり | 対応 | 対応 | `ContainerAsync::get_bridge_ip_address` に委譲 / Docker: inspect の `NetworkSettings.Networks` 先頭エントリの `IPAddress` から取得 |
| `get_host(&self) -> Result<Host>` | あり | 対応 | 部分対応 | `ContainerAsync::get_host` に委譲 (macOS では `localhost` 固定) / Docker: localhost 固定 |
| `exec(&self, cmd: ExecCommand) -> Result<SyncExecResult>` | あり | 部分対応 | 対応 | `ContainerAsync::exec` に委譲。XPC は stdout/stderr/Env 付き。Docker は stdout/stderr/Env 付き |
| `copy_file_from<T>(&self, path, target: T) -> Result<T::Output>` | あり | 対応 | 対応 | `ContainerAsync::copy_file_from` に委譲 / Docker: `ContainerAsync` 経由で Docker archive API を利用 |
| `stop(&self) -> Result<()>` | あり | 対応 | 対応 | `stop_with_timeout(None)` のエイリアス / Docker: stop_with_timeout 対応に依存 |
| `stop_with_timeout(&self, secs: Option<i32>) -> Result<()>` | あり | 対応 | 対応 | `ContainerAsync::stop_with_timeout` に委譲 / Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `start(&self) -> Result<()>` | あり | 対応 | 対応 | `ContainerAsync::start` に委譲 / Docker: container_state 対応に依存 |
| `pause(&self) -> Result<()>` (async 宣言だが sync impl) | あり | なし | なし | XPCRoute に pause 系が無いためシグネチャ自体を削除済み / Docker: Docker Engine には pause API はあるが未導入 |
| `unpause(&self) -> Result<()>` | あり | なし | なし | 同上 / Docker: 同上 |
| `rm(mut self) -> Result<()>` | あり | 対応 | 対応 | Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `rm_blocking(mut self) -> Result<()>` | なし | shiguredo 拡張 | shiguredo 拡張 | `ContainerAsync::rm_blocking` に委譲。`block_on` を使わないため Runtime 内の同期コンテキストから呼んでも deadlock しない |
| `stdout(&self, follow) -> Box<dyn BufRead + Send>` | あり | 対応 | 対応 | ContainerAsync の同期リーダーへ委譲。`follow=true` は追記ポーリング (呼び出しスレッドをブロック) / Docker: 共有バッファを `park_timeout(50ms)` 周期起床で読む。`follow=false` は 1-shot 取得 |
| `stderr(&self, follow) -> Box<dyn BufRead + Send>` | あり | 対応 | 対応 | ContainerAsync の同期リーダーへ委譲。`follow=true` は追記ポーリング (呼び出しスレッドをブロック) / Docker: 共有バッファを `park_timeout(50ms)` 周期起床で読む。`follow=false` は 1-shot 取得 |
| `stdout_to_vec(&self) -> Result<Vec<u8>>` | あり | 対応 | 対応 | `ContainerAsync::stdout_to_vec` に委譲 / Docker: 1-shot 取得 |
| `stderr_to_vec(&self) -> Result<Vec<u8>>` | あり | 対応 | 対応 | `ContainerAsync::stderr_to_vec` に委譲 / Docker: 1-shot 取得 |
| `is_running(&self) -> Result<bool>` | あり | 対応 | 対応 | `ContainerAsync::is_running` に委譲 / Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `container_state(&self) -> Result<ContainerState>` | なし | shiguredo 拡張 | 対応 | `ContainerAsync::container_state` に委譲。本家 0.27 に無し / Docker: ContainerAsync の Linux 分岐から DockerClient を呼び出し |
| `exit_code(&self) -> Result<Option<i64>>` | あり | 部分対応 | 部分対応 | `ContainerAsync::exit_code` に委譲。6.1 委譲・制約同じ (観測済みキャッシュのみ) / Docker: バックグラウンド wait スレッドの観測済みキャッシュのみ |

## 8. `ContainerRequest<I>` のメソッド (`core::containers::request`)

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `image(&self) -> &I` | あり | 対応 | 対応 |  |
| `network(&self) -> &Option<String>` | あり | 対応 | 対応 |  |
| `labels(&self) -> &BTreeMap<String, String>` | あり | 対応 | 対応 |  |
| `container_name(&self) -> &Option<String>` | あり | 対応 | 対応 |  |
| `platform(&self) -> &Option<String>` | あり | 対応 | 対応 | ImageExt::with_platform とセット |
| `env_vars(&self) -> impl Iterator<...>` | あり | 対応 | 対応 | Image 側と ContainerRequest 側の chain |
| `hosts(&self) -> impl Iterator<...>` | あり | 対応 | 対応 |  |
| `mounts(&self) -> impl Iterator<...>` | あり | 対応 | 対応 |  |
| `copy_to_sources(&self) -> impl Iterator<...>` | あり | 対応 | 対応 |  |
| `ports(&self) -> Option<&Vec<PortMapping>>` | あり | 対応 | 対応 | リクエスト上のマッピング一覧。実行中コンテナの `ContainerAsync::ports` とは別 |
| `host_port_exposures(&self)` (feature) | あり | なし | なし | feature = `host-port-exposure` |
| `privileged(&self) -> bool` | あり | 対応 | 対応 |  |
| `cap_add(&self) -> Option<&Vec<String>>` | あり | 対応 | 対応 |  |
| `cap_drop(&self) -> Option<&Vec<String>>` | あり | 対応 | 対応 |  |
| `cgroupns_mode(&self) -> Option<CgroupnsMode>` | あり | なし | なし | with_cgroupns_mode とセット |
| `userns_mode(&self) -> Option<&str>` | あり | なし | なし | with_userns_mode とセット |
| `shm_size(&self) -> Option<u64>` | あり | 対応 | 対応 | XPC `ContainerCfg.shmSize` に反映 (2 章参照) |
| `entrypoint(&self) -> Option<&str>` | あり | 対応 | 対応 | Image に委譲 |
| `cmd(&self) -> impl Iterator<...>` | あり | 対応 | 対応 | `either` 依存を回避した実装 |
| `descriptor(&self) -> String` | あり | 対応 | 対応 | `name:tag` |
| `ready_conditions(&self) -> Vec<WaitFor>` | あり | 対応 | 対応 | `ContainerRequest::ready_conditions` オーバーライドが有効 (2 章参照) |
| `expose_ports(&self) -> &[ContainerPort]` | あり | 対応 | 対応 | Docker: トレイト定義は OS 共通。未マッピングの expose は create 時に `HostPort=0` の `PortBindings` に載せる。macOS: 事前に空きホストポートを割当 |
| `exec_after_start(&self, cs) -> Result<Vec<ExecCommand>>` | あり | 対応 | 対応 |  |
| `startup_timeout(&self) -> Option<Duration>` | あり | 対応 | 対応 | `AsyncRunner::start` で `DEFAULT_STARTUP_TIMEOUT` の代替として使用 |
| `working_dir(&self) -> Option<&str>` | あり | 対応 | 対応 |  |
| `reuse(&self) -> ReuseDirective` (feature) | あり | なし | なし | feature = `reusable-containers` |
| `user(&self) -> Option<&str>` | あり | 対応 | 対応 |  |
| `security_opts(&self) -> Option<&Vec<String>>` | あり | なし | なし |  |
| `readonly_rootfs(&self) -> bool` | あり | 対応 | 対応 |  |
| `hostname(&self) -> Option<&str>` | あり | 対応 | 対応 |  |
| `health_check(&self) -> Option<&Healthcheck>` | あり | 対応 | 対応 | 16 章参照 |
| `host_config_modifier(&self) -> Option<&HostConfigModifier>` | あり | なし | なし | Docker Engine API の低レベル型に依存するため未対応 |
| `device_requests(&self)` (feature) | あり | なし | なし | feature = `device-requests` |
| `open_stdin(&self) -> Option<bool>` | あり | 対応 | 対応 |  |
| `impl<I: Image> From<I> for ContainerRequest<I>` | あり | 対応 | 対応 |  |
| `init/ssh` accessor | なし | shiguredo 拡張 | 部分対応 | `init()` は Docker HostConfig.Init に配線済み。`ssh()` は Linux の Docker 設定構築で未反映 |
| `PortMapping::new (crate内)` | あり | 対応 | 対応 |  |
| `PortMapping::host_port` | あり | 対応 | 対応 |  |
| `PortMapping::container_port` | あり | 対応 | 対応 |  |
| `Host::Addr(IpAddr)` | あり | 対応 | 対応 | `core::host::Host` |
| `Host::Domain(String)` | あり | 対応 | 対応 | IP でなければ Domain |
| `Host` Display | あり | 対応 | 対応 |  |
| `CgroupnsMode::Host / Private` | あり | なし | なし | 型のみ定義。setter / 配線は無し (`with_cgroupns_mode` は「なし」) |

## 9. `WaitFor` バリアント + コンストラクタ

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `WaitFor::Nothing` | あり | 対応 | 対応 |  |
| `WaitFor::Log(LogWaitStrategy)` | あり | 対応 | 対応 | 動作は 10.1 参照 / Docker: logs ストリーム (demux + 共有バッファ) で成立。EOF は demux 終端 (`logs_terminated`) で判定 |
| `WaitFor::Duration { length }` | あり | 対応 | 対応 |  |
| `WaitFor::Healthcheck(HealthWaitStrategy)` | あり | 未実装 (XPC 制約) | 対応 | 動作は 10.2 参照 / Linux は Healthy/Unhealthy/Starting/None (running 後) の 4 分岐 |
| `WaitFor::Http(Box<HttpWaitStrategy>)` (feature) | あり | 対応 | 部分対応 | feature = `http_wait_plain` / Docker: ports() 配線済みで host port 解決は可能。Log 待機との併用は未対応 |
| `WaitFor::Exit(ExitWaitStrategy)` | あり | 対応 | 未実装 | Docker: Linux では即時未実装エラー |
| `pub fn message_on_stdout(msg)` | あり | 対応 | 対応 | Docker: demux が stdout を分離するため本当に stdout のみに反応 |
| `pub fn message_on_stderr(msg)` | あり | 対応 | 対応 | macOS: stderr FD は VM の bootlog を指すためアプリの stderr メッセージは成立せず、`startup_timeout` でタイムアウトする。代わりに `message_on_stdout` / `message_on_either_std` を使うこと (アプリの stderr は stdout 側ログに混流する) / Docker: demux が stderr を分離するため本当に stderr のみに反応 |
| `pub fn message_on_either_std(msg)` | あり | 対応 | 対応 | Docker: stdout / stderr 両ストリームを並行照合 |
| `pub fn log(strategy)` | あり | 対応 | 対応 | Docker: logs ストリームで成立 |
| `pub fn healthcheck() -> WaitFor` | あり | 未実装 (XPC 制約) | 対応 | Docker: Linux は inspect ポーリング、macOS は with_health_check で即エラー |
| `pub fn http(strategy)` (feature) | あり | 対応 | 部分対応 | feature = `http_wait_plain` / Docker: ports() 配線済みで host port 解決は可能 |
| `pub fn exit(strategy)` | あり | 対応 | 未実装 | Docker: ExitWaitStrategy 自体が Linux 未実装のため、生成しても待機時にエラー |
| `pub fn seconds(len)` | あり | 対応 | 対応 |  |
| `pub fn millis(len)` | あり | 対応 | 対応 |  |
| `pub fn millis_in_env_var(name)` | あり | 対応 | 対応 |  |
| `impl From<HttpWaitStrategy> for WaitFor` (feature) | あり | 対応 | 対応 | feature = `http_wait_plain`。`WaitFor::Http(Box::new(...))` に変換 |

## 10. Wait strategy 型

### 10.1 `LogWaitStrategy`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub fn stdout(msg)` | あり | 対応 | 対応 | Docker: demux 済み stdout 共有バッファを照合 |
| `pub fn stderr(msg)` | あり | 対応 | 対応 | macOS: 待機対象は VM bootlog のため成立せず起動待ちがタイムアウトする (stdout 側に混流。10.1 参照) / Docker: demux 済み stderr 共有バッファを照合 |
| `pub fn stdout_or_stderr(msg)` | あり | 対応 | 対応 | Docker: stdout / stderr 両共有バッファを並行照合 |
| `pub fn new(source, msg)` | あり | 対応 | 対応 |  |
| `pub fn with_times(mut self, n)` | あり | 対応 | 対応 |  |
| `wait_until_ready` impl | あり | 部分対応 | 対応 | チャンク境界・非 UTF-8 対応。macOS の stderr 側は VM bootlog を指すため、`LogSource::StdErr` 待機 (アプリの stderr は stdout 側ログに混流する) は成立せず `startup_timeout` でタイムアウトする。`StdOut` / `BothStd` を使うこと / Docker: demux が STREAM_TYPE で分離するため stderr は本当に stderr のみ。EOF は demux 終端 (`logs_terminated`) → DRAIN_GRACE → `EndOfStream` で判定 |

### 10.2 `HealthWaitStrategy`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub fn new()` | あり | 対応 | 対応 |  |
| `pub fn with_poll_interval(mut self, d)` | あり | 対応 | 対応 | Linux では inspect のポーリング間隔として利用 |
| `impl Default` | あり | 対応 | 対応 |  |
| `wait_until_ready` impl | あり | 未実装 (XPC 制約) | 対応 | Apple container は Docker HEALTHCHECK を実行しない。実装不可 / Docker: Linux は inspect ポーリング (`starting` / `healthy` / `unhealthy` / `Health` 不在は running 後 `HealthCheckNotConfigured`)、macOS は `HealthCheckNotConfigured` 維持 |

### 10.3 `HttpWaitStrategy` (feature = `http_wait_plain`)

shiguredo は reqwest ではなく `shiguredo_http11` + `tokio::net::TcpStream` で実装している (plain HTTP のみ、TLS 非対応)。`new` / `with_port` / `with_header` / `with_body` / `with_basic_auth` / `with_bearer_auth` / `with_poll_interval` / `with_expected_status_code` は本家同等。reqwest の型を受け取る API は shiguredo 独自の型に置き換えている: `with_method` は文字列、`with_response_matcher` は `Fn(&HttpResponse) -> bool` (受信済みレスポンスを渡すため async matcher は不要)。`with_client` / `with_tls` / `with_response_matcher_async` は無い。エラーは `WaitContainerError::HttpWait(HttpWaitError)` (feature ゲート付きの型付きエラー、本家と同じ構造)。

> **macOS の制約 (Local Network Privacy)**: `HttpWaitStrategy` は published port (`localhost:hostPort`) に接続するため、Apple container のポートフォワーダー (`container-runtime-linux`) を経由する。macOS 15+ の Local Network Privacy (LNP) は、許可されていないユーザープロセスからローカルネットワークへの接続をブロックし得る。self-hosted CI ではコンテナ IP 直結の HTTP 疎通テストを実行し、published port / `HttpWaitStrategy` 依存のテストは `RUN_HOST_NETWORK_TESTS=1` を指定した LNP 許可済み環境でのみ実行する (ローカルでも直結テストは同ゲートで制御する)。LNP は TCC / MDM で事前付与できず、root で動く launchd daemon のみ対象外となる。

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub struct HttpWaitStrategy` | あり | 対応 | 部分対応 | Docker: ports() 配線済みで host port 解決は可能 |
| `HttpWaitError` | あり | 部分対応 (意図的) | 部分対応 (意図的) | shiguredo は URL パースをしないため `InvalidUrl` は不要。matcher 未設定は本家の実行時 other エラーに対し型付き `NoResponseMatcher` |
| `pub fn new(path)` | あり | 対応 | 対応 |  |
| `pub fn with_port(mut, port)` | あり | 対応 | 対応 |  |
| `pub fn with_client(mut, client)` | あり | なし | なし | reqwest 非依存のため対象外 (方針) |
| `pub fn with_method(mut, m)` | あり | 部分対応 (意図的) | 部分対応 (意図的) | 引数型が独自 |
| `pub fn with_header(mut, k, v)` | あり | 対応 | 対応 | 引数は文字列ペア |
| `pub fn with_body(mut, b)` | あり | 対応 | 対応 |  |
| `pub fn with_basic_auth(mut, u, p)` | あり | 対応 | 対応 | base64 は base64ct |
| `pub fn with_bearer_auth(mut, t)` | あり | 対応 | 対応 |  |
| `pub fn with_tls(mut)` | あり | なし | なし | TLS 非対応 (方針) |
| `pub fn with_poll_interval(mut, d)` | あり | 対応 | 対応 |  |
| `pub fn with_expected_status_code(mut, s)` | あり | 対応 | 対応 |  |
| `pub fn with_response_matcher(mut, f)` | あり | 部分対応 (意図的) | 部分対応 (意図的) | 受信済みレスポンスの独自型を渡す |
| `pub fn with_response_matcher_async(mut, f)` | あり | なし | なし | body 受信済みのため同期 matcher で足りる (方針) |
| `wait_until_ready` impl | あり | 対応 | 対応 | ポート解決 → TCP 接続 → 照合をポーリング。`Connection: close` 送信 |

### 10.4 `ExitWaitStrategy`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub fn new()` | あり | 対応 | 対応 |  |
| `pub fn with_poll_interval(mut, d)` | あり | 対応 | 対応 |  |
| `pub fn with_exit_code(mut, c)` | あり | 対応 | 対応 | Docker: コンストラクタは動作するが、wait_until_ready が未実装のため設定しても効果なし |
| `impl Default` | あり | 対応 | 対応 |  |
| `wait_until_ready` impl | あり | 対応 | 未実装 | macOS: バックグラウンド `containerWait` の観測値で exit code を判定。停止の検出は `containerList` のポーリング / Docker: Linux では即時未実装エラー |

### 10.5 `WaitStrategy` (`pub(crate)` trait) — 廃止済み

本家 0.27 には `pub(crate) trait WaitStrategy` があるが、shiguredo では廃止済み。各戦略型 (`LogWaitStrategy` / `HealthWaitStrategy` / `HttpWaitStrategy` / `ExitWaitStrategy`) が直接 `pub(crate) async fn wait_until_ready` を持つインherent メソッド方式に変更済み。`WaitFor::wait_until_ready` が各戦略のメソッドを直接呼び出す。

## 11. `LogConsumer` / `LoggingConsumer` / `LogFrame` / `LogSource`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `LogFrame::StdOut(Bytes)` | あり | 対応 | 対応 | Docker: demux の STREAM_TYPE=1 フレームから生成 |
| `LogFrame::StdErr(Bytes)` | あり | 対応 | 対応 | Docker: demux の STREAM_TYPE=2 フレームから生成 |
| `LogFrame::source(&self) -> LogSource` | あり | 対応 | 対応 |  |
| `LogFrame::bytes(&self) -> &Bytes` | あり | 対応 | 対応 |  |
| `LogSource::StdOut` | あり | 対応 | 対応 | Docker: demux 済み stdout 共有バッファを照合 |
| `LogSource::StdErr` | あり | 対応 | 対応 | Docker: demux 済み stderr 共有バッファを照合 |
| `LogSource::BothStd` | あり | 対応 | 対応 | 待機戦略では stdout / stderr を並行照合。出現回数は両ストリーム合算 |
| `LogSource::includes_stdout()` (`pub(super)`) | あり | なし | なし | 内部メソッド |
| `LogSource::includes_stderr()` (`pub(super)`) | あり | なし | なし | 内部メソッド |
| `WaitLogError::EndOfStream(Vec<Bytes>)` | あり | 部分対応 | 部分対応 | payload は上限 (1 MiB) 内の直近ログ・連結済み (要素 0 または 1)。要素型は本家 `Bytes` / shiguredo `Vec<u8>` (意図的差分) |
| `WaitLogError::Io(io::Error)` | あり | 対応 | 対応 |  |
| `LogConsumer::accept(&self, &LogFrame) -> BoxFuture<'a, ()>` | あり | 対応 | 対応 |  |
| `impl<F: Fn(&LogFrame)> LogConsumer for F` | あり | 対応 | 対応 |  |
| `LoggingConsumer::new()` | あり | 対応 | 対応 | 既定は両ストリームとも `eprintln!` |
| `LoggingConsumer::with_stdout_level(mut, level)` | あり | 対応 | 対応 | 引数は `tracing::Level` (本家は `log::Level`)。指定時は tracing へ |
| `LoggingConsumer::with_stderr_level(mut, level)` | あり | 対応 | 対応 | 同上 |
| `LoggingConsumer::with_prefix(mut, prefix)` | あり | 対応 | 対応 | メッセージ先頭に接頭辞を付与 |
| `impl Default for LoggingConsumer` | あり | 対応 | 対応 |  |
| `impl LogConsumer for LoggingConsumer` | あり | 部分対応 | 部分対応 | 既定は `eprintln!`、レベル指定時は tracing。本家は常に `log` |
| `WaitingStreamWrapper` (`pub(crate)`) | あり | 内部型なので対象外 | 内部型なので対象外 | 本家のログストリームラッパ。shiguredo は `ContainerAsync` の `containerLogs` FD / `FdReader` 読み取りを使う |
| `LogStream` (`pub(crate)`) | あり | 内部型なので対象外 | 内部型なので対象外 |  |
| `RawLogStream` (`pub(crate)`) | あり | 内部型なので対象外 | 内部型なので対象外 | 本家の内部ログストリーム型。shiguredo には無し。ログは `ContainerAsync` の `containerLogs` FD / `FdReader` 読み取り |

## 12. `ExecCommand` / `ExecResult`

### 12.1 `ExecCommand`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub fn new(cmd: impl IntoIterator<Item = impl Into<String>>) -> Self` | あり | 対応 | 対応 |  |
| `pub fn with_container_ready_conditions(mut, Vec<WaitFor>)` | あり | 対応 | 対応 |  |
| `pub fn with_cmd_ready_condition(mut, impl Into<CmdWaitFor>)` | あり | 対応 | 対応 |  |
| `pub fn with_env_vars(mut, iter)` | あり | 対応 | 対応 | コンテナ env にマージして XPC `ProcessConfiguration.environment` へ送る。同名キーは ExecCommand 側が優先 / Docker: コンテナ env を inspect で取得し exec 分で上書きマージして `ExecConfig.Env` に設定。空ならコンテナ env を継承 |
| `impl Default` | あり | 対応 | 対応 |  |

### 12.2 `ExecResult`

本家と同一シグネチャ。本家はログストリームを追いかけるリーダーだが、shiguredo は exec 完了時点の全出力を保持したバッファ上のリーダーを返す (読み出しは本家と同様に消費型)。

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub async fn exit_code(&self) -> Result<Option<i64>>` | あり | 対応 | 対応 | `XpcClient::exec` が `containerWait` の `exitCode` を取得済み / Docker: ストリーム EOF 後の inspect で ExitCode を取得 |
| `pub fn stdout<'b>(&'b mut self) -> Pin<Box<dyn AsyncBufRead + Send + 'b>>` | あり | 対応 | 対応 | `XpcClient::exec` で pipe FD から取得した stdout のバッファ済みリーダー / Docker: multiplexed stream を demux した stdout バッファのリーダー |
| `pub fn stderr<'b>(&'b mut self) -> Pin<Box<dyn AsyncBufRead + Send + 'b>>` | あり | 対応 | 対応 | 同上 (stderr) / Docker: multiplexed stream を demux した stderr バッファのリーダー |
| `pub async fn stdout_to_vec(&mut self) -> Result<Vec<u8>>` | あり | 対応 | 対応 | Docker: demux 済み stdout バッファを返す |
| `pub async fn stderr_to_vec(&mut self) -> Result<Vec<u8>>` | あり | 対応 | 対応 | Docker: demux 済み stderr バッファを返す |
| `impl Debug` | あり | 対応 | 対応 |  |

### 12.3 `CmdWaitFor` (`core::wait::cmd_wait`)

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `CmdWaitFor::Nothing` | あり | 対応 | 対応 |  |
| `CmdWaitFor::StdOutMessage { message: Bytes }` | あり | 対応 | 対応 | exec が取得した stdout への部分一致で判定 |
| `CmdWaitFor::StdErrMessage { message: Bytes }` | あり | 対応 | 対応 | exec が取得した stderr への部分一致で判定 |
| `CmdWaitFor::Duration { length }` | あり | 対応 | 対応 |  |
| `CmdWaitFor::Exit { code: Option<i64> }` | あり | 対応 | 対応 | `code: None` なら終了だけ待ち、`Some(N)` なら終了コードの一致も検証 |
| `pub fn message_on_stdout(msg)` | あり | 対応 | 対応 | exec が取得した stdout への部分一致で判定 |
| `pub fn message_on_stderr(msg)` | あり | 対応 | 対応 | exec が取得した stderr への部分一致で判定 |
| `pub fn exit() -> Self` | あり | 対応 | 対応 |  |
| `pub fn exit_code(code: i64) -> Self` | あり | 対応 | 対応 | Docker: `CmdWaitFor::Exit` として Linux exec でも利用可 |
| `pub fn duration(d)` | あり | なし | なし | seconds / millis しかない |
| `pub fn seconds(secs)` | あり | 対応 | 対応 |  |
| `pub fn millis(ms)` | あり | 対応 | 対応 |  |

### 12.4 `SyncExecResult` (feature = `blocking`)

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub struct SyncExecResult` | あり | 対応 | 対応 | async 版 `ExecResult` を包む同期ラッパー |
| `pub fn exit_code(&self) -> Result<Option<i64>, _>` | あり | 対応 | 対応 | Docker: exec 結果の ExitCode を返す |
| `pub fn stdout / stderr / stdout_to_vec / stderr_to_vec` | あり | 対応 | 対応 | `stdout` / `stderr` は `Box<dyn BufRead + Send>` / Docker: async 版 `ExecResult` の demux 済みバッファに委譲 |

## 13. Copy 関連

### 13.1 `CopyDataSource`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `CopyDataSource::File(PathBuf)` | あり | 対応 | 対応 |  |
| `CopyDataSource::Data(Vec<u8>)` | あり | 対応 | 対応 |  |
| `impl From<&Path> for CopyDataSource` | あり | なし | なし |  |
| `impl From<PathBuf> for CopyDataSource` | あり | 対応 | 対応 |  |
| `impl From<Vec<u8>> for CopyDataSource` | あり | 対応 | 対応 |  |
| `pub(crate) async fn append_tar(...)` | あり | なし | なし | コピーは `AsyncRunner::start` の `containerCopyIn` で実装済み。tar 経路のみ未実装 |

### 13.2 `CopyToContainer`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub fn new(source: impl Into<CopyDataSource>, target: impl Into<CopyTargetOptions>)` | あり | 対応 | 対応 | シグネチャ一致 |
| `pub(crate) async fn tar()` | あり | なし | なし | 未実装 |
| `pub(crate) async fn append_tar(...)` | あり | なし | なし | コピーは `containerCopyIn` 済み。tar 経路のみ未実装 |

### 13.3 `CopyToContainerCollection`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `CopyToContainerCollection` 型 | あり | なし | なし |  |
| `pub fn new(collection)` | あり | なし | なし |  |
| `pub fn add(&mut, entry)` | あり | なし | なし |  |
| `pub(crate) async fn tar()` | あり | なし | なし |  |

### 13.4 `CopyTargetOptions`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub struct CopyTargetOptions` | あり | 対応 | 対応 | フィールドは public `path` (本家 private `target`) + `mode: u32` (本家 `Option<u32>`、本家は `with_mode` で設定) + `uid` / `gid` (XPC 非反映) |
| `pub fn new(target)` | あり | 対応 | 対応 | デフォルト `mode` 0o644 |
| `pub fn with_mode(mut, mode)` | あり | 対応 | 対応 | `mode: u32` フィールドを更新。`uid` / `gid` は XPC 非反映のまま |
| `pub fn target(&self) -> &str` | あり | なし | なし | フィールド `path` で参照 |
| `pub fn mode(&self) -> Option<u32>` | あり | 対応 | 対応 | 常に `Some(self.mode)` (既定 0o644 含む) |
| `impl<T: Into<String>> From<T> for CopyTargetOptions` | あり | なし | なし | 本家専用の blanket。shiguredo には無い |
| `impl From<String> for CopyTargetOptions` | あり | 対応 | 対応 |  |
| `impl From<&str> for CopyTargetOptions` | あり | 対応 | 対応 |  |

### 13.5 `CopyFileFromContainer`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub trait CopyFileFromContainer { type Output; async fn copy_from_reader<R>(self, reader: R) -> Result<Self::Output, CopyFromContainerError>; }` | あり | 部分対応 | 部分対応 | shiguredo は `async_trait` を使わず生の Future を返す。GAT 相当。使う側の互換性はあり |
| `impl CopyFileFromContainer for Vec<u8>` | あり | 対応 | 対応 |  |
| `impl CopyFileFromContainer for &mut Vec<u8>` | あり | なし | なし |  |
| `impl CopyFileFromContainer for PathBuf` | あり | 対応 | 対応 |  |
| `impl CopyFileFromContainer for &Path` | あり | なし | なし |  |

### 13.6 `CopyFromContainerError`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `Io(#[from] io::Error)` | あり | 対応 | 対応 |  |
| `EmptyArchive` | あり | 対応 | 対応 |  |
| `IsDirectory` | あり | 対応 | 対応 |  |
| `UnsupportedEntry(tokio_tar::EntryType)` | あり | 部分対応 (意図的) | 部分対応 (意図的) | 内部型の差分は意図的。依存最小方針のため tokio_tar は追加しない |

### 13.7 `CopyToContainerError`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `IoError(std::io::Error)` | あり | 対応 | 対応 | `From<std::io::Error>` も実装 |
| `PathNameError(String)` | あり | 対応 | 対応 |  |

## 14. `Mount` / `MountType` / `AccessMode`

### 14.1 `Mount`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub fn bind_mount(host, container)` | あり | 対応 | 対応 | Docker: HostConfig.Binds に `source:target:ro|rw` で反映 |
| `pub fn volume_mount(name, container)` | あり | 対応 | 明示エラー | Docker: create 時に `volume mount is not implemented on Linux` |
| `pub fn tmpfs_mount(container)` | あり | 対応 | 明示エラー | Docker: create 時に `tmpfs mount is not implemented on Linux` |
| `pub fn with_access_mode(mut, mode)` | あり | 対応 | 対応 | Docker: Bind の `:ro` / `:rw` に反映 |
| `pub fn access_mode(&self) -> AccessMode` | あり | 対応 | 対応 | Docker: Bind の `:ro` / `:rw` に反映 |
| `pub fn mount_type(&self) -> MountType` | あり | 対応 | 部分対応 | Docker: Bind は反映。Volume/Tmpfs は create 時に明示エラー |
| `pub fn source(&self) -> Option<&str>` | あり | 対応 | 部分対応 | Docker: Bind は反映。Volume/Tmpfs は create 時に明示エラー |
| `pub fn target(&self) -> Option<&str>` | あり | 対応 | 部分対応 | Docker: Bind は反映。Volume/Tmpfs は create 時に明示エラー |
| `pub fn with_size_bytes(mut, size)` | あり | 対応 | 未反映 | tmpfs 用。XPC `options` に `size=<bytes>` |
| `pub fn with_size(mut, "20g")` | あり | 対応 | 未反映 | tmpfs 用。人間可読サイズをバイトへ変換 |
| `pub fn with_mode(mut, mode)` | あり | 対応 | 未反映 | tmpfs 用。XPC `options` に `mode=<octal>` |
| `pub fn tmpfs_options(&self) -> Option<&MountTmpfsOptions>` | あり | 対応 | 未反映 |  |

### 14.2 `MountType` / `AccessMode` / `MountTmpfsOptions`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `MountType::Bind` | あり | 対応 | 対応 | Docker: HostConfig.Binds に反映 |
| `MountType::Volume` | あり | 対応 | 明示エラー | Docker: create 時に明示エラー |
| `MountType::Tmpfs` | あり | 対応 | 明示エラー | Docker: create 時に明示エラー |
| `MountType` `Display (snake_case)` | あり | 対応 | 対応 |  |
| `AccessMode::ReadOnly` | あり | 対応 | 対応 | Docker: Bind の `:ro` に反映 |
| `AccessMode::ReadWrite` | あり | 対応 | 対応 | Docker: Bind の `:rw` に反映 |
| `AccessMode` `Display (ro/rw)` | あり | 対応 | 対応 | Docker: Bind のサフィックスに利用 |
| `MountTmpfsOptions` struct + `size_bytes` `mode` | あり | 対応 | 未反映 | XPC `Filesystem.options` の `size=` / `mode=` |
| `impl Default for MountTmpfsOptions` | あり | 対応 | 未反映 |  |

## 15. `ContainerPort` / `Ports` / `IntoContainerPort` / `PortMapping`

### 15.1 `ContainerPort`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `ContainerPort::Tcp(u16)` | あり | 対応 | 対応 |  |
| `ContainerPort::Udp(u16)` | あり | 対応 | 対応 |  |
| `ContainerPort::Sctp(u16)` | あり | 対応 | 対応 | Apple container の PublishProtocol は tcp/udp のみで sctp なし。`build_config` 時点で明示エラー (`SCTP port publishing is not supported on Apple container`) になる / Docker: Docker Engine では SCTP 公開を拒否しない (create JSON に載る) |
| `Display` (`8080/tcp` 形式) | あり | 対応 | 対応 |  |
| `FromStr` | あり | 対応 | 対応 | shiguredo は `unknown protocol` の場合 `Err(String)` |
| `Hash, Eq, PartialEq, Copy, Clone` | あり | 対応 | 対応 |  |
| `pub fn as_u16(self) -> u16` | あり | 対応 | 対応 |  |

### 15.2 `Ports`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub struct Ports { ipv4_mapping, ipv6_mapping }` | あり | 対応 | 対応 |  |
| `pub fn new(ports: HashMap<String, Option<Vec<HashMap<String, String>>>>)` | あり | なし | なし | Docker Engine API の内部表現に依存する互換 API |
| `pub fn map_to_host_port_ipv4(&self, port) -> Option<u16>` | あり | 対応 | 対応 |  |
| `pub fn map_to_host_port_ipv6(&self, port) -> Option<u16>` | あり | 対応 | 対応 |  |
| `pub(crate) fn ipv4_mapping(&self)` | あり | 内部メソッド | 内部メソッド |  |
| `pub(crate) fn ipv6_mapping(&self)` | あり | 内部メソッド | 内部メソッド |  |
| `impl TryFrom<PortMap> for Ports` | あり | なし | なし | Docker Engine API の内部表現に依存 |
| `Default, Clone, Debug, Eq, PartialEq` | あり | 対応 | 対応 |  |
| (Ports の中身) | あり | 対応 | 対応 | 6.2 参照 |

### 15.3 `IntoContainerPort`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `trait IntoContainerPort` | あり | 対応 | 対応 |  |
| `fn tcp(self) -> ContainerPort` | あり | 対応 | 対応 |  |
| `fn udp(self) -> ContainerPort` | あり | 対応 | 対応 |  |
| `fn sctp(self) -> ContainerPort` | あり | 対応 | 対応 |  |
| `impl IntoContainerPort for u16` | あり | 対応 | 対応 |  |
| `impl From<u16> for ContainerPort` | あり | 対応 | 対応 |  |

### 15.4 `PortMappingError`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `PortMappingError::FailedToParseContainerPort(parse_display::ParseError)` | あり | なし | なし | Ports::new / TryFrom<PortMap> 未対応と表裏 |
| `PortMappingError::FailedToParseHostPort(ParseIntError)` | あり | なし | なし |  |

## 16. `ContainerState` / `Host` / `CgroupnsMode` / `Healthcheck`

### 16.1 `ContainerState` (`core::image::ContainerState`)

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub struct ContainerState { id, host, ports }` | あり | 対応 | 対応 | フィールドは private |
| `pub async fn from_container<I: Image>(&ContainerAsync<I>) -> Result<Self>` | あり | 対応 | 対応 | `ContainerAsync::container_state()` へ委譲。`new` は `pub(crate)` のまま (ユーザーコードから直接生成しない) |
| `pub fn host(&self) -> &Host` | あり | 対応 | 対応 |  |
| `pub fn host_port_ipv4(&self, port) -> Result<u16>` | あり | 対応 | 対応 |  |
| `pub fn host_port_ipv6(&self, port) -> Result<u16>` | あり | 対応 | 対応 |  |

### 16.2 `Host` (`core::containers::request::Host`)

15 章末尾 (8 章の下段) と重複。省略。

### 16.3 `CgroupnsMode`

8 章と重複。省略。

### 16.4 `Healthcheck` (`core::healthcheck::Healthcheck`)

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub struct Healthcheck { test, interval, timeout, retries, start_period, start_interval }` | あり | 対応 | 対応 | Apple container はカスタム healthcheck 未対応 (`with_health_check` は macOS で start 時に明示エラー) |
| `pub fn none()` | あり | 対応 | 対応 |  |
| `pub fn cmd_shell(cmd)` | あり | 対応 | 対応 |  |
| `pub fn cmd<I,S>(cmd)` | あり | 対応 | 対応 |  |
| `pub fn empty()` | あり | 対応 | 対応 |  |
| `pub fn with_interval(mut, d)` | あり | 対応 | 対応 |  |
| `pub fn with_timeout(mut, d)` | あり | 対応 | 対応 |  |
| `pub fn with_retries(mut, n)` | あり | 対応 | 対応 |  |
| `pub fn with_start_period(mut, d)` | あり | 対応 | 対応 |  |
| `pub fn with_start_interval(mut, d)` | あり | 対応 | 対応 |  |
| `pub fn test/interval/timeout/retries/start_period/start_interval` accessor | あり | 対応 | 対応 |  |
| `pub(crate) fn into_health_config()` | あり | なし | なし | shiguredo は `to_docker_json()` に置換 (bollard 非依存) |
| `pub(crate) fn to_docker_json() -> Option<String>` | なし | なし | 対応 | shiguredo 拡張。Docker Config.Healthcheck JSON を生成 (Linux 専用) |

## 17. エラー型 `Error` と サブエラー

### 17.1 `Error`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `Client(#[from] ClientError)` | あり | 対応 | 対応 | ClientError の中身は違う (17.4 参照) |
| `WaitContainer(#[from] WaitContainerError)` | あり | 対応 | 対応 |  |
| `PortNotExposed { id, port }` | あり | 対応 | 対応 |  |
| `MissingInfo(#[from] ContainerMissingInfo)` | あり | 対応 | 対応 |  |
| `Exec(#[from] ExecError)` | あり | 対応 | 対応 | Docker: exit code mismatch 等で利用 |
| `Io(#[from] std::io::Error)` | あり | 対応 | 対応 |  |
| `Other(Box<dyn Error + Sync + Send>)` | あり | 対応 | 対応 |  |
| `Error::other<E>(e) -> Self` | あり | 対応 | 対応 |  |
| `pub type Result<T>` | あり | 対応 | 対応 |  |

### 17.2 `ContainerMissingInfo`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `#[error("container '{id}' does not have: {path}")]` | あり | 対応 | 対応 |  |
| `pub(crate) fn new(id, path)` | あり | 対応 | 対応 |  |

### 17.3 `ExecError`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `ExitCodeMismatch { expected: i64, actual: i64 }` | あり | 対応 | 対応 |  |
| `WaitLog(#[from] WaitLogError)` | あり | 対応 | 対応 |  |

### 17.4 `ClientError`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `Init(BollardError)` | あり | なし | なし | shiguredo は Docker Engine API クライアントとして bollard を使わない |
| `Configuration(#[from] ConfigurationError)` | あり | 部分対応 | 部分対応 | サブエラー型 (`ConfigurationError`) がない、単に文字列 |
| `InvalidDockerHost(String)` | あり | なし | なし |  |
| `PullImage / BuildImage / PortMapping / ListContainers / CreateContainer / RemoveContainer / StartContainer / StopContainer / PauseContainer / UnpauseContainer / InspectContainer / CreateNetwork / InspectNetwork / ListNetworks / RemoveNetwork / InitExec / InspectExec / UploadToContainerError / CopyToContainerError / CopyFromContainerError` | あり | なし | なし | shiguredo は Xpc 系エラーに置換 |
| `XpcConnect` | なし | shiguredo 拡張 | なし | XPC 接続失敗 / Docker: XPC 固有 |
| `Xpc(String)` | なし | shiguredo 拡張 | なし | Docker: XPC 固有 |
| `XpcNullReply` | なし | shiguredo 拡張 | なし | Docker: XPC 固有 |
| `ImageNotFound(String)` | なし | shiguredo 拡張 | 対応 | Docker: Linux の DockerClient でも使用 |
| `ContainerNotFound(String)` | なし | shiguredo 拡張 | 対応 | Docker: Linux の DockerClient でも使用 |
| `Json(String)` | なし | shiguredo 拡張 | 対応 | Docker: Linux の DockerClient でも使用 |
| `Other(String)` | なし | shiguredo 拡張 | 対応 | Docker: Linux の DockerClient でも使用 |

### 17.5 `ConfigurationError`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `InvalidDockerHost(String)` | あり | なし | なし | 型ごとなし |
| `UnknownCommand(String)` | あり | なし | なし |  |
| `WrongPropertiesFormat` (feature) | あり | なし | なし | feature = `properties-config` |
| `MissingContainerNameAndLabels` (feature) | あり | なし | なし | feature = `reusable-containers` |

### 17.6 `WaitContainerError`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `WaitLog(#[from] WaitLogError)` | あり | 対応 | 対応 |  |
| `StateUnavailable` | あり | 対応 | 対応 |  |
| `HttpWait(#[from] HttpWaitError)` (feature) | あり | 対応 | 対応 | feature = `http_wait_plain`。non-feature ビルドではバリアント自体が無い (本家と同じ) |
| `HealthCheckNotConfigured(String)` | あり | 対応 | 対応 |  |
| `Unhealthy(String)` | あり | 対応 | 対応 |  |
| `StartupTimeout` | あり | 対応 | 対応 |  |
| `UnexpectedExitCode { expected: i64, actual: Option<i64> }` | あり | 対応 | 対応 |  |

### 17.7 `WaitLogError`

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `EndOfStream(Vec<Bytes>)` | あり | 部分対応 | 部分対応 | 11 章と同一。payload は上限 (1 MiB) 内の直近ログ・連結済み (要素 0 または 1)。要素型は本家 `Bytes` / shiguredo `Vec<u8>` (意図的差分) |
| `Io(#[from] std::io::Error)` | あり | 対応 | 対応 |  |

## 18. `Network` (内部型)

本家では `Network` は `pub(crate)` の内部型。ユーザーには公開されていない。ただし `with_network(name)` で内部的に作られる。shiguredo は独立の `Network` 型を持たず、`with_network(name)` で指定された名前を XPC 側の `networks[0].network` に渡す (未指定時は `"default"`)。本家と違いネットワークの自動作成・自動削除は行わない。

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub(crate) struct Network` | あり | 内部型なので影響なし | 内部型なので影響なし |  |
| `pub(crate) async fn new(name, client)` | あり | 内部関数 | 内部関数 |  |
| `Drop` impl | あり | 内部関数 | なし | XPC `networkDelete` は存在するが、ネットワークライフサイクル管理は shiguredo にない / Docker: 同上 |

## 19. `GenericImage` (`images::generic::GenericImage`)

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub fn new<S: Into<String>>(name, tag)` | あり | 対応 | 対応 |  |
| `pub fn with_wait_for(mut, WaitFor)` | あり | 対応 | 対応 |  |
| `pub fn with_entrypoint(mut, &str)` | あり | 対応 | 対応 |  |
| `pub fn with_exposed_port(mut, ContainerPort)` | あり | 対応 | 対応 | Docker: 未マッピングなら `HostPort=0` の `PortBindings` に載せる。macOS: 事前に空きホストポートを割当 |
| `impl Image for GenericImage` | あり | 対応 | 対応 |  |

## 20. `GenericBuildableImage` / `BuildableImage` / `BuildContextBuilder` / `BuildImageOptions`

現時点ではすべて **なし**。Apple container の BuildKit 統合は XPC 単独では完結せず、`container` デーモンが管理する builder コンテナに対して vsock/gRPC 経由の BuildKit shim (`container-builder-shim`) を呼び出す 2 段構成になっている。当面は `container build` CLI を `tokio::process::Command` で呼び出す subprocess 方式で本家互換 API を PoC し、需要と CLI 依存の制約を確認してから XPC/gRPC 完全実装への移行を検討する。完全実装には builder コンテナの lifecycle 管理、vsock dial、gRPC プロトコル、build context の転送、OCI tar 出力の受け取りとローカル image store への load が必要。

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub struct GenericBuildableImage` | あり | なし | なし |  |
| `pub fn new(name, tag)` | あり | なし | なし |  |
| `pub fn with_dockerfile(source: impl Into<PathBuf>)` | あり | なし | なし |  |
| `pub fn with_dockerfile_string(content)` | あり | なし | なし |  |
| `pub fn with_file(source, target)` | あり | なし | なし |  |
| `pub fn with_data(data, target)` | あり | なし | なし |  |
| `impl BuildableImage for GenericBuildableImage { type Built = GenericImage; ... }` | あり | なし | なし |  |
| `pub trait BuildableImage { type Built; fn build_context; fn descriptor; fn into_image }` | あり | なし | なし |  |
| `pub struct BuildContextBuilder` | あり | なし | なし |  |
| `pub fn with_dockerfile/with_dockerfile_string/with_file/with_data/collect/as_copy_to_container_collection` | あり | なし | なし |  |
| `pub struct BuildImageOptions` | あり | なし | なし |  |
| `pub fn new/with_skip_if_exists/with_no_cache/with_build_arg/with_build_args` | あり | なし | なし |  |

## 21. `ReuseDirective` (feature = `reusable-containers`)

現時点では本家 `reusable-containers` feature は未対応。reuse はローカル開発でのテスト起動高速化向けの利便機能で、テストライブラリとしての中核機能ではない。CI ではコンテナ掃除との相性が悪く、現時点では明確な需要が確認できていないため、需要が発生した場合に再検討する。

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `ReuseDirective::Never` | あり | なし | なし | `reusable-containers` feature 自体が未実装 (型も無し) |
| `ReuseDirective::Always` | あり | なし | なし |  |
| `ReuseDirective::CurrentSession` | あり | なし | なし |  |
| `impl Default` (Never) | あり | なし | なし |  |
| `impl Display` | あり | なし | なし |  |
| `ContainerRequest.reuse` フィールド | あり | なし | なし |  |
| `ContainerRequest::reuse()` accessor | あり | なし | なし |  |
| `ImageExt::with_reuse(self, dir)` | あり | なし | なし |  |
| `AsyncRunner::start` の reuse ロジック | あり | なし | なし |  |

## 22. feature ゲート

| feature | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `ring` (default) | あり | なし | なし | 該当なし (shiguredo では rustls を直接使用) |
| `aws-lc-rs` | あり | なし | なし | 同上 |
| `ssl` | あり | なし | なし | 同上 |
| `blocking` | あり | あり | 対応 | 同期 API は ContainerAsync に委譲。Linux はライフサイクル・ログ・copy とも利用可 |
| `watchdog` | あり | あり (macOS のみ) | なし | 対応。本家 (シグナルハンドラ方式) と異なり外部 reaper プロセス方式 (下記) / Linux では未提供 |
| `http_wait` / `http_wait_plain` | あり | あり (`http_wait_plain`) | 部分対応 | ports() 配線済み。HTTP wait / Log 待機とも利用可 |
| `properties-config` | あり | なし | なし | なし。macOS では外部プロパティファイルを読まない設計 |
| `reusable-containers` | あり | なし | なし | なし。feature・型 (ReuseDirective) ともに未実装 (21 章参照) |
| `device-requests` | あり | なし | なし | なし。GPU デバイスマッピング相当が Apple container に無い |
| `host-port-exposure` | あり | なし | なし | 未対応 (方針)。SSH tunnel は実装せず、ホスト側サービスへはコンテナから直接到達できるネットワーク構成を使う |
| `docker-compose` | あり | なし | なし | なし。Apple container は compose 相当を持たない |

### watchdog の仕組み (macOS)

本家のシグナルハンドラ方式は SIGKILL / SIGSEGV では動かないため、shiguredo は外部 reaper プロセス方式を採る。`watchdog` feature 有効時、最初のコンテナ起動で reaper (`/bin/sh`) を別プロセスグループで起動し、`create_container` 成功直後にコンテナ ID を pipe 経由で登録する (bootstrap / start / copy 途中のクラッシュでも孤立を掃除するため)。テストプロセスが死ぬと (シグナル種別を問わず) pipe が EOF になり、reaper が登録済みコンテナを `container rm --force` で削除する。通常終了時は Drop が先に削除しているため reaper の rm は無害に失敗する。`TESTCONTAINERS_COMMAND=keep` 指定時は登録自体を行わない。

## 23. `compose` モジュール + `bollard` 再エクスポート

shiguredo には `compose` モジュールも `bollard` の再エクスポートも存在しない。

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub mod compose;` (feature = `docker-compose`) | あり | なし | なし |  |
| `pub use bollard;` | あり | なし | なし | shiguredo は bollard を依存に含まない |
| `pub use bollard_stubs` (2次エクスポート) | あり | なし | なし | 同上 |

## 24. `lib.rs` の pub use と shiguredo 拡張

| API | 本家 | Apple Container | Docker Engine API | 備考 |
|:--|:--|:--|:--|:--|
| `pub use crate::core::Container` (feature=blocking) | あり | 対応 | 対応 |  |
| `pub use crate::core::ReuseDirective` (feature=reusable-containers) | あり | なし | なし | 21 章参照 |
| `pub use crate::core::{...}` (CopyDataSource, CopyTargetOptions, CopyToContainer, CopyToContainerError, Error, BuildableImage, ContainerAsync, ContainerRequest, Healthcheck, Image, ImageExt)` | あり | 部分対応 | 部分対応 | copy 系 4 型 (`CopyDataSource` / `CopyTargetOptions` / `CopyToContainer` / `CopyToContainerError`) は `core::` 経由で再エクスポート。crate root 直下には無い。`BuildableImage` は shiguredo に無し。`Healthcheck` は 16.4 節参照 (Linux 対応、macOS は accessor 対応・`with_health_check` は明示エラー)。`ExecCommand` / `WaitFor` は shiguredo 独自に追加 (元の crate の lib.rs では pub use にない) |
| `pub use buildables::generic::GenericBuildableImage;` | あり | なし | なし | 20 章参照 |
| `pub use images::generic::GenericImage;` | あり | 対応 | 対応 |  |
| `pub use bollard;` | あり | なし | なし | 23 章 |
| `AsyncRunner` (lib.rs 直下 pub use) | なし (runners:: 経由) | 対応 | 対応 | shiguredo は `pub use crate::runners::AsyncRunner` を lib.rs で行っており、Linux 側 pub use に `runners` が入っているのと同等 |
| `SyncRunner` (lib.rs 直下 pub use, feature=blocking) | なし (runners:: 経由) | 対応 | 対応 |  |

---

## 実装不可 (XPC 制約)

Apple container / XPC に設定口や route が無く、本クレート単体では実装できない項目。Apple 側の仕様追加を待つ。

| 項目 | 理由 |
|:--|:--|
| `ContainerAsync::pause` / `unpause`、`Container::pause` / `unpause` | XPCRoute に pause 系が無い。stub で常にエラーを返す方針は採らず、シグネチャ自体を削除済み |
| `HealthWaitStrategy` | Apple container が Docker HEALTHCHECK 相当を実行・公開しない |
| `ImageExt::with_ulimit` | コンテナ全体の ulimit に相当する XPC 項目が無い (プロセス rlimits とは別) |
| `ImageExt::with_cgroupns_mode` | XPC に該当項目が無い |
| `ImageExt::with_userns_mode` | XPC に該当項目が無い |
| `ImageExt::with_security_opt` | XPC に該当項目が無い |
| `ImageExt::with_host_config_modifier` | 本家は bollard HostConfig 前提。bollard 非依存かつ XPC に包括 modifier が無い |

方針で未対応にしているもの (XPC 以前に導入しない決定) は上表に含めない。例: `host-port-exposure`、`device-requests`、`reusable-containers`、HttpWait の TLS / `with_client`。

## シグネチャあり・macOS では明示エラー

Apple container の XPC には対応 route が無いが、本家 API 互換のためシグネチャは公開し、start 時に明示エラーを返す項目。

| 項目 | 理由 |
|:--|:--|
| `Healthcheck` 型、`ImageExt::with_health_check` | XPC にヘルスチェック設定口が無い。Linux では Config.Healthcheck に配線済み |

## 意図的に保持する shiguredo 拡張

以下は本家に対応 API が無い追加である。

- `ImageExt::with_init` — Apple: XPC `useInit` に反映。Docker: create JSON の HostConfig.Init として反映
- `ImageExt::with_ssh` — Apple: XPC `ssh` に反映。Docker: 未反映 (Apple 固有)
- `ContainerAsync::container_state` — Apple: 対応。Docker: 配線済み
- `Container::container_state` — Apple: 対応 (ContainerAsync に委譲)。Docker: 配線済み

---

## 本家との型不整合の注意点

以下は本家 testcontainers-rs とシグネチャの型が異なるため、本家からの移行時に同じユーザーコードが通らない可能性が高い箇所。

| API | 本家型 | shiguredo 型 | 備考 |
|:--|:--|:--|:--|
| `CopyFromContainerError::UnsupportedEntry` 型 | `tokio_tar::EntryType` | `&'static str` | 意図的な差分 (tokio_tar 依存を追加しない方針) |
| `WaitLogError::EndOfStream` 要素型 | `Vec<Bytes>` | `Vec<Vec<u8>>` | 意図的な差分 (`bytes` 依存を追加しない方針) |
