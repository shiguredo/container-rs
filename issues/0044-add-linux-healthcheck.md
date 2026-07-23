# 機能追加: Linux で Healthcheck / with_health_check / WaitFor::Healthcheck を実装する

- Priority: Medium
- Created: 2026-07-23
- Completed: {YYYY-MM-DD}
- Model: Claude Fable 5
- Branch: feature/add-linux-healthcheck
- Polished: {YYYY-MM-DD}
- Reporter: @voluntas

## 目的

Linux (Docker Engine API) バックエンドで、本家 testcontainers 0.27 互換の `Healthcheck` 型・`ImageExt::with_health_check`・`WaitFor::healthcheck()` の待機を実装する。

利用者フィードバック (mqtt-rs の EMQX SCRAM テスト) で、HTTP wait だけでは「Dashboard が応答する」までしか保証できず、`/status` が 200 を返しても SCRAM provider が未登録で `no_available_provider_for` になるレースが CI で発生した。本家なら次のようにアプリ固有の ready をコンテナ側に寄せられる。

```rust
.with_health_check(
    Healthcheck::cmd_shell("...")  // 認証 API が受け付けるまで
        .with_interval(...)
)
.with_wait_for(WaitFor::healthcheck())
```

EMQX 公式イメージ自体に HEALTHCHECK は無いため、イメージ側ではなく利用側の設定口 (`with_health_check`) が本丸。

## 優先度根拠

利用者フィードバック (mqtt-rs の CI の失敗の直接原因)。テスト側の認証 API リトライで回避は可能だが、本家にあるギャップの中で mqtt-rs 観点の効きが最も大きい。macOS は Apple container の XPC に口が無く不可 (pending `0002`) だが、CI が走る Linux だけでも価値が高い。Medium。

## 現状

- `Healthcheck` 型と `ImageExt::with_health_check` はクレートに存在しない (`src/core/image/image_ext.rs` のトレイト定義に無い)
- `HealthWaitStrategy::wait_until_ready` は OS 非依存で無条件に `WaitContainerError::HealthCheckNotConfigured` を返す (`src/core/wait/health_strategy.rs:38-51`)。`poll_interval` フィールドは保持しているが読まれていない
- `pending/0002-add-macos-healthcheck-xpc.md` は macOS 限定で「Apple が XPC に口を用意するまで保留」。Linux 側の issue は無い
- Docker Engine API には完全な口がある: create JSON の `Config.Healthcheck` (`Test` / `Interval` / `Timeout` / `Retries` / `StartPeriod`) と、inspect (`GET /containers/{id}/json`) の `State.Health.Status` (`starting` / `healthy` / `unhealthy`)
- `DockerClient::container_state` (`src/core/client/docker_client.rs:327`) が既に inspect を叩いているが、`ContainerSnapshot` は `running` / `ports` のみで health を読んでいない
- `build_container_config` (`src/runners/async_runner.rs:476`) と `ContainerConfig` に healthcheck の口が無い
- macOS 側の非対応 `ImageExt` を明示エラーにする前例がある (`with_host(..., HostGateway)` の `src/runners/async_runner.rs:651`)

### 既存 issue との干渉

- `0010-refactor-remove-unused-types`: `HealthWaitStrategy::poll_interval` / `with_poll_interval` を「待機処理で読まれない」ことを根拠に削除対象としている。本 issue の Linux 実装でポーリング間隔として使われる側に変わるため、0010 の該当項目は取り下げまたは修正が必要
- `0009-other-dead-error-variants-policy`: `WaitContainerError::Unhealthy` / `StateUnavailable` を「構築経路なし」と整理している。本 issue で `Unhealthy` に構築経路が生まれる

## 設計方針

- `Healthcheck` 型を追加する。コンストラクタ (`cmd` / `cmd_shell`) とビルダーメソッド (`with_interval` / `with_timeout` / `with_retries` / `with_start_period` 等) のシグネチャは実装時に本家 testcontainers 0.27 と照合して一致させる
- `ImageExt::with_health_check` を追加し、`ContainerRequest` に保存して Linux の `build_container_config` で create JSON の `Config.Healthcheck` に反映する
- `HealthWaitStrategy::wait_until_ready` に Linux 分岐を実装する: inspect の `State.Health.Status` を `poll_interval` 間隔でポーリングし、`healthy` で `Ok`、`unhealthy` で `WaitContainerError::Unhealthy`、`Health` フィールド自体が無い (healthcheck 未設定) 場合は現行どおり `HealthCheckNotConfigured`。全体のタイムアウトは既存の ready 待機 (`startup_timeout`) の枠に乗る
- health の取得は `ContainerSnapshot` への `health` フィールド追加または専用メソッドとし、既存の `container_state` の inspect 呼び出しを流用する
- macOS: `with_health_check` 指定時は黙って無視せず明示エラーにする (`with_host(..., HostGateway)` と同型)。`HealthWaitStrategy` の macOS 分岐は現行の `HealthCheckNotConfigured` を維持する
- イメージ側 HEALTHCHECK (Dockerfile 由来) しか無い場合も、inspect に `Health` が載るため同じポーリングで待てる (with_health_check 必須にはしない)

## 完了条件

- Linux で `with_health_check` + `WaitFor::healthcheck()` を使ったコンテナ起動が、healthcheck 通過まで待ってから ready になる統合テストが pass する (healthy 到達・unhealthy での `Unhealthy` エラー・healthcheck 未設定での `HealthCheckNotConfigured` の 3 経路)
- macOS で `with_health_check` 指定時に明示エラーが返る
- `docs/TESTCONTAINERS.md` の Healthcheck 該当節 (10.2) が Linux 対応済み・macOS 非対応に更新されている
- 0010 / 0009 の干渉項目 (poll_interval 削除・Unhealthy dead variant) について、各 issue 側の記述を更新している
- `CHANGES.md` に `[ADD]` エントリがある
- `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass する

## 解決方法

`Healthcheck` 型と `ImageExt::with_health_check` を追加し、`build_container_config` で `Config.Healthcheck` JSON に配線する。`HealthWaitStrategy::wait_until_ready` の Linux 分岐で inspect の `State.Health.Status` をポーリングする。EMQX 相当の「HTTP は応答するがアプリ固有 ready は別」ケースを模した統合テスト (シェルコマンドの healthcheck) を追加する。
