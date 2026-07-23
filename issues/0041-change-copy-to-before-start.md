# 仕様変更: with_copy_to をコンテナプロセス起動前に完了させる

- Priority: High
- Created: 2026-07-23
- Completed: {YYYY-MM-DD}
- Model: Claude Fable 5
- Branch: feature/change-copy-to-before-start
- Polished: {YYYY-MM-DD}
- Reporter: @voluntas

## 目的

`with_copy_to` で投入するファイルが「コンテナの初期プロセスが起動する前」に存在することを契約にする。

testcontainers-rs では次のように書けた。Docker が create → archive copy → start の順を許すため、起動待ちシェルは不要だった。

```rust
.with_cmd(["mosquitto", "-c", conf_path])
.with_copy_to(conf_path, ...)
.with_wait_for(WaitFor::message_on_stderr("... running"))
```

現在の shiguredo_container は両 OS とも start 後にコピーするため、利用側 (mqtt-rs) に次の回避策が必要になっている。

- `while [ ! -f .../server.key ]; do sleep ...; done; exec mosquitto ...` の起動待ちシェルが必須
- イメージ同梱の `mosquitto.conf` がある場合、「conf が現れるのを待つ」方式ではコピー前に同梱 conf で起動してしまう
- EMQX の QUIC 用証明書は「EMQX の起動が遅いからたまたまコピーが間に合う」というレース依存

## 優先度根拠

利用者フィードバック (mqtt-rs の移行)。copy のタイミング契約が testcontainers-rs と異なることが移行時の最大の痛点で、上記の起動待ちシェルとレース依存はライブラリ側でしか解消できない。High。

## 現状

- Linux: `AsyncRunner::start` が `start_container` 成功後に `copy_to_sources_linux` を呼ぶ (`src/runners/async_runner.rs:288-299`)。コメントに「macOS が start_process 後に呼ぶのと揃える」とあり、issue `0013` (closed) で意図的にこの位置を選んでいる
- macOS: `start_process` 成功後に `copy_to_sources` を呼ぶ (`src/runners/async_runner.rs:160-174`)。コメントに「Apple container の containerCopyIn はコンテナが running の場合にのみ利用できるため」とある
- Docker Engine API の `PUT /containers/{id}/archive` は created (未 start) のコンテナにも使える (`docker cp` が停止中コンテナに使えるのと同じ)

## 設計方針

契約: `AsyncRunner::start` / `SyncRunner::start` が返した時点ではなく、**コンテナの初期プロセスが最初の命令を実行する前** に `with_copy_to` のファイルが配置済みであること。実装は OS で分けてよい。

- Linux: `copy_to_sources_linux` の呼び出しを `create_container` 成功後・`start_container` 前に移す。ロールバック (Keep-gated 明示 rm) の構造は維持する
- macOS: `containerCopyIn` が created (bootstrap 済み・start_process 前) の状態で使えるかを実測する
  - 使える場合: `bootstrap_container` 後・`start_process` 前にコピーを移す
  - 使えない場合: 本 issue は Linux のみ先行して契約を変更し、macOS は現状の制約 (start 後コピー、レースあり) を README / `docs/TESTCONTAINERS.md` / SKILL に明記したうえで、内部 wait-and-exec 等の遅延起動方式を別 issue に切り出す
- コピー失敗時のロールバック挙動 (Keep-gated 明示 rm) は変更しない

## 完了条件

- Linux で `with_copy_to` + `with_cmd(["cat", "/path/to/copied-file"])` のような「起動直後にコピー済みファイルを読む」コンテナが、起動待ちシェルなしで成功する統合テストが pass する
- macOS の実測結果に応じて、コピー位置の変更または制約の文書化 + 別 issue 起票が済んでいる
- README / `docs/TESTCONTAINERS.md` / `skills/shiguredo-container/SKILL.md` の copy タイミング記述が実装と一致している
- `CHANGES.md` に `[CHANGE]` エントリがある
- `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass する

## 解決方法

`src/runners/async_runner.rs` の Linux 分岐で `copy_to_sources_linux` を `create_container` と `start_container` の間に移動する。macOS は `containerCopyIn` の created 状態での可否を実測してから、コピー位置の移動または制約の文書化を行う。「start 前にコピーが完了している」ことを検証する統合テストを追加する。
