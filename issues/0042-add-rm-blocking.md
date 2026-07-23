# 機能追加: tokio Runtime 内から削除完了を待てる同期 rm_blocking を追加する

- Priority: Medium
- Created: 2026-07-23
- Completed: {YYYY-MM-DD}
- Model: Claude Fable 5
- Branch: feature/add-rm-blocking
- Polished: {YYYY-MM-DD}
- Reporter: @voluntas

## 目的

tokio Runtime 内の同期コンテキスト (利用者側の `Drop` ガード等) から、コンテナ削除の完了を待てる公開 API を提供する。

testcontainers-rs 利用時の mqtt-rs は `ContainerAsync` を保持するだけで追加の掃除をしていなかったが、shiguredo_container への移行後は「Runtime 内 Drop は削除完了を保証しない」契約 (issue `0035` で文書化) のため、全テストのガードが `Drop` から CLI (`docker rm -f` / `container rm`) を直接呼ぶ回避策を取っている。ライブラリ内の削除経路を同期で呼べれば、この CLI 依存をなくせる。

## 優先度根拠

利用者フィードバック (mqtt-rs の移行)。削除完了を待てないとテストプロセス終了直前の Drop でコンテナが残り得るため、利用側は CLI 強制削除で自衛している。契約文書化 (`0035`) では挙動変更を明示的にスコープ外としており、完了を待てる同期経路の提供が残課題。copy タイミング契約 (`0041`) に次ぐ痛点で Medium。

## 現状

- Runtime 内 Drop (`src/core/containers/async_container.rs:1361-1371`): 専用 std スレッドで `remove_blocking` を実行するが join しない (fire-and-forget)。復帰時点で削除完了を保証しない
- 明示 `rm()`: `ContainerAsync::rm(mut self) -> Result<()>` は async (`src/core/containers/async_container.rs:762,786`)。sync `Container::rm()` (`src/core/containers/sync_container.rs:142`) は内部で `block_on` するため、**tokio Runtime 内の同期コンテキストからは呼べない** (panic する)
- `remove_blocking` は macOS (`XpcClient::remove_blocking`) / Linux (`DockerClient::remove_blocking`) とも tokio Runtime に依存しない同期実装で、Drop の削除スレッドが既に使っている
- macOS には `watchdog` feature (`src/watchdog.rs`) があり、プロセスクラッシュ時の孤立コンテナを reaper プロセスが掃除する。Linux には無い (本 issue の対象外)

## 設計方針

- `ContainerAsync` に `pub fn rm_blocking(mut self) -> Result<()>` を追加する。内部は Drop と同じ `remove_blocking` 経路を使い、tokio Runtime 内外のどちらから呼んでも安全 (block_on を使わない) にする
- セマンティクスは既存 `rm()` に揃える: `force=true`、404 は冪等成功、`TESTCONTAINERS_COMMAND=keep` でも削除する、成功後は `dropped = true` にして Drop の二重削除を防ぐ
- sync `Container` にも同名の `rm_blocking()` を委譲で追加する
- Runtime 内 Drop の fire-and-forget 挙動自体は変更しない (`0035` の契約を維持する)。Drop の timeout 付き join や Linux 側 reaper は本 issue の対象外とし、必要になれば別 issue とする
- rustdoc に「Runtime 内 Drop で完了を待ちたい場合はこれを使う」旨と `rm()` との使い分けを書く

## 完了条件

- `ContainerAsync::rm_blocking()` / `Container::rm_blocking()` が追加され、tokio Runtime 内の同期コンテキスト (spawn した std スレッドや Drop ガード相当) から呼んで削除完了まで待てる
- `rm_blocking()` が `Ok` を返した直後に 1 ショットの不在確認 (`docker inspect` 相当 1 回) が通る統合テストが pass する
- README / `docs/TESTCONTAINERS.md` / `skills/shiguredo-container/SKILL.md` の掃除契約に `rm_blocking` を追記している
- `CHANGES.md` に `[ADD]` エントリがある
- `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass する

## 解決方法

`src/core/containers/async_container.rs` に `rm_blocking` を追加し、Drop が使っている `remove_blocking` クロージャと同じ分岐 (macOS: `XpcClient::remove_blocking` / Linux: `DockerClient::remove_blocking`) を呼ぶ。`dropped` フラグと keep 非対称の扱いは `rm()` に揃える。sync 側は委譲のみ。統合テストで Runtime 内同期コンテキストからの削除完了を検証する。
