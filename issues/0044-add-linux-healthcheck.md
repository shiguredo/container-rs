# 機能追加: Linux で Healthcheck 型と with_health_check を実装する

- Priority: Medium
- Created: 2026-07-23
- Completed: {YYYY-MM-DD}
- Model: Claude Fable 5
- Branch: feature/add-linux-healthcheck
- Polished: 2026-07-24
- Reporter: @voluntas

## 目的

Linux (Docker Engine API) バックエンドで、本家 testcontainers-rs 0.27 互換の `Healthcheck` 型・`ImageExt::with_health_check`・`ContainerRequest::health_check` accessor・`HealthWaitStrategy::wait_until_ready` の Linux 分岐を実装する。`WaitFor::Healthcheck` variant と `WaitFor::healthcheck()` コンストラクタは既存 (`src/core/wait/mod.rs:40, 81-83`) で、実装対象は待機ロジックの Linux 分岐と、それを駆動する設定口・API 型一式。

利用者フィードバック (mqtt-rs の EMQX SCRAM テスト) で、HTTP wait だけでは「Dashboard が応答する」までしか保証できず、`/status` が 200 を返しても SCRAM provider が未登録で `no_available_provider_for` になるレースが CI で発生した。本家なら次のようにアプリ固有の ready をコンテナ側に寄せられる。

```rust
.with_health_check(
    Healthcheck::cmd_shell("...")
        .with_interval(Duration::from_secs(1))
)
.with_wait_for(WaitFor::healthcheck())
```

EMQX 公式イメージ自体に HEALTHCHECK は無いため、イメージ側ではなく利用側の設定口 (`with_health_check`) を本命として扱う。

## 優先度根拠

mqtt-rs CI の失敗の直接原因であり、テスト側の認証 API リトライで回避は可能だが、本家にあるギャップの中で mqtt-rs 観点の効きが最も大きい。macOS は Apple container の XPC に口が無く不可 (`pending/0002`) だが、CI が走る Linux だけでも価値が高い。Medium。

## 現状

### コード側

- `Healthcheck` 型と `ImageExt::with_health_check` はクレートに存在しない (`src/core/image/image_ext.rs` の trait 定義と `impl` の両方に無い)。`ContainerRequest` (`src/core/containers/request.rs:22-50`) にも `health_check` フィールド無し
- `HealthWaitStrategy::wait_until_ready` (`src/core/wait/health_strategy.rs:37-52`) は OS 非依存で無条件に `WaitContainerError::HealthCheckNotConfigured` を返す。`poll_interval` フィールド (既定 `Duration::from_millis(100)`) は保持しているが読まれていない
- `DockerClient::container_state` (`src/core/client/docker_client.rs:327-361`) が既に inspect を叩き、`ContainerSnapshot` (`src/core/client.rs:37-40`) に `running: bool` (source: `State.Running`) と `ports` を抽出している。`State` フィールド自体が inspect 応答から欠落した場合は `.required()?` 経由で `ClientError::Json` に寄せている。`State.Health` は読んでいない
- `build_container_config` (`src/runners/async_runner.rs:479-507`) と `ContainerConfig` (`src/core/client.rs:44-57`) に healthcheck の口が無い。JSON 組み立ては `CreateContainerBody::to_json_string` (`src/core/client/docker_client.rs:607-654`) の手書きで、`escape_json` (`src/core/client/docker_client.rs:971`, 現状 module-private) と `json_array` (単一文字列 → JSON string / `Vec<String>` → JSON array、`escape_json` を内部で呼ぶ) を使う。`Healthcheck` は Config 直下 (HostConfig の外側) に置く
- 本家 testcontainers-rs 0.27 の `Healthcheck` 型: `https://github.com/testcontainers/testcontainers-rs/blob/testcontainers-0.27.0/testcontainers/src/core/image/healthcheck.rs`。`pub struct Healthcheck { test, interval, timeout, retries, start_period, start_interval }` の 6 フィールド。derive は `Debug, Clone, PartialEq, Eq`。`with_health_check` シグネチャは `fn with_health_check(self, healthcheck: Healthcheck) -> ContainerRequest<Self::Image>` で `impl Into<Healthcheck>` は取らない
- Linux 側は `linux_unsupported_request_reason` (`src/runners/async_runner.rs:604-637`) で `with_platform` / `with_network` / `with_host` などを start 前に fail-fast する枠がある。macOS 側にはこれの対称関数は現時点で存在しない (`grep -n "macos_unsupported" src/` は 0 件)
- macOS 側の start (`src/runners/async_runner.rs:52-247` の `#[cfg(target_os = "macos")]` ブロック内、62-66 行で `let client = match client { Client::MacOs(c) => c, ... }`、68 行で `let descriptor = container_req.descriptor();`)。Linux 側 (`src/runners/async_runner.rs:250-330`) の Client 分岐は 253-258 行

### Docker Engine API 側

