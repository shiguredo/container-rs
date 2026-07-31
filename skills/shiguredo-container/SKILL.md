---
name: shiguredo-container
description: 時雨堂のテスト用コンテナライブラリ shiguredo_container の機能・API リファレンス。Apple container (macOS XPC) / Docker Engine API (Linux) でのコンテナ起動、待機戦略 (WaitFor / Healthcheck)、exec、ログ、ポート解決、ファイルコピー、feature 構成に関する質問時に使用。
---

# shiguredo_container

Apple の [container](https://github.com/apple/container) 対応をメインとする Rust 用テストコンテナライブラリ。

## 特徴

- **macOS がメイン対象**: Apple container の XPC API を自前実装で直接叩く。Docker Desktop 不要
- **Linux 対応**: Docker Engine API (`/var/run/docker.sock`) を利用。ライフサイクル (start / exec / stop / rm / Drop)、ログ関連 (stdout / stderr / ログ待機 / LogConsumer)、ホストポート公開、ファイルコピー (`copy_file_from` / `with_copy_to`)、ヘルスチェック (`with_health_check` / `WaitFor::healthcheck`)、exec の stdout / stderr 取得、bridge IP 取得が動く。network 系設定などは未対応
- **testcontainers-rs 互換 API**: 学習コスト削減のため公開 API を testcontainers-rs 0.27 に寄せている (完全互換は目指さない)
- **依存最小**: `libc` / `nojson` / `shiguredo_http11` / `tokio` / `tracing` (+ optional `base64ct`)。bollard / reqwest / bytes 等は使わない
- **黙って無視しない**: 未対応の設定はリクエストに保存だけして無視するのではなく、start / create 時に明示エラーを返す

## バージョン情報

- crate 名: `shiguredo_container`
- バージョン: 2026.1.0-canary.4
- Rust Edition: 2024
- 最小 Rust バージョン: 1.93
- ライセンス: Apache-2.0

## 要件

| OS | ランタイム | 備考 |
|:--|:--|:--|
| macOS 26 (Apple Silicon) | `container` (`brew install container`) | `container system start` 済みであること |
| Linux | Docker Engine (Docker Engine API 互換) | Podman 等 API 互換ランタイムも可 |

## feature フラグ

| feature | 説明 |
|:--|:--|
| `blocking` | 同期 API (`Container` / `SyncRunner` / `SyncExecResult`) を有効化 |
| `http_wait_plain` | `WaitFor::http` (`HttpWaitStrategy`) を有効化。plain HTTP のみ (TLS 非対応)。`base64ct` が有効になる |
| `watchdog` | テストプロセスのクラッシュ (SIGKILL 含む) 時に孤立コンテナを掃除する (macOS のみ) |

## コア API

### crate root の公開 API (`lib.rs`)

| 型 / トレイト | 説明 |
|:--|:--|
| `GenericImage` | 汎用イメージ定義。`new(name, tag)`, `with_wait_for(WaitFor)`, `with_entrypoint(&str)`, `with_exposed_port(ContainerPort)` |
| `Image` | イメージトレイト (`name()`, `tag()`, `ready_conditions()`, `env_vars()`, `mounts()`, `cmd()`, `expose_ports()`, `exec_after_start()`, `exec_before_ready()` 等)。独自イメージ型を作るときに実装する |
| `ImageExt` | リクエスト構築 builder トレイト (下表参照) |
| `AsyncRunner` | `start() -> ContainerAsync<I>`, `pull_image() -> ContainerRequest<I>` (async) |
| `SyncRunner` | `start() -> Container<I>`, `pull_image()` (feature = `blocking`) |
| `ContainerAsync<I>` | 起動済みコンテナ (async, 下表参照) |
| `Container<I>` | 起動済みコンテナ (sync, feature = `blocking`)。各メソッドは `ContainerAsync` に委譲 |
| `ContainerRequest<I>` | `ImageExt` で構築されるリクエスト。各種 accessor を持つ |
| `Healthcheck` | ヘルスチェック設定型。Linux は Config.Healthcheck に配線。macOS は `with_health_check` 指定時に start で明示エラー |
| `ExecCommand` | exec コマンド定義 |
| `WaitFor` | 準備完了待機戦略 (下表参照) |
| `Error` | エラー型 (下表参照) |

`core::` 経由でエクスポートされる型: `ContainerState`, `ExecResult`, `SyncExecResult`, `ExtraHost`, `PortMapping`, `Host`, `Mount` / `MountType` / `AccessMode` / `MountTmpfsOptions`, `ContainerPort` / `IntoContainerPort` / `Ports`, `CmdWaitFor`, copy 系 (`CopyDataSource` / `CopyFileFromContainer` / `CopyFromContainerError` / `CopyTargetOptions` / `CopyToContainer` / `CopyToContainerError`)。待機戦略型は `core::wait::` (`HttpWaitStrategy`, `LogWaitStrategy`, `ExitWaitStrategy` 等)、ログ系は `core::logs::LogFrame` / `core::logs::LogSource` / `core::logs::LogConsumer` / `core::logs::LoggingConsumer`。

### `ImageExt` の主要メソッド

| メソッド | macOS (XPC) | Linux (Docker) |
|:--|:--|:--|
| `with_cmd`, `with_name`, `with_tag`, `with_container_name`, `with_label(s)`, `with_env_var`, `with_mapped_port`, `with_startup_timeout`, `with_working_dir`, `with_ready_conditions` | 対応 | 対応 |
| `with_platform` | 対応 (`"linux/amd64"` で rosetta / pull / architecture に反映) | start 時に明示エラー |
| `with_network` | 部分対応 (事前に `container network create` が必要。自動作成しない) | start 時に明示エラー |
| `with_mount` | 対応 (Bind / Volume / Tmpfs) | Bind のみ (`ro`/`rw` 付き)。Volume / Tmpfs は明示エラー |
| `with_copy_to` | 対応 (XPC `containerCopyIn`。start 後 copy。起動前契約なし。親作成は `createParents`。ディレクトリ再帰投入可。`uid` / `gid` は非反映) | 対応 (create → copy → start。`PUT /containers/{id}/archive?path=/`。親ディレクトリ自動作成・ディレクトリ一括投入。`mode` / `uid` / `gid` は regular file に反映) |
| `with_log_consumer` | 対応 (行単位で `LogFrame` を配信) | 対応 (demux 済み共有バッファから行単位で配信。行末 `\n` / `\r` 剥がし、終端後の非改行残余は破棄) |
| `with_privileged` | 部分対応 (`capAdd: ["ALL"]` 相当) | 対応 |
| `with_cap_add`, `with_cap_drop`, `with_shm_size`, `with_readonly_rootfs`, `with_open_stdin`, `with_hostname` | 対応 | start 時に明示エラー |
| `with_host` | 部分対応 (`ExtraHost::Addr` は exec で `/etc/hosts` 追記。`HostGateway` はエラー) | start 時に明示エラー |
| `with_user` | 部分対応 (環境により非 root UID が機能しないことがある) | 対応 |
| `with_init` | **shiguredo 拡張** (XPC `useInit`) | 対応 (HostConfig.Init) |
| `with_ssh` | **shiguredo 拡張** (XPC `ssh`) | start 時に明示エラー |
| `with_health_check` | 未実装 (XPC 制約)。start 時に明示エラー | 対応 (Config.Healthcheck)。`WaitFor::healthcheck` と併用可 |

本家にあって存在しないもの: `with_ulimit` / `with_cgroupns_mode` / `with_userns_mode` / `with_security_opt` / `with_host_config_modifier` / `with_reuse` / `with_exposed_host_port(s)` / `with_device_requests` (XPC に設定口が無い、または方針で未対応)。

### `ContainerAsync<I>` / `Container<I>` のメソッド

| メソッド | macOS (XPC) | Linux (Docker) |
|:--|:--|:--|
| `id()`, `image()` | 対応 | 対応 |
| `ports()`, `get_host_port_ipv4(port)`, `get_host_port_ipv6(port)` | 対応 | 対応 |
| `get_bridge_ip_address()` | 対応 (`networks[0].ipv4Address` から抽出) | 対応 (inspect の `NetworkSettings.Networks` 先頭 `IPAddress`) |
| `get_host()` | `localhost` 固定 | `localhost` 固定 |
| `exec(ExecCommand)` | 対応 (stdout / stderr / env 付き) | 対応 (stdout / stderr / env 付き) |
| `start()` (再起動), `stop()`, `stop_with_timeout(Option<i32>)`, `is_running()`, `rm()`, `rm_blocking()` | 対応 | 対応 |
| `container_state()` | **shiguredo 拡張** | 対応 |
| `exit_code()` | 部分対応 (バックグラウンド wait の観測済みキャッシュのみ) | 部分対応 (バックグラウンド wait の観測済みキャッシュのみ) |
| `copy_file_from(path, target)` | 対応 (`Vec<u8>` / `PathBuf` を target にできる) | 対応 (`GET /containers/{id}/archive` + 自前 ustar パーサ。source は絶対パス必須・ファイル専用) |
| `stdout(follow)`, `stderr(follow)`, `stdout_to_vec()`, `stderr_to_vec()` | 対応 (`follow=true` は追記ポーリング) | 対応 (demux 済み共有バッファ。ストリームあたり 8 MiB・drop-oldest。`follow=false` は呼び出しごとに新規 HTTP セッションで全ログ取得) |
| `Drop` | 対応 (削除。Keep ゲートあり) | 対応 |

`pause` / `unpause` は XPC に route が無いためシグネチャごと存在しない。

`stop_with_timeout` の意味: macOS では `Some(0)` = 即時 SIGKILL、`None` = 30 秒 SIGTERM。Linux では `Some(t>=0)` = `t` 秒、`None`・負値 = 30 秒。

注意: macOS の stderr 側ログは VM の bootlog を指す。アプリケーションの stderr は stdout 側ログに混流する。Linux は Docker Engine API の STREAM_TYPE で stdout / stderr が正しく分離される。

### `WaitFor` (準備完了待機)

| コンストラクタ | macOS (XPC) | Linux (Docker) |
|:--|:--|:--|
| `WaitFor::Nothing` (既定) | 対応 | 対応 |
| `WaitFor::message_on_stdout(msg)` / `message_on_stderr(msg)` / `message_on_either_std(msg)` / `log(LogWaitStrategy)` | 対応 | 対応 (demux 済みログストリームに対して待機) |
| `WaitFor::http(HttpWaitStrategy)` (feature = `http_wait_plain`) | 対応 | 対応 (host port 解決可) |
| `WaitFor::exit(ExitWaitStrategy)` | 対応 | 対応 |
| `WaitFor::healthcheck()` | 未実装 (XPC 制約: Apple container は HEALTHCHECK を実行しない) | 対応 (inspect ポーリング。`starting` / `healthy` / `unhealthy` / Health 不在) |
| `WaitFor::seconds(n)` / `millis(n)` / `millis_in_env_var(name)` | 対応 | 対応 |

待機戦略型:

- `LogWaitStrategy`: `stdout(msg)`, `stderr(msg)`, `stdout_or_stderr(msg)`, `new(source, msg)`, `with_times(n)`
- `HttpWaitStrategy`: `new(path)`, `with_port`, `with_method` (文字列), `with_header`, `with_body`, `with_basic_auth`, `with_bearer_auth`, `with_poll_interval`, `with_expected_status_code`, `with_response_matcher(Fn(&HttpResponse) -> bool)`。reqwest ではなく `shiguredo_http11` + `tokio::net::TcpStream` で実装。TLS / `with_client` / `with_response_matcher_async` は無い
- `ExitWaitStrategy`: `new()`, `with_poll_interval`, `with_exit_code`
- `HealthWaitStrategy`: Linux は inspect ポーリングで判定。macOS は常に `HealthCheckNotConfigured` エラー (XPC 制約)

### `ExecCommand` / `ExecResult` / `CmdWaitFor`

- `ExecCommand::new(["cmd", "arg"])`, `with_container_ready_conditions(Vec<WaitFor>)`, `with_cmd_ready_condition(CmdWaitFor)`, `with_env_vars(iter)` (macOS: コンテナ env にマージされ同名は ExecCommand 側優先。Linux: 非空だと明示エラー)
- `ExecResult`: `exit_code()`, `stdout()`, `stderr()`, `stdout_to_vec()`, `stderr_to_vec()`。exec 完了時点の全出力を保持したバッファ上のリーダーを返す (消費型)。Linux も multiplexed stream demux で stdout / stderr を返す
- `CmdWaitFor`: `message_on_stdout(msg)` / `message_on_stderr(msg)` (両 OS とも取得済みバッファへの部分一致)、`exit()`, `exit_code(n)`, `seconds(n)`, `millis(n)`

### `Mount` / ポート

- `Mount::bind_mount(host, container)` / `volume_mount(name, container)` / `tmpfs_mount(container)`, `with_access_mode(AccessMode::ReadOnly | ReadWrite)`。tmpfs は `with_size_bytes` / `with_size("20g")` / `with_mode(0o770)`
- `ContainerPort::Tcp(u16)` / `Udp(u16)` / `Sctp(u16)`。`IntoContainerPort` により `80.tcp()` / `53.udp()` と書ける。SCTP は Apple container 非対応で明示エラー
- `Ports`: `map_to_host_port_ipv4(port)` / `map_to_host_port_ipv6(port)`

### `LogConsumer` / `LoggingConsumer`

- `LogConsumer::accept(&LogFrame)` トレイト。`Fn(&LogFrame)` クロージャにも blanket impl がある
- `LogFrame::StdOut(Vec<u8>)` / `StdErr(Vec<u8>)`、`source()`, `bytes()`
- `LoggingConsumer::new()` (既定は `eprintln!`)、`with_stdout_level(tracing::Level)`, `with_stderr_level`, `with_prefix`

## コード例

### 非同期 API (nginx + HTTP 待機)

```rust
use std::time::Duration;

use shiguredo_container::{
    AsyncRunner, GenericImage, ImageExt, WaitFor,
    core::{IntoContainerPort, wait::HttpWaitStrategy},
};

#[tokio::test]
async fn test_with_nginx() {
    let container = GenericImage::new("nginx", "latest")
        .with_exposed_port(80.tcp())
        .with_wait_for(WaitFor::http(
            HttpWaitStrategy::new("/")
                .with_port(80.tcp())
                .with_expected_status_code(200_u16),
        ))
        .with_startup_timeout(Duration::from_secs(120))
        .start()
        .await
        .expect("failed to start nginx");

    let host_port = container
        .get_host_port_ipv4(80.tcp())
        .await
        .expect("failed to resolve published host port");
    // 127.0.0.1:host_port へ接続してテスト...

    container.stop().await.expect("failed to stop");
    container.rm().await.expect("failed to remove");
}
```

### ブロッキング API (feature = `blocking`)

```rust
use shiguredo_container::{GenericImage, ImageExt, SyncRunner, WaitFor};

#[test]
fn test_with_nginx_blocking() {
    let container = GenericImage::new("nginx", "latest")
        .with_wait_for(WaitFor::message_on_either_std("start worker processes"))
        .with_startup_timeout(std::time::Duration::from_secs(180))
        .start()
        .expect("failed to start nginx");

    container.stop().expect("failed to stop");
    container.rm().expect("failed to remove");
}
```

### exec

```rust
use shiguredo_container::{AsyncRunner, GenericImage, ImageExt, core::ExecCommand};

let container = GenericImage::new("alpine", "latest")
    .with_cmd(["tail", "-f", "/dev/null"])
    .start()
    .await?;

let mut result = container.exec(ExecCommand::new(["uname", "-m"])).await?;
assert_eq!(result.exit_code().await?, Some(0));
let stdout = result.stdout_to_vec().await?;
```

### ファイルコピー

```rust
// コンテナ → ホスト (Vec<u8> または PathBuf に受ける)
let bytes: Vec<u8> = container.copy_file_from("/etc/os-release", Vec::new()).await?;

// ホスト → コンテナ (リクエスト組み立ては start 前。実コピー実行タイミングは OS 依存)
// Linux: create 後・start 前に投入完了。macOS: start_process 後に投入（起動前契約なし）
use shiguredo_container::core::{CopyToContainer, CopyTargetOptions};
let image = GenericImage::new("alpine", "latest")
    .with_copy_to("/etc/config.json", b"{}".to_vec());
```

Linux は親ディレクトリ自動作成とディレクトリ一括投入に対応する。`with_copy_to` の起動前投入は Linux のみの公開契約である。

### LogConsumer

```rust
use shiguredo_container::core::logs::LogFrame;

let container = GenericImage::new("nginx", "latest")
    .with_log_consumer(move |frame: &LogFrame| {
        eprintln!("{:?}: {:?}", frame.source(), frame.bytes());
    })
    .start()
    .await?;
```

## コンテナの掃除契約

- 既定 (`TESTCONTAINERS_COMMAND` 未設定または `remove`) では `ContainerAsync` / `Container` の Drop 時にコンテナを削除する。削除経路は Drop が Runtime 内か外かで異なる
  - Runtime 内 Drop (`Handle::try_current()` が `Ok`): 削除を専用 std スレッドで実行し、5 秒 (`DROP_REMOVE_TIMEOUT`) を上限に完了を待つ。timeout 内に完了すれば `drop` 復帰時点で削除は終わっている。超過時は best-effort (削除スレッドは裏で走り続けるが、`drop` 直後にプロセスが終了すると中断され得る)
  - Runtime 外 Drop: 呼び出しスレッドで削除試行が終わるまで待つ (成功は保証しない。失敗は `tracing::error` に記録するのみで呼び出し側には届かない)
- 削除の完了待ち、または成否の `Result` が必要なら明示 `rm()` を使う (async は `rm().await`、sync は `rm()`)。同期コンテキスト (Runtime 内の Drop ガードや `spawn_blocking` 内) から削除完了を待ちたい場合は `rm_blocking()` を使う (`block_on` を使わないため Runtime 内から呼んでも deadlock しない)。明示 `rm()` / `rm_blocking()` は `TESTCONTAINERS_COMMAND=keep` でも削除する (Drop の `keep` ゲートとは非対称)
- `TESTCONTAINERS_COMMAND=keep` のときは Drop で削除しない (調査用に残す)
- `stop()` は LogConsumer 配信を止める。明示的な `rm()` を呼ばなくても Drop で削除される (keep 除く)
- `watchdog` feature (macOS): 外部 reaper プロセス方式。テストプロセスが SIGKILL / SIGSEGV で死んでも pipe EOF を検知して登録済みコンテナを `container rm --force` する。`keep` 指定時は登録しない

## 環境変数

| 変数 | 説明 |
|:--|:--|
| `TESTCONTAINERS_COMMAND` | `keep` で Drop 時の削除を抑止 (既定は `remove`) |
| `RUN_HOST_NETWORK_TESTS` | 本リポジトリの統合テスト用。`1` でホスト→コンテナ接続依存のテストを有効化 |

## エラー型

`Error` (crate root): `Client(ClientError)`, `WaitContainer(WaitContainerError)`, `PortNotExposed { id, port }`, `MissingInfo(ContainerMissingInfo)`, `Exec(ExecError)`, `Io(std::io::Error)`, `Other(Box<dyn Error>)`。`pub type Result<T>` あり。

- `ClientError`: shiguredo 拡張として `XpcConnect` / `Xpc(String)` / `XpcNullReply` / `ImageNotFound` / `ContainerNotFound` / `Json` / `Other` を持つ (bollard 系エラーは無い)
- `WaitContainerError`: `WaitLog`, `StateUnavailable`, `HttpWait(HttpWaitError)` (feature), `HealthCheckNotConfigured`, `Unhealthy`, `StartupTimeout`, `UnexpectedExitCode { expected, actual }`
- `ExecError`: `ExitCodeMismatch { expected, actual }`, `WaitLog(WaitLogError)`
- `WaitLogError`: `EndOfStream(Vec<Vec<u8>>)` (本家は `Vec<Bytes>`。意図的差分), `Io`

## 既知の制限事項

- **macOS の Local Network Privacy (LNP)**: `HttpWaitStrategy` や published port への接続は macOS 15+ の LNP にブロックされ得る。LNP は TCC / MDM で事前付与できない。CI ではコンテナ IP 直結テストを基本とし、published port 依存テストは許可済み環境でのみ実行する
- **blocking の再入 deadlock**: `LogConsumer` コールバック内や既存の tokio ランタイムコンテキストから `SyncRunner::start` 等の同期 API を呼ぶと共有 Runtime への再入で deadlock する。ライブラリは再入を検出して即エラーにするが、コールバック内での同期 API 呼び出しは避けること。共有 Runtime ワーカースレッド上で最後の同期 `Container` を drop するとハングし得る既知の限界もある
- **Linux の残ギャップ**: `ExitWaitStrategy`、`with_platform` / `with_network` / `with_hostname` / `with_host` / `with_cap_add` / `with_cap_drop` / `with_shm_size` / `with_readonly_rootfs` / `with_open_stdin` / `with_ssh`、Volume / Tmpfs mount が未対応。詳細は `docs/TESTCONTAINERS.md` 参照
- **イメージビルド未対応**: `GenericBuildableImage` / `BuildableImage` 等の build 系 API は無い
- **reuse 未対応**: `reusable-containers` 相当の feature・型は無い
- **本家との型不整合**: `CopyFromContainerError::UnsupportedEntry` は `&'static str` (本家 `tokio_tar::EntryType`)、`WaitLogError::EndOfStream` は `Vec<Vec<u8>>` (本家 `Vec<Bytes>`)。いずれも依存最小方針による意図的差分

## 参考資料

- 本家 testcontainers-rs との API 対応表 (409 API の判定一覧): `docs/TESTCONTAINERS.md`
