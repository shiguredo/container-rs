# 機能追加: tokio Runtime 内から削除完了を待てる同期 rm_blocking を追加する

- Priority: Medium
- Created: 2026-07-23
- Completed: 2026-07-31
- Model: Claude Fable 5
- Branch: feature/add-rm-blocking
- Polished: 2026-07-29
- Reporter: @voluntas

## 目的

tokio Runtime 内の同期コンテキスト (利用者側の `Drop` ガード等) から、コンテナ削除の完了を待てる公開 API を提供する。既存の `ContainerAsync::rm()` は async のため同期コンテキストから `.await` できず、sync `Container::rm()` は `block_on_runtime` を使うため同一 Runtime 内からの再入で `Err` を返す。

testcontainers-rs 利用時の mqtt-rs は `ContainerAsync` を保持するだけで追加の掃除をしていなかったが、shiguredo_container への移行後は「Runtime 内 Drop は削除完了を保証しない」契約 (issue `0035` で文書化) のため、全テストのガードが `Drop` から CLI (`docker rm -f` / `container rm`) を直接呼ぶ回避策を取っている。ライブラリ内の削除経路を同期で呼べれば、この CLI 依存をなくせる。

## 現状

- Runtime 内 Drop (`impl Drop for ContainerAsync` の `Handle::try_current()` が `Ok` の分岐): 専用 std スレッドで `remove_blocking` を実行するが join しない (fire-and-forget)。復帰時点で削除完了を保証しない
- 明示 `rm()`: `ContainerAsync::rm(mut self) -> Result<()>` は async (macOS / Linux 両 `#[cfg]` 定義)。sync `Container::rm()` は内部で `block_on_runtime` を使うため、同一共有 Runtime への再入では `Err` を返す（panic はしない。別 Runtime 内では別スレッドに退避して正常動作する）
- `remove_blocking` は macOS (`XpcClient::remove_blocking`) / Linux (`DockerClient::remove_blocking`) とも tokio Runtime に依存しない同期実装で、Drop の削除スレッドが既に使っている
- macOS には `watchdog` feature (`src/watchdog.rs`) があり、プロセスクラッシュ時の孤立コンテナを reaper プロセスが掃除する。Linux には無い (本 issue の対象外)

## 設計方針

- `ContainerAsync` に `pub fn rm_blocking(mut self) -> Result<()>` を追加する。内部は Drop と同じ `remove_blocking` 経路を使い、tokio Runtime 内外のどちらから呼んでも安全 (block_on を使わない) にする
- セマンティクスは既存 `rm()` に揃える: `force=true`、404 は冪等成功、`TESTCONTAINERS_COMMAND=keep` でも削除する、成功後は `dropped = true` にして Drop の二重削除を防ぐ。Linux 版 `rm()` と同様に削除前に `stop_log_delivery()` を呼ぶ（macOS 版 `rm()` には `stop_log_delivery()` がないが、Drop は共通で呼んでいるため、`rm_blocking` も共通で呼ぶ）
- sync `Container` にも同名の `rm_blocking()` を委譲で追加する
- Runtime 内 Drop の fire-and-forget 挙動自体は変更しない (`0035` の契約を維持する)。Drop の timeout 付き join や Linux 側 reaper は本 issue の対象外とし、0049 で対応する。0049 が先に実装されると Drop が timeout 付きで待つようになるため、`rm_blocking` の存在意義は「成否 `Result` が必要な経路」にシフトする。rustdoc の文言は 0049 実装後に書き直しになり得る
- rustdoc に「Runtime 内 Drop で完了を待ちたい場合はこれを使う」旨と `rm()` との使い分けを書く

## 完了条件

- [ ] `ContainerAsync::rm_blocking()` / `Container::rm_blocking()` が追加され、tokio Runtime 内の同期コンテキスト (Runtime ワーカースレッド上で実行される Drop ガードや `spawn_blocking` 内) から呼んで削除完了まで待てる
- [ ] `rm_blocking()` が `Ok` を返した直後に 1 ショットの不在確認 (`docker inspect` 相当 1 回) が通る統合テストが pass する
- [ ] `rm_blocking` の rustdoc に `rm()` との使い分けと Runtime 内 Drop との関係を記載している
- [ ] README / `docs/TESTCONTAINERS.md` / `skills/shiguredo-container/SKILL.md` の掃除契約に `rm_blocking` を追記している
- [ ] `CHANGES.md` に `[ADD]` エントリがある
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass する

## 解決方法

`ContainerAsync` に `pub fn rm_blocking(mut self) -> Result<()>` を追加した。

1. `rm_blocking` は Drop が使っている `remove_blocking` と同じ分岐 (macOS: `XpcClient::remove_blocking` / Linux: `DockerClient::remove_blocking`) を直接呼び出す。`block_on` を使わないため tokio Runtime 内の同期コンテキストから呼んでも deadlock しない
2. 削除前に `stop_log_delivery()` を呼び、成功後は `dropped = true` にして Drop の二重削除を防ぐ。`force=true`、404 冪等、`keep` 非対称は `rm()` に揃えた
3. macOS 版 `rm()` にも `stop_log_delivery()` を追加して `rm_blocking` / Linux 版 `rm()` / Drop とセマンティクスを揃えた
4. sync `Container` にも `rm_blocking()` を委譲で追加した
5. rustdoc に `rm()` との使い分けと Runtime 内 Drop との関係を記載した
6. README / `docs/TESTCONTAINERS.md` (6.1 API 表 + 7 章 API 表 + Drop 行) / `skills/shiguredo-container/SKILL.md` の掃除契約に `rm_blocking` を追記した
7. Linux / macOS の両方に `spawn_blocking` 内から `rm_blocking` を呼ぶ統合テストを追加した
8. `CHANGES.md` に `[ADD]` エントリを追加した

変更ファイル: `src/core/containers/async_container.rs`、`src/core/containers/sync_container.rs`、`tests/container_linux.rs`、`tests/container_macos.rs`、`README.md`、`docs/TESTCONTAINERS.md`、`skills/shiguredo-container/SKILL.md`、`CHANGES.md`