- Docker Engine API には完全な口がある: create JSON の `Config.Healthcheck` (`Test` / `Interval` / `Timeout` / `Retries` / `StartPeriod` / `StartInterval`) と、inspect (`GET /containers/{id}/json`) の `State.Running` (bool) / `State.Status` (string) / `State.Health.Status` (string)。時間系フィールドはすべて nanosecond の int64
- Docker Engine 仕様: `Config.Healthcheck` の各時間フィールドは `0` を送ると「未指定 (親イメージ or Docker Engine 既定を継承)」の意味。したがって `None` と `0` は Docker 側では等価に扱われる。ただし本 issue では利用者が `Duration::ZERO` を明示的に渡した意図を rustc の `Option::Some` の意味論として尊重し、`Some(Duration::ZERO)` は `0` を出す (`None` はキーごと省略)。実挙動は Docker 側で同じ
- `StartInterval` は Docker Engine API v1.44 (Docker 25.0, 2024 年 1 月) 以降のみ。旧エンジンは未知キーを黙って無視するため、`Option::None` のときにキーごと省略する実装であれば旧エンジンとも動く。統合テストは Docker 25.0 以降が動く環境を前提とし、CI runner (`.github/workflows/ci.yml` の `ubuntu-24.04`) は Docker 27 系プリインストールでこれを満たす
- `State.Health.Status` の既知値: `starting` / `healthy` / `unhealthy`。加えて Docker Engine 実装 (`moby/moby/daemon/health.go`) は `types.NoHealthcheck` 定数 `"none"` を healthcheck を起動しない判定に使う。実測 (`docker inspect` on `docker run --health-cmd=NONE ...`) では `Test=["NONE"]` 指定時に `State.Health` フィールド自体が inspect 応答に現れない。ただし Docker Engine のバージョンや設定によって `State.Health.Status = "none"` の文字列が露出する可能性を排除できないため、本 issue の実装では `"none"` を「healthcheck 未定義」相当として `probe.health = None` に落とす扱いに統一する (下記「health 取得インフラ」参照)

### 既存 issue との干渉

- `0010-refactor-remove-unused-types`: `HealthWaitStrategy::poll_interval` / `with_poll_interval` を「待機処理で読まれない」ことを根拠に削除対象としている。本 issue の Linux 実装で inspect のポーリング間隔として使われる側になるため、0010 の該当項目 1 件のみが事実と乖離する (0010 の他 2 項目 `CgroupnsMode` / 未使用 `Display` impl は独立)
- `0009-other-dead-error-variants-policy`: `WaitContainerError::Unhealthy` / `StateUnavailable` を「構築経路なし」と整理している。本 issue で `Unhealthy` に構築経路が生まれ shape を変える。`StateUnavailable` は本 issue でも構築しない (inspect 応答に `State` フィールドが無いのは Docker Engine 側の異常で、既存 `container_state` と同じく `ClientError::Json` に寄せる)
- `pending/0002-add-macos-healthcheck-xpc.md`: 本 issue マージ後は「`Healthcheck` 型と `with_health_check` は本クレートに無い」「`HealthWaitStrategy::wait_until_ready` は OS 非依存で常にエラー」の 2 行が事実誤認になる

本 issue のブランチでは 0009 / 0010 / pending 0002 の issue ファイル本文の書き換えは行わない (別カテゴリの作業になるため)。マージ後にこれら 3 件を `refresh-issue` の対象として扱う。担保手段は PR 本文末尾の `After merge: /refresh-issue 9 10 pending/0002` 明記。

## 設計方針

### `Healthcheck` 型 (新規、両 OS で公開)

配置は `src/core/healthcheck.rs`。`src/core.rs` に `pub mod healthcheck;` を追加した上で `src/core.rs` の `pub use self::{...}` 集約リストに `healthcheck::Healthcheck` を、`src/lib.rs` の `pub use crate::core::{...}` 集約リストに `Healthcheck` を追加する。`CODEBASE.md` L3 のトレイト列挙は変更不要 (`Healthcheck` は struct)。`pub use` の追加は `CODEBASE.md` L6 の許可 (「クレート公開面の `pub use` re-export を許可」) の枠内。

本家 testcontainers-rs 0.27 (`https://github.com/testcontainers/testcontainers-rs/blob/testcontainers-0.27.0/testcontainers/src/core/image/healthcheck.rs`) と 1:1 で以下を実装する (`impl Into<Healthcheck>` などの独自拡張はしない):

- `pub struct Healthcheck { test: Vec<String>, interval: Option<Duration>, timeout: Option<Duration>, retries: Option<u64>, start_period: Option<Duration>, start_interval: Option<Duration> }`
- コンストラクタ: `pub fn none() -> Self` (`test = ["NONE"]`)、`pub fn empty() -> Self` (`test = []`)、`pub fn cmd_shell(cmd: impl Into<String>) -> Self` (`test = ["CMD-SHELL", cmd]`)、`pub fn cmd<I, S>(cmd: I) -> Self where I: IntoIterator<Item = S>, S: Into<String>` (`test = ["CMD", ...cmd]`)
- ビルダー: `pub fn with_interval(mut self, d: Duration) -> Self` / `with_timeout` / `with_retries(u64)` / `with_start_period(Duration)` / `with_start_interval(Duration)`
- accessor: `test(&self) -> &[String]` / `interval(&self) -> Option<Duration>` / `timeout(&self) -> Option<Duration>` / `retries(&self) -> Option<u64>` / `start_period(&self) -> Option<Duration>` / `start_interval(&self) -> Option<Duration>`
- derive は `Debug, Clone, PartialEq, Eq` (本家一致)
- `with_start_interval` の doc comment に `/// Docker Engine 25.0 (API v1.44) 未満では未知キーとして無視される。` を明示する

