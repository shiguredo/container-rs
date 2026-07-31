# リファクタリング: 未使用の型・メソッド (CgroupnsMode / 未使用 Display impl) を整理する

- Priority: Low
- Created: 2026-07-12
- Completed: 2026-08-01
- Model: Kimi
- Branch: feature/refactor-remove-unused-types
- Polished: 2026-07-29

## 目的

本家 testcontainers-rs とのシグネチャ互換のために定義したものの、構築・参照ともに一度も行われていない型・メソッドが公開 API として残っている。利用者に「使える」という誤解を与えるため、各項目について削除するか意図的保持 (判断をコメント等で明記) するかを決めて整理する。対象は次の 2 点:

- `CgroupnsMode` (`src/core/containers/request.rs`)
- 本番コードから呼ばれない `Display` impl (`MountType` / `ExtraHost`)

## 優先度根拠

機能上の欠陥は無く、死にコードと API 形状の正直さの問題であるため Low。

## 現状

### CgroupnsMode

- `src/core/containers/request.rs` に `CgroupnsMode` (Host / Private) が定義され、`src/core/containers/mod.rs` と `src/core.rs` で公開 re-export されている
- しかし `ImageExt` に `with_cgroupns_mode` メソッドは存在せず、`ContainerRequest` に `cgroupns_mode` フィールドも無い。`src/` と `tests/` のどこからも構築・参照されていない (型の定義と re-export 以外の出現箇所ゼロ)
- `docs/TESTCONTAINERS.md` でも `cgroupns_mode(&self)` は「なし (with_cgroupns_mode とセット)」と明記されており、型だけが浮いている。`with_cgroupns_mode` を実装しない限り永久に死にコード
- `issues/pending/0004-add-macos-cgroupns-mode-xpc.md` が将来 `with_cgroupns_mode` を実装する可能性があり、その際には enum 定義 + re-export 2 箇所の復元が必要

### 本番コードから呼ばれない Display impl

- `MountType` (`src/core/mounts.rs`)、`ExtraHost` (`src/core/containers/request.rs`) にそれぞれ `Display` impl がある
- 実際の変換箇所はすべて `match` で直接行っており `Display` を経由しない:
  - `mount_cfg` (`src/core/client/container_cfg.rs`): `MountType` は `match` で `fs_type` 文字列に変換
  - `CreateContainerBody::from_config` (`src/core/client/docker_client.rs`): `MountType::Bind` だけ `match` で取り出して `"{source}:{target}"` を組み立てる
  - `apply_extra_hosts` (`src/runners/async_runner.rs`): `ExtraHost` を `match` で分解し、`Addr` の内側の `IpAddr` に対して `to_string()` する (ExtraHost 自身の Display ではない)。`HostGateway` の場合はエラーを返す
- ただし `src/core/mounts.rs` のユニットテスト (`access_mode_and_display_are_reflected`) が `MountType::Bind.to_string()` 等を明示的に呼び出している。このテストは対象外の `AccessMode` Display のアサーションも含むため、`MountType` Display を削除する場合はテスト全体ではなく `MountType` のアサーション 3 行のみ除去し、`AccessMode` のアサーションは残す
- **`AccessMode` の Display impl は対象外**: `docker_client.rs:630` の `format!("{source}:{target}:{}", m.access_mode())` が `Display` 経由で `"ro"` / `"rw"` を生成しており、Linux 経路の本番コードで実際に使われている。`MountType` / `ExtraHost` と同列に扱えない

## 設計方針

各項目について「削除」か「意図的保持 (判断をコメント等で明記)」かを決めて整理する。以下を目安とする:

- `CgroupnsMode`: 削除する。`with_cgroupns_mode` と `ContainerRequest` フィールドを実装するときに戻せばよく、型だけ先に置いておく意味は無い。re-export (`src/core/containers/mod.rs`、`src/core.rs`) もあわせて外す。`skills/shiguredo-container/SKILL.md` の公開 API 一覧からも `CgroupnsMode` を外す。`docs/TESTCONTAINERS.md` は、型自体を記録する行 (292 行目の `CgroupnsMode::Host / Private` 行、618-620 行目の `### 16.3 CgroupnsMode` 節) を削除し、本家 API との比較行 (135 行目 `with_cgroupns_mode`、264 行目 `cgroupns_mode(&self)`) は「本家にはあるが自前にはない」という事実が変わらないため維持する。82 行目のサマリ文言も実態に合わせて修正する。CODEBASE.md は本家互換のための re-export を許可しているが、`CgroupnsMode` は re-export だけで本体のメソッドもフィールドも存在しないため、互換性の維持対象にならない
- `MountType` / `ExtraHost` の未使用 `Display` impl: 本家互換の公開 API として意図的に保持する判断なら、その旨を impl 付近のコメントに明記する。保持する必然が無いと判断した場合は impl を削除する (この場合 `src/core/mounts.rs` のユニットテスト `access_mode_and_display_are_reflected` から `MountType` Display のアサーション 3 行のみ除去し、対象外の `AccessMode` Display アサーションは残す)。判断基準は「本家 testcontainers-rs に同じ Display impl が存在し、利用者が `to_string()` で使う可能性があるか」。`ExtraHost::HostGateway` の `Display` 出力 "host-gateway" は Docker 固有の表現であり macOS では未対応 (`apply_extra_hosts` でエラーにしている) な点も判断材料とする。`MountType` Display を削除する場合、`docs/TESTCONTAINERS.md:549` の `MountType Display (snake_case)` 行も実態に合わせて更新する
- `docs/TESTCONTAINERS.md` の該当行を整理後の実態に合わせて更新する (上記の各項目参照)
- `CgroupnsMode` の削除は公開 API の破壊的変更にあたるため、`CHANGES.md` に `[CHANGE]` エントリを記載する。Display impl を削除する場合も同様に `[CHANGE]`
- `ExitWaitStrategy` / `HttpWaitStrategy` / `HealthWaitStrategy` の `poll_interval` は実際に使われているため対象外とする (HealthWaitStrategy は Linux 経路の `health_strategy.rs:91` で `tokio::time::sleep(self.poll_interval)` として読まれている)

## 完了条件

- [ ] `CgroupnsMode` が削除されるか、保持する場合はその判断根拠がコードコメントに明記されること
- [ ] `MountType` / `ExtraHost` の各 `Display` impl について、削除または意図的保持 (コメント明記) のいずれかの判断がなされること
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の該当箇所が整理後の実態に合わせて更新されること
- [ ] 公開 API の削除を伴う場合は `CHANGES.md` に `[CHANGE]` エントリが記載されること
- [ ] `cargo test` が pass すること (公開 API 変更で統合テストのコンパイルに影響する場合は `cargo test --all-features`)
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

- `CgroupnsMode` を `src/core/containers/request.rs` から削除し、`src/core/containers/mod.rs` と `src/core.rs` の re-export も外した
- `MountType` (`src/core/mounts.rs`) と `ExtraHost` (`src/core/containers/request.rs`) の Display impl は意図的保持と判断し、各 impl 付近に保持理由のコメントを追加した。ExtraHost は Linux (Docker) 経路の extra_hosts 変換で利用するため保持
- `docs/TESTCONTAINERS.md` から CgroupnsMode の型定義行と 16.3 節を削除し、節番号の振り直し (16.4 → 16.3) と相互参照の修正を行った
- `skills/shiguredo-container/SKILL.md` の公開 API 一覧から CgroupnsMode を外した
- `CHANGES.md` に `[CHANGE]` エントリを追加した
