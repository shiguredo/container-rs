# リファクタリング: 未使用の型・メソッド (CgroupnsMode / 未使用 Display impl / poll_interval) を整理する

- Priority: Low
- Created: 2026-07-12
- Completed:
- Model: Kimi
- Branch: feature/refactor-remove-unused-types
- Polished: 2026-07-21

## 目的

本家 testcontainers-rs とのシグネチャ互換のために定義したものの、構築・参照ともに一度も行われていない型・メソッド・フィールドが公開 API として残っている。利用者に「使える / 効いている」という誤解を与えるため、各項目について削除するか意図的保持 (判断をコメント等で明記) するかを決めて整理する。対象は次の 3 点:

- `CgroupnsMode` (`src/core/containers/request.rs`)
- 一度も呼ばれない `Display` impl (`MountType` / `AccessMode` / `ExtraHost`)
- `HealthWaitStrategy::with_poll_interval` と `poll_interval` フィールド (`src/core/wait/health_strategy.rs`)

## 優先度根拠

機能上の欠陥は無く、すべて死にコードと API 形状の正直さの問題であるため Low。ただし `with_poll_interval` は「設定したのに効かない」という静かな誤解を生むため、次回の公開 API 整理のタイミングで片付けておきたい。

## 現状

### CgroupnsMode

- `src/core/containers/request.rs` に `CgroupnsMode` (Host / Private) が定義され、`src/core/containers/mod.rs` と `src/core.rs` で公開 re-export されている
- しかし `ImageExt` に `with_cgroupns_mode` メソッドは存在せず、`ContainerRequest` に `cgroupns_mode` フィールドも無い。`src/` と `tests/` のどこからも構築・参照されていない (型の定義と re-export 以外の出現箇所ゼロ)
- `docs/TESTCONTAINERS.md` でも `cgroupns_mode(&self)` は「なし (with_cgroupns_mode とセット)」と明記されており、型だけが浮いている。`with_cgroupns_mode` を実装しない限り永久に死にコード

### 一度も呼ばれない Display impl

- `MountType` (`src/core/mounts.rs`)、`AccessMode` (`src/core/mounts.rs`)、`ExtraHost` (`src/core/containers/request.rs`) にそれぞれ `Display` impl がある
- 実際の変換箇所はすべて `match` で直接行っており `Display` を経由しない:
  - `mount_cfg` (`src/core/client/container_cfg.rs`): `MountType` は `match` で `fs_type` 文字列に、`AccessMode` は `matches!` で bool に変換
  - `CreateContainerBody::from_config` (`src/core/client/docker_client.rs`): `MountType::Bind` だけ `match` で取り出して `"{source}:{target}"` を組み立てる
  - `apply_extra_hosts` (`src/runners/async_runner.rs`): `ExtraHost` を `match` で分解し、`Addr` の内側の `IpAddr` に対して `to_string()` する (ExtraHost 自身の Display ではない)。`HostGateway` の場合はエラーを返す
- ただし `src/core/mounts.rs` のユニットテスト (`access_mode_and_display_are_reflected`) が `MountType::Bind.to_string()` / `AccessMode::ReadOnly.to_string()` 等を明示的に呼び出している。Display impl を削除する場合、このテストの削除または書き換えが必須である

### HealthWaitStrategy::with_poll_interval

- `src/core/wait/health_strategy.rs` の `poll_interval` フィールドは `with_poll_interval` で書き込まれるが、`wait_until_ready` が無条件に `WaitContainerError::HealthCheckNotConfigured` を返すため一度も読まれない (`#[derive(Debug)]` 経由の出力以外に用途無し)
- 対比として `ExitWaitStrategy` (`src/core/wait/exit_strategy.rs`) と `HttpWaitStrategy` (`src/core/wait/http_strategy.rs`) は `poll_interval` を実際に `tokio::time::sleep` に使っている
- `HealthWaitStrategy` は公開型 (`src/core/wait/mod.rs` で re-export) なので、`with_poll_interval` を呼ぶ利用者は「ポーリング間隔が効いている」と誤解する。ヘルスチェックの待機自体が Apple Container の XPC 制約 (ヘルスチェック相当の情報を公開しない) で実装不可と方針決定済み (`docs/TESTCONTAINERS.md`)

## 設計方針

各項目について「削除」か「意図的保持 (判断をコメント等で明記)」かを決めて整理する。以下を目安とする:

- `CgroupnsMode`: 削除する。`with_cgroupns_mode` と `ContainerRequest` フィールドを実装するときに戻せばよく、型だけ先に置いておく意味は無い。re-export (`src/core/containers/mod.rs`、`src/core.rs`) もあわせて外す。`docs/TESTCONTAINERS.md` の `CgroupnsMode` 関連行も削除する
- 未使用の `Display` impl: 本家互換の公開 API として意図的に保持する判断なら、その旨を impl 付近のコメントに明記する。保持する必然が無いと判断した場合は impl を削除する (この場合 `src/core/mounts.rs` のユニットテストもあわせて削除する)。`ExtraHost::HostGateway` の `Display` 出力 "host-gateway" は Docker 固有の表現であり macOS では未対応 (`apply_extra_hosts` でエラーにしている) な点も判断材料とする
- `HealthWaitStrategy`: `poll_interval` フィールドと `with_poll_interval` を削除し、`HealthWaitStrategy` を設定を持たない型にする。これにより `WaitFor::healthcheck()` が常に `HealthCheckNotConfigured` エラーになることが API 形状から読み取れる正直な形になる (設定項目を持たない = 調整の余地が無い)。`Default` derive への置き換え等、最小の変更に留める。`docs/TESTCONTAINERS.md` の `with_poll_interval` 行も削除するか「設定値は待機処理で読まれない」旨の備考に修正する
- `docs/TESTCONTAINERS.md` の該当行 (`cgroupns_mode` 行と `CgroupnsMode` の節、`with_poll_interval` 行) を整理後の実態に合わせて更新する
- `ExitWaitStrategy` / `HttpWaitStrategy` の `poll_interval` は実際に使われているため対象外とする

## 完了条件

- [ ] `CgroupnsMode` が削除されるか、保持する場合はその判断根拠がコードコメントに明記されること
- [ ] `MountType` / `AccessMode` / `ExtraHost` の各 `Display` impl について、削除または意図的保持 (コメント明記) のいずれかの判断がなされること
- [ ] `HealthWaitStrategy` から `poll_interval` フィールドと `with_poll_interval` が削除されるか、保持する場合は「待機処理では読まれない」旨が明記されること
- [ ] `docs/TESTCONTAINERS.md` の該当箇所が整理後の実態に合わせて更新されること
- [ ] `cargo test` が pass すること (公開 API 変更で統合テストのコンパイルに影響する場合は `cargo test --all-features`)
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