本家の `pub(crate) fn into_health_config()` は bollard の `HealthConfig` に依存するため取り込まない。代替として Linux 専用 (`#[cfg(target_os = "linux")]`) の `pub(crate) fn to_docker_json(&self) -> Option<String>` を実装する。返り値を `Option` にする理由: `test.is_empty()` かつ他フィールドがすべて `None` の場合は Docker Engine 側で image inherit と等価になるため、`Healthcheck` オブジェクト自身を出さず `None` を返す (呼び出し元の `to_json_string` が `,"Healthcheck":...` の付加を丸ごとスキップする)。それ以外は JSON オブジェクトの文字列を `Some(...)` で返す。

`to_docker_json` の中で `escape_json` / `json_array` を使うため、両ヘルパを `pub(crate)` に格上げする (`src/core/client/docker_client.rs:971` の `fn escape_json` と `json_array`)。`docker_client` モジュールは Linux 専用 (`src/core/client.rs:11` で `#[cfg(target_os = "linux")]`) のため、`src/core/healthcheck.rs` 内での `use` も `#[cfg(target_os = "linux")]` で gate する。

`to_docker_json()` の JSON 形式:

- `Test` は `Vec<String>` を JSON 配列にする。各要素は `escape_json` で escape (`"` / `\` / `\n` / マルチバイト文字を含む場合も正しく処理される)。`test.is_empty()` でもキー自体は出す (`empty()` の意味論 = `Test=[]` = image inherit を保つ)
- `Interval` / `Timeout` / `StartPeriod` / `StartInterval` は `Some(Duration)` の場合のみキーを出す。値は `Duration::as_nanos()` (`u128`) を `nanos.min(i64::MAX as u128) as i64` で飽和変換し int で出力 (nanosecond)。`Some(Duration::ZERO)` は明示的に `0` を出す (Docker Engine 側では `0` = 未指定と等価に扱われるが、rustc の `Option::Some` の意味論として明示 `0` と `None` を区別する)。`None` はキーごと省略
- `Retries` は `Some(u64)` のみ int で出す

### `ContainerRequest` の設定口 (両 OS で公開)

- `src/core/containers/request.rs:22-50` の `ContainerRequest<I>` に `pub(crate) health_check: Option<Healthcheck>` を追加する。struct 定義の順序は `mounts` (34 行) の直後に挿入する (機能グループ的に近い位置)
- `From<I> for ContainerRequest<I>` (232-264 行) の初期化に `health_check: None` を追加
- accessor: `pub fn health_check(&self) -> Option<&Healthcheck>` を追加 (本家 `docs/TESTCONTAINERS.md:280` に合わせる)
- `Debug` impl (283-312 行) に `health_check` フィールドを含める (struct 定義に合わせ `mounts` の直後に挿入)。既存の除外 3 件 (`copy_to_sources` / `ready_conditions` / `log_consumers`) は「サイズ・要素数が可変で診断ログが肥大化しうる」ため省略された前例。`Healthcheck` は最大 6 フィールドの固定サイズ struct でログ肥大化のリスクが無いため含める
- `src/core/image/image_ext.rs` の `ImageExt` trait 定義 (18-105 行) に `fn with_health_check(self, healthcheck: Healthcheck) -> ContainerRequest<Self::Image>;` を追加 (trait 側は `Self::Image` 戻り、本家 1:1)。impl (107-335 行) に `fn with_health_check(self, healthcheck: Healthcheck) -> ContainerRequest<I> { let mut container_req = self.into(); container_req.health_check = Some(healthcheck); container_req }` を追加 (impl 側は `I` 戻り、既存メソッドと同じ書き分け)

### Docker Engine API (Linux) への配線

- `ContainerConfig` (`src/core/client.rs:44-57`, `#[cfg(target_os = "linux")]` 枠) に `pub health_check: Option<Healthcheck>` を追加し、同ファイル頭 (Linux ゲート下) に `use crate::core::healthcheck::Healthcheck;` を追加する
- `build_container_config` (`src/runners/async_runner.rs:479-507`) の struct literal に `health_check: req.health_check().cloned(),` を追加する
- `CreateContainerBody` (`src/core/client/docker_client.rs:546-556`) に `healthcheck: Option<Healthcheck>` を追加し、`from_config` (559-605 行) で `config.health_check` を移送する
- `to_json_string` (607-654 行) の `HostConfig` 挿入位置 (650 行 `,"HostConfig":`) の直前に、`self.healthcheck.as_ref().and_then(|hc| hc.to_docker_json())` が `Some(s)` の場合のみ `,"Healthcheck":<s>` を追加する (`None` の場合は何も出さない)。`HostConfig::to_json_string` (665-681 行) は変更しない

### `WaitContainerError::Unhealthy` の shape 変更 (破壊的変更)

`src/core/error.rs` の 3 箇所を書き換える:

- variant 定義 (L206): `Unhealthy,` → `Unhealthy(String),`
- Display arm (L227): `WaitContainerError::Unhealthy => write!(f, "container is unhealthy"),` → `WaitContainerError::Unhealthy(s) => write!(f, "container is unhealthy: {s}"),`。既存の `HealthCheckNotConfigured(String)` の Display 書式 (`"healthcheck is not configured for container: {s}"`) と id を末尾に置く形で揃える。外側 `Error::WaitContainer` の Display (`src/core/error.rs:39`, `"container is not ready: {e}"`) と合成した結果は `"container is not ready: container is unhealthy: <id>"` になる (現状の `HealthCheckNotConfigured` 合成 `"container is not ready: healthcheck is not configured for container: <id>"` と同じく "container" 2 回、精度と precedent 一致を優先)
- source arm (L248): `WaitContainerError::Unhealthy => None,` → `WaitContainerError::Unhealthy(_) => None,`

これは公開エラー型の SemVer 破壊的変更 (unit variant → tuple variant への切替。`match WaitContainerError::Unhealthy => ...` は破綻し `Unhealthy(_)` への書き換えが必要)。

### health 取得インフラ (Linux 専用)

`ContainerSnapshot` (`src/core/client.rs:37-40`) は変更しない。`src/core/client.rs` の Linux ゲート下に以下を追加する:

- `pub(crate) enum HealthStatus { Starting, Healthy, Unhealthy }` (derive: `Debug, Clone, Copy, PartialEq, Eq`)
- `pub(crate) struct HealthProbe { pub(crate) running: bool, pub(crate) health: Option<HealthStatus> }` (derive: `Debug, Clone`)

`DockerClient::container_health(&self, id: &str) -> Result<HealthProbe>` を新設する (エラー型は既存 `container_state` と同じ `crate::core::error::Result<T>` エイリアス)。実装:

- `GET /containers/{id}/json` を叩く。既存 `container_state` の inspect と同じエンドポイントだが抽出フィールドが異なる (running + ports vs running + health status)。`State.Running` (bool) は既存 `container_state` の抽出コード (`src/core/client/docker_client.rs:352-357`) と同じ形式で取り出す (running 判定を二本立てにしない。将来的な共通化余地はあるが本 issue では既存 `container_state` の抽出を独立関数化まではせず、同じロジックを新関数側にも書く。差異が生じないよう code review で確認)
- `State` フィールド自体が inspect 応答から欠落した場合は既存 `container_state` と同じく `.required()?` で `ClientError::Json` に寄せる
- HTTP status: 404 (`ClientError::ContainerNotFound`) / 400 以上 (`ClientError::Other`, 既存 `container_state` の `status_code >= 400` 分岐と同じ) / JSON 破損 (`ClientError::Json`) はそのまま `Err(_)` で返す
- `State.Health.Status` の文字列マップ:
  - `"starting"` → `HealthStatus::Starting`
  - `"healthy"` → `HealthStatus::Healthy`
  - `"unhealthy"` → `HealthStatus::Unhealthy`
  - `"none"` / 空文字 → `health = None` として扱う (Docker Engine 実装依存の値で「healthcheck 未定義」相当の意味論。`Test=["NONE"]` 指定時に `State.Health` フィールドが不在になる主経路と同じ扱いに統一する)
  - その他未知文字列 → `health = None` として扱う (未来の Docker 実装追加に前向きに堅牢化するより、既存挙動 `HealthCheckNotConfigured` に落として StartupTimeout を待たせない側を優先)
- `State.Health` フィールドが inspect 応答に無ければ `health = None`

### 待機 (`HealthWaitStrategy::wait_until_ready` の Linux 分岐)

`src/core/wait/health_strategy.rs:37-52` の `wait_until_ready` を OS 分岐に書き換える。パラメータ名 `_client: &Client` は Linux 分岐で実際に使うため `client: &Client` に rename する。macOS 分岐 (`#[cfg(target_os = "macos")]`) では `let _ = client;` で unused 警告を抑制し、現行通り `Err(WaitContainerError::HealthCheckNotConfigured(container.id().to_string()).into())` を返す。

Linux 分岐 (`#[cfg(target_os = "linux")]`) の擬似コード (precedent は `src/runners/async_runner.rs:253-258` の Linux ブロック):

```
#[expect(clippy::infallible_destructuring_match)]
let docker = match client {
    Client::Linux(c) => c,
    #[cfg(target_os = "macos")]
    Client::MacOs(_) => unreachable!("Linux block is not compiled on macOS"),
};
let id = container.id().to_string();
let mut seen_running = false;
loop {
    let probe = docker.container_health(&id).await?;
    if probe.running {
        seen_running = true;
    }
    match probe.health {
        Some(HealthStatus::Healthy) => return Ok(()),
        Some(HealthStatus::Unhealthy) => {
            return Err(WaitContainerError::Unhealthy(id).into());
        }
        Some(HealthStatus::Starting) => {
            // Docker が判定中。次の tick へ
        }
        None => {
            if seen_running {
                // running=true を一度でも観測した後の tick で State.Health 不在 →
                // healthcheck 未設定 (Dockerfile HEALTHCHECK も with_health_check も無い、
                // Test=["NONE"] で明示無効化、その他 Docker 実装依存の "none"/未知値 相当) と確定
                return Err(WaitContainerError::HealthCheckNotConfigured(id).into());
            }
            // まだ running を観測していないうちは Docker daemon 側の health monitor 初期化ラグの可能性が高いため継続
        }
    }
    tokio::time::sleep(self.poll_interval).await;
}
```

`id: String` は `return Err(...)` の各 arm で move されるが、両 arm とも発散するためコンパイル成立する (rustc NLL)。将来「1 回だけリトライ」等の分岐を追加する場合は `id.clone()` を各 arm で使う想定。

全体タイムアウトは呼び出し元 `run_ready_sequence` (`src/runners/async_runner.rs:410-434`) の `tokio::time::timeout(startup_timeout, block_until_ready)` に委ねる (`WaitContainerError::StartupTimeout` に集約)。`HealthWaitStrategy` に固有の全体タイムアウト値は持たない。`poll_interval` (既定 `Duration::from_millis(100)`) は inspect (`GET /containers/{id}/json`) を叩く間隔。Docker Engine 側 `Config.Healthcheck.Interval` (コンテナ内でプローブを実行する間隔、既定 30 秒) とは別軸。

`src/core/wait/health_strategy.rs:1-4` と `src/core/wait/mod.rs:5` の doc comment を「Linux は inspect ポーリング (`starting` / `healthy` / `unhealthy` / `Health` 不在 = running 後は `HealthCheckNotConfigured`)。macOS は現行通り常に `HealthCheckNotConfigured`」に書き換える。

### `Healthcheck::none()` / `empty()` / Dockerfile HEALTHCHECK

- `Healthcheck::none()` (`Test=["NONE"]`): Docker Engine 側でイメージ HEALTHCHECK を無効化。`State.Health` は生成されない (または実装依存で `"none"` 文字列が返るが、上記マップで `health = None` に統一)。Linux 分岐は running 後 tick で `HealthCheckNotConfigured` を返す
- `Healthcheck::empty()` (`Test=[]`): Docker Engine 側でイメージ HEALTHCHECK の test 定義を継承しつつ他フィールドだけ user 上書き。イメージに HEALTHCHECK が無い場合は `State.Health` が生成されず `HealthCheckNotConfigured`、ある場合はイメージの test でポーリング可能
- `Healthcheck::cmd_shell(...)` / `cmd(...)`: user 定義の Test を使う。Docker Engine の仕様通り create の `Config.Healthcheck` が Dockerfile HEALTHCHECK を上書きする
- Dockerfile HEALTHCHECK があるイメージに `with_health_check` を呼ばずに `WaitFor::healthcheck()` だけ指定した場合: inspect の `State.Health` にイメージ由来のステータスが載る

### macOS の明示エラー (fail-fast)

macOS 側の `AsyncRunner::start` の `#[cfg(target_os = "macos")]` ブロック内、`let client = match client { ... }` の直後 (`src/runners/async_runner.rs:66` の match 閉じ直後、`let descriptor = container_req.descriptor();` である 68 行の直前) に、以下を直接書く:

```
if container_req.health_check().is_some() {
    return Err(crate::Error::other("with_health_check() is not supported on macOS"));
}
```

Linux 側の `linux_unsupported_request_reason` (`src/runners/async_runner.rs:604-637`) の呼び出し位置 (261-263 行) と対称の位置。macOS 側の rejection 汎用関数化は本 issue のスコープ外とし 1 行 inline で済ませる。

### `SyncRunner` (`blocking` feature) との関係

`SyncRunner::start` は `AsyncRunner::start` に委譲する (`src/runners/sync_runner.rs`) ため、`with_health_check` の追加は sync 側でも通り、macOS 側の fail-fast も sync 経由で発火する。

## 完了条件

### 型・設定口 (両 OS)

- [ ] `src/core.rs` に `pub mod healthcheck;` を追加し、`pub use self::{...}` に `healthcheck::Healthcheck` を追加。`src/lib.rs` の `pub use crate::core::{...}` に `Healthcheck` を追加
- [ ] `src/core/healthcheck.rs` に `Healthcheck` 型が新設され、上記シグネチャ (コンストラクタ 4 種・ビルダー 5 種・accessor 6 種) を満たす。derive は `Debug, Clone, PartialEq, Eq`。`with_start_interval` の doc comment に「Docker Engine 25.0 (API v1.44) 未満では未知キーとして無視される」を明示
- [ ] `Healthcheck` の単体テスト (両 OS 実行):
  - [ ] 各コンストラクタ (`none` / `empty` / `cmd_shell` / `cmd`) が期待の `test` 配列を持つ
  - [ ] 各ビルダーで設定した値が対応 accessor から取り出せる
- [ ] `ImageExt::with_health_check(self, healthcheck: Healthcheck)` が trait 定義 (`Self::Image` 戻り) と impl (`I` 戻り) の両方に追加されている
- [ ] `ContainerRequest` に `health_check: Option<Healthcheck>` フィールド (struct 定義で `mounts` の直後)、`health_check() -> Option<&Healthcheck>` accessor、`Debug` impl 反映 (`mounts` 出力の直後)、`From<I>` 初期化が入っている

### Docker Engine API 配線 (Linux)

- [ ] `escape_json` / `json_array` (`src/core/client/docker_client.rs`) が `pub(crate)` に格上げされている
- [ ] `Healthcheck::to_docker_json()` が `#[cfg(target_os = "linux")]` で実装され、返り値は `Option<String>`。`test.is_empty() && interval.is_none() && timeout.is_none() && retries.is_none() && start_period.is_none() && start_interval.is_none()` のときのみ `None`、それ以外は `Some(JSON 文字列)` を返す。`use crate::core::client::docker_client::{escape_json, json_array};` は `#[cfg(target_os = "linux")]` で gate する
- [ ] `to_docker_json` の単体テストが `#[cfg(all(test, target_os = "linux"))]` mod で追加され、以下を検証する:
  - [ ] `["CMD-SHELL", ...]` / `["CMD", ...]` / `["NONE"]` / `[]` の各形態が JSON 配列として正しく出力
  - [ ] `Duration::from_secs(30).as_nanos()` が `30000000000` として出力
  - [ ] `Some(Duration::ZERO)` は `0` を出力
  - [ ] `None` フィールドはキー省略
  - [ ] `Test` 要素の `"` / `\` / `\n` / マルチバイト文字が正しく escape
  - [ ] `Healthcheck::empty().with_interval(Duration::from_secs(5))` が `{"Test":[],"Interval":5000000000}` を出力 (`test.is_empty()` かつ他フィールド Some のケース、image inherit + user 上書きの主要 use case)
  - [ ] `Healthcheck::default()` 相当 (全フィールド `None` / `test.is_empty()`) で `None` を返す
- [ ] `ContainerConfig` (Linux ゲート下) に `health_check: Option<Healthcheck>` フィールドと `use crate::core::healthcheck::Healthcheck;` が追加されている
- [ ] `build_container_config` (`src/runners/async_runner.rs:479-507`) の struct literal に `health_check: req.health_check().cloned(),` が追加されている
- [ ] `CreateContainerBody` と `to_json_string` に Healthcheck が配線され、生成される Config JSON の `"HostConfig":` 直前に `"Healthcheck":<json>` が挿入される (`to_docker_json` が `None` を返した場合は何も出力しない)

### エラー型変更

- [ ] `src/core/error.rs` の 3 箇所 (variant 定義 L206、Display L227、source L248) が書き換わり、`Unhealthy` が `Unhealthy(String)` として構築される
- [ ] 破壊的変更の下流影響確認: リポジトリ全体を `grep -rn "WaitContainerError::Unhealthy" .` して既存の match/構築箇所を洗い出し、書き換え漏れが無いこと (現状は match 箇所ゼロだが、確認手順として実行)

### health 取得インフラ (Linux)

- [ ] `src/core/client.rs` の Linux ゲート下に `HealthStatus` enum (`Starting/Healthy/Unhealthy`, derive `Debug, Clone, Copy, PartialEq, Eq`) と `HealthProbe` struct (`running: bool, health: Option<HealthStatus>`, derive `Debug, Clone`) が定義されている
- [ ] `DockerClient::container_health(&self, id: &str) -> Result<HealthProbe>` が新設され、`State.Running` (bool) と `State.Health.Status` (`"starting"` / `"healthy"` / `"unhealthy"` を enum に、`"none"` / 空文字 / 既知外文字列は `health = None`、`State.Health` フィールド不在も `health = None`) を同一 inspect レスポンスから抽出する。`State` フィールド不在は `ClientError::Json`、404 は `ClientError::ContainerNotFound`、`>= 400` は `ClientError::Other`

### 待機ロジック

- [ ] `HealthWaitStrategy::wait_until_ready` のパラメータ名が `_client` から `client` に変更され、Linux 分岐 (`#[cfg(target_os = "linux")]`) が上記擬似コード通り動く (`match` は precedent `src/runners/async_runner.rs:253-258` と同型で `#[expect(clippy::infallible_destructuring_match)]` + `#[cfg(target_os = "macos")] Client::MacOs(_) => unreachable!(...)` アーム付き)
- [ ] macOS 分岐 (`#[cfg(target_os = "macos")]`) は現行通り `HealthCheckNotConfigured` を返し、`let _ = client;` で unused 警告を抑制する
- [ ] `src/core/wait/health_strategy.rs:1-4` と `src/core/wait/mod.rs:5` の doc comment を「Linux は inspect ポーリング (`starting` / `healthy` / `unhealthy` / `Health` 不在 = running 後は `HealthCheckNotConfigured`)。macOS は現行通り常に `HealthCheckNotConfigured`」に書き換える

### macOS fail-fast

- [ ] `src/runners/async_runner.rs` の macOS ブロック (68 行の直前) に `if container_req.health_check().is_some() { return Err(crate::Error::other("with_health_check() is not supported on macOS")); }` が inline で追加されている

### 統合テスト

Docker Engine が稼働している環境で `cargo test --all-features` を実行する前提。`tests/container_linux.rs` に 5 本追加 (すべて `alpine:latest` を使い、`sh -c` 経由で常駐 CMD を明示):

- [ ] healthy 到達 (async): `with_cmd(["sh", "-c", "touch /tmp/ok; while true; do sleep 60; done"])` + `Healthcheck::cmd_shell("test -f /tmp/ok").with_interval(Duration::from_millis(500)).with_retries(3)` + `WaitFor::healthcheck()` で ready
- [ ] healthy 到達 (sync, `#[cfg(feature = "blocking")]`): async 版と同じシナリオを sync 版で 1 本
- [ ] unhealthy: `with_cmd(["sh", "-c", "while true; do sleep 60; done"])` + `Healthcheck::cmd_shell("false").with_interval(Duration::from_millis(500)).with_timeout(Duration::from_secs(1)).with_retries(1)` + `WaitFor::healthcheck()` が `WaitContainerError::Unhealthy(_)` を返す
- [ ] healthcheck 未設定: `with_cmd(["sh", "-c", "while true; do sleep 60; done"])` (alpine には Dockerfile HEALTHCHECK が無い) + `with_health_check` を呼ばずに `WaitFor::healthcheck()` を指定し、`WaitContainerError::HealthCheckNotConfigured(_)` を返す
- [ ] startup_timeout: `with_cmd(["sh", "-c", "while true; do sleep 60; done"])` + `Healthcheck::cmd_shell("sleep 120").with_interval(Duration::from_secs(1)).with_timeout(Duration::from_secs(60)).with_retries(10).with_start_period(Duration::from_secs(120))` + `with_startup_timeout(Duration::from_secs(10))` + `WaitFor::healthcheck()` で `WaitContainerError::StartupTimeout` を返す。テストの成立根拠: probe コマンド (`sleep 120`) は 120 秒経過するまで返らないため 1 回目のプローブ結果が出るのは早くても t=1 (interval) + 120 (プローブ実行) = 121 秒後。それより十分早い t=10 秒 で `tokio::time::timeout(startup_timeout, ...)` が発火する。`start_period=120s` は境界ケースを避けるため十分大きく取り、`timeout=60s` は probe コマンド (`sleep 120`) より短くしてタイムアウト境界にも触れない
- [ ] macOS fail-fast (`tests/container_macos.rs` に async 1 本 + `tests/container_sync_drop_macos.rs` 系の既存ファイルまたは新規 `tests/container_sync_healthcheck_macos.rs` に sync 1 本): `with_health_check(Healthcheck::cmd_shell("true"))` を指定した `.start()` が `"with_health_check() is not supported on macOS"` を含むエラーで即座に失敗する

### 触らないことの確認

- [ ] `WaitContainerError::StateUnavailable` について本 issue では触らない (0009 の refresh-issue で扱う)。`grep -rn "StateUnavailable" src/` で新規参照が生えていないこと

### `docs/TESTCONTAINERS.md` 更新

判定用語辞書 (`docs/TESTCONTAINERS.md:13-20`) の「対応 / 部分対応 / 未配線 / 未反映 / 未実装 / 未実装 (XPC 制約) / なし / shiguredo 拡張」に厳密に従う。Apple 列は「XPC route / 仕様が無い」ことに起因する項目は「未実装 (XPC 制約)」で統一する (「シグネチャあり + fail-fast」は応答形式であって根本理由ではないため、辞書を使い分けない)。行番号はマージ前に `grep -n` で再確認する。

- [ ] 2 章 `with_health_check(self, hc)` 行 (現状 L147): Docker 列「なし」→「対応」、Apple 列「なし」→「未実装 (XPC 制約)」、備考は「Linux は Config.Healthcheck に配線。macOS は start 時に `Err("with_health_check() is not supported on macOS")`」
- [ ] 8 章 `health_check(&self) -> Option<&Healthcheck>` accessor 行 (現状 L280): Docker 列「なし」→「対応」、Apple 列「なし」→「対応」(accessor 自体は macOS でも呼べる)
- [ ] 9 章 `WaitFor::Healthcheck(HealthWaitStrategy)` 行 (現状 L301): Docker 列「未実装」→「対応」、備考の Docker 記述を「Linux は Healthy/Unhealthy/Starting/None (running 後) の 4 分岐」に更新
- [ ] 9 章 `pub fn healthcheck()` 行 (現状 L308): Docker 列「未実装」→「対応」、備考の「Docker: 常に HealthCheckNotConfigured (OS 非依存)」を「Docker: Linux は inspect ポーリング、macOS は with_health_check で即エラー」に更新
- [ ] 10.2 節 `HealthWaitStrategy` 表 (現状 L329-336):
  - [ ] `pub fn with_poll_interval` 行 (現状 L334): Docker 列は既存「対応」を維持、備考に「Linux では inspect のポーリング間隔として利用」を追記
  - [ ] `wait_until_ready impl` 行 (現状 L336): Docker 列「未実装」→「対応」、備考の「Docker: OS 非依存で常に HealthCheckNotConfigured」を「Docker: Linux は inspect ポーリング (`starting` / `healthy` / `unhealthy` / `Health` 不在は running 後 `HealthCheckNotConfigured`)、macOS は `HealthCheckNotConfigured` 維持」に更新
- [ ] 16.4 節 `Healthcheck` 型全表 (現状 L622-637): Docker 列を行ごとに個別判定 (型定義行・コンストラクタ・ビルダー・accessor の 12 行は「なし」→「対応」、`pub(crate) fn into_health_config()` 行は「なし」を維持し備考を「shiguredo は `to_docker_json()` に置換 (bollard 非依存)」)。Apple 列は行ごとに個別判定 (型定義・コンストラクタ・ビルダー・accessor は macOS でもコンパイル・実行可能なため「なし」→「対応」、`pub(crate) fn into_health_config()` は「なし」維持)。表全体末尾に新規行 `| pub(crate) fn to_docker_json() -> Option<String> | なし | なし | 対応 | shiguredo 拡張。Docker Config.Healthcheck JSON を生成 (Linux 専用) |` を追加。型定義行の備考「Apple container はカスタム healthcheck 未対応」は「Apple container はカスタム healthcheck 未対応 (`with_health_check` は macOS で start 時に明示エラー)」へ改訂
- [ ] 17.6 節 `WaitContainerError` 表 (現状 L702): `Unhealthy` 行の API 列を `Unhealthy(String)` に書き換える
- [ ] Apple Container サマリ (現状 L68-82) の件数は今回の変更に沿って概算で更新する (正確な再集計は refresh-issue で扱う)
- [ ] 24 章 `pub use` (現状 L804): 備考「`Healthcheck` / `BuildableImage` は shiguredo に無し。`ExecCommand` / `WaitFor` は shiguredo 独自に追加」を「`BuildableImage` は shiguredo に無し。`Healthcheck` は 16.4 節参照 (Linux 対応、macOS は accessor 対応・`with_health_check` は明示エラー)。`ExecCommand` / `WaitFor` は shiguredo 独自に追加」に書き換え。セル判定は現状のまま
- [ ] 「実装不可 (XPC 制約)」節 (現状 L813-828、24 章とは独立の `##` トップレベル節) の表 (L820-821) から `Healthcheck` 型と `ImageExt::with_health_check` の行を除外する。`HealthWaitStrategy` 行は残す (macOS 分岐は現行の `HealthCheckNotConfigured` を返すのみで、XPC route が無いことに起因して「実装できない」根本要因は変わらない)。除外した 2 行の内容は同節の直後 (「実装不可 (XPC 制約)」節と「意図的に保持する shiguredo 拡張」節 (現状 L830) の間) に新規 `##` トップレベル節「シグネチャあり・macOS では明示エラー」を追加してそこに移す (24.4 サブ節にはしない。24 章はサブ節を持たないフラットな 1 表構成のため)。新規節の説明文は「Apple container の XPC には対応 route が無いが、本家 API 互換のためシグネチャは公開し、start 時に明示エラーを返す項目」

### README

- [ ] `README.md:27` の Linux 対応の列挙「ライフサイクル (start / exec / stop / rm / Drop) に加えログ関連 (stdout / stderr / ログ待機 / LogConsumer) とファイルコピー (`copy_file_from` / `with_copy_to`) も動くが」に「とヘルスチェック待機 (`with_health_check` / `WaitFor::healthcheck`)」を追記して「ライフサイクル ... とログ関連 ... とファイルコピー ... とヘルスチェック待機 (`with_health_check` / `WaitFor::healthcheck`) も動くが」に変更する

### CHANGES.md

`## develop` セクションに以下 2 エントリを追加する (shiguredo-changelog 規約準拠。並び順は CHANGE → ADD → UPDATE → FIX、newest-first)。担当者行はエントリ直後に 2 スペースインデントで置く:

- [ ] `[CHANGE]` 群の末尾に以下を追加:

  ```
  - [CHANGE] `WaitContainerError::Unhealthy` を `Unhealthy(String)` 形式に変更する
    - @voluntas
  ```

- [ ] `[ADD]` 群の先頭 (`[CHANGE]` 群の直下、既存の 0043 precedent の直上) に以下を追加:

  ```
  - [ADD] Linux で `Healthcheck` / `ImageExt::with_health_check` / `HealthWaitStrategy` の Linux 分岐に対応する
    - @voluntas
  ```

### CI

- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass する

### PR 本文

- [ ] PR 本文の末尾に `After merge: /refresh-issue 9 10 pending/0002` を明記する (0009 / 0010 / pending 0002 は本 issue のブランチでは触らず、マージ後に refresh-issue の対象として扱うため)
- [ ] PR 本文に「マージ後に mqtt-rs 側の EMQX SCRAM テストで `with_health_check(Healthcheck::cmd_shell(...))` を導入する」を after-merge task として明記する (本 issue の動機を実利用に接続する。mqtt-rs 側の作業は container-rs リポジトリ外)

## 解決方法

未着手 (open)。
