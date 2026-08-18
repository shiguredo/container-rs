# 仕様変更: with_copy_to をコンテナプロセス起動前に完了させる

- Priority: High
- Created: 2026-07-23
- Completed: 2026-07-23
- Model: Claude Fable 5
- Branch: feature/change-copy-to-before-start
- Polished: 2026-07-23
- Reporter: @voluntas

## 目的

`with_copy_to` で投入するファイルが、`start_container` / `start_process` より前に投入完了していることを **Linux では公開契約**にする。macOS は実測結果に応じて同じ契約に揃えるか、現状制約を文書化したうえで残作業を別 issue に切り出す。

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

- Linux: `AsyncRunner::start` が `start_container` 成功後に `copy_to_sources_linux` を呼ぶ (`src/runners/async_runner.rs:288-299`)。コメントは「macOS が start_process 後に呼ぶのと揃える」
- macOS: `start_process` 成功後に `copy_to_sources` を呼ぶ (`src/runners/async_runner.rs:160-174`)。「running の場合にのみ」は **コードコメント由来**で、`XpcClient::copy_in` に状態ゲートは無く、Apple 一次資料での確認も本リポジトリには無い。本 issue で実測する
- Docker Engine API の `PUT /containers/{id}/archive` (`PutContainerArchive`) はリクエスト仕様上 running を必須としない。closed `0013` はこれを既知のうえで macOS との挙動差を減らすため **あえて start 後**に置いた。本 issue はその判断を、testcontainers 互換と mqtt-rs 痛点解消のために **意図的に覆す**
- `ContainerAsync::start`（停止後の再起動）は `with_copy_to` を再実行しない（`src/core/containers/async_container.rs` の `start`。Linux 再起動は `refresh_log_streams` 内で `start_container`）
- 既存の copy 統合テストは常駐プロセス + start 後の `copy_file_from` / `exec` のみで、**起動時読込レースは検出できない**
- 文書上のタイミング記述: `docs/TESTCONTAINERS.md:56` と `skills/shiguredo-container/SKILL.md:69` は「start 後」。同 L157 の macOS 列は `StartProcess → CopyIn`。L104 / L127 は経路・制約の説明でタイミング未記載。L157 の Docker 列は copy 自体が未記載
- 関連: issue `0043` は親ディレクトリ自動作成・ディレクトリ投入（タイミングとは別）。同一関数と docs を触るため **本 issue を先に**進める

## 設計方針

### 公開契約（OS スコープ付き）

- **Linux (MUST)**: 初回の `AsyncRunner::start`（それを呼ぶ `SyncRunner::start` を含む）で、`start_container` より前に `with_copy_to` の投入が完了していること
- **macOS**: 実測で `bootstrap` 後・`start_process` 前に `containerCopyIn` が使える場合のみ、同契約（`start_process` 前に投入完了）に揃える。使えない場合は契約を Linux のみに限定し、macOS は「start 後コピー・レースあり」を文書化した現状維持とする。OS 非対称を隠した単一契約は謳わない
- **スコープ外**: `ContainerAsync::start`（再起動）での再投入は行わない。再投入が必要なら別 issue
- 順序保証の実装点は `AsyncRunner::start` の OS 分岐のみ。`SyncRunner` / `DockerClient::copy_to` / `XpcClient::copy_in` のシグネチャ変更は不要

### Linux

- `copy_to_sources_linux` を `create_container` 成功後・`start_container` 前に移す
- 新順序とロールバック（Keep-gated 明示 rm。コピー個別の巻き戻しはしない）:

  ```text
  create_container
  copy_to_sources_linux   // 失敗 → Keep-gated remove(id); return Err（未 start）
  start_container         // 失敗 → Keep-gated remove(id); return Err（copy 済みでも rm のみ）
  log stream / ContainerAsync::new / ready
  ```

- `async_runner.rs:288-289` の「macOS と揃える」コメントは削除し、**理由そのもの**（起動前投入契約のため create 後・start 前へ移した）を書く
- Linux に `watchdog` は無い。create〜start 区間が延びるが、失敗時は明示 rm で掃除するため追加対策しない

### macOS 実測プロトコル

1. `create_container` → `bootstrap_container` まで進める
2. `start_process` **前**に既存の `copy_to_sources` を 1 回呼ぶ
3. **移動可の判定**: step 2 の `copy_in` が成功し、かつ初期プロセスが投入内容を読めること（Linux と同型の `WaitFor::message_on_stdout` テストで確認）。`copy_in` 成功でも内容確認に失敗したら「使えない」側に倒す
4. 移動可ならコピー位置を `bootstrap` 後・`start_process` 前へ移す。ロールバック順:

  ```text
  create_container
  bootstrap_container
  copy_to_sources    // 失敗 → Keep-gated remove
  start_process      // 失敗 → Keep-gated remove
  … / ready
  ```

   「running の場合にのみ」コメントは実測結果に合わせて更新する。同等の統合テストを `tests/container_macos.rs` に追加する（CI skip 慣例に従う）。target は Linux と同じく `/tmp/...` または `/etc/motd`（alpine で通常ファイルかつ start 後も残る）等とし、`/etc/hostname`・`/etc/hosts`・`/etc/resolv.conf` は使わない。watchdog 登録コメント（`async_runner.rs` の create 直後、および `docs/TESTCONTAINERS.md` の reaper 節）の `bootstrap / start / copy` 順も実順序に合わせて直す
5. 使えない場合: XPC エラー全文・Apple Container / OS バージョンを本 issue の `## macOS 実測結果` に追記し、コピー位置は現状維持。以下の草稿を docs / SKILL / README に反映する。残作業（起動前にファイルを見せる手段）は実装せず別 issue を起票するだけとし、手段名は本 issue で確定しない

失敗時ドキュメント草稿:

- `docs/TESTCONTAINERS.md`: `Linux: create 後・start 前に PUT /archive。macOS: start_process 後の containerCopyIn（レースあり）。起動前投入は Linux のみの公開契約。`
- `skills/shiguredo-container/SKILL.md`: `Linux: create → copy → start。macOS: start 後 copy（起動前契約なし）。`
- `README.md`: `` `with_copy_to` の起動前投入は Linux のみ。macOS は start 後コピーのため、初期プロセスが起動時に読むファイルには利用側の起動待ち等が別途必要になり得る。``

### 後方互換・CHANGES

- 種別は `[CHANGE]`。`## develop` では CHANGE → ADD → UPDATE → FIX の順で `[ADD]` / `[FIX]` より上に置く。担当者行（`  - @voluntas`）を付ける
- 影響: Linux では start 後にファイルが後から現れる前提のコードは壊れる。mqtt-rs の起動待ちシェルは不要になる（残しても動く）
- start 後に CMD が親ディレクトリを作ってから copy が走る前提のパスは、本変更後は確実に失敗する（親自動作成は別 issue。CHANGES 本文に issue 番号は書かない）
- macOS が移動できない場合は「Linux のみ契約変更、macOS は従来どおり」と書き分ける
- 公開 API シグネチャは変更しない

### エッジケース

- `copy_to_sources` 空: no-op
- 親ディレクトリ不在: create 直後も同様に失敗（自動作成なし）。必須テストの target はイメージに既にある親（`/tmp/...` や上書き対象の既存通常ファイル）に限る
- Docker が start 時に bind-mount で差し替えるパス（`/etc/hostname`・`/etc/hosts`・`/etc/resolv.conf`）への上書き検証は禁止。create 後の投入は start で隠される
- 複数 `with_copy_to`（および Image 側 sources）: **すべて**終わってから `start_*`。その後に ready。2 件目失敗時もコンテナ単位の Keep-gated rm のみ。複数 copy を必須テストにはしない
- Keep (`TESTCONTAINERS_COMMAND=keep`): 失敗時も削除しない

### 変更対象ファイル（要のみ）

| 対象 | 内容 |
|------|------|
| `src/runners/async_runner.rs` | Linux 呼び出し位置 + コメント。macOS は実測次第 |
| `src/core/image/image_ext.rs` の `with_copy_to` rustdoc | タイミング契約（OS スコープ付き）を必須で追記 |
| `src/core/image.rs` の `Image::copy_to_sources` rustdoc | 「起動時に」を契約に合わせる（または `with_copy_to` に従うと明示）。`copy.rs` は補足可 |
| `tests/container_linux.rs` | 起動前投入の統合テスト（新規作成 + 既存パス上書き） |
| `tests/container_macos.rs` | 移動可なら同型テスト |
| `docs/TESTCONTAINERS.md` | L56 / L104 / L127。L157 の Docker 列は `create → copy → start` の順で copy を追記。macOS 移動時は XPC 列と reaper 節（`bootstrap / start / copy`）も更新 |
| `skills/shiguredo-container/SKILL.md` | L69。L214 付近は「リクエスト組み立て」と「実コピー実行」を混同しない |
| `README.md` | OS 差を書く場合 |
| `CHANGES.md` | `[CHANGE]` |

## 完了条件

- Linux で次の 2 統合テストが `tests/container_linux.rs` に追加され pass する（即終了 `cat` 単体や `WaitFor::exit` / `exit_code` 依存は禁止）。`with_wait_for` は `GenericImage` 固有のため **ImageExt 呼び出しより前**に置く（既存 Log 待機テストと同順）。常駐は `tail -f /dev/null`（既存慣例）。`Duration` / `WaitFor` / `ImageExt` は同ファイル先頭の既存 import で足りる

  1. 新規パス（イメージに無いファイル）:

  ```rust
  /// 初回 start 経路で、初期プロセスが start 前に投入された新規ファイルを読めること。
  #[tokio::test]
  async fn copy_to_visible_before_initial_process() {
      let marker = "COPY_BEFORE_START_OK";
      let container = GenericImage::new("alpine", "latest")
          .with_wait_for(WaitFor::message_on_stdout(marker))
          .with_cmd([
              "sh", "-c",
              "test -f /tmp/payload.txt && cat /tmp/payload.txt && exec tail -f /dev/null",
          ])
          .with_copy_to("/tmp/payload.txt", format!("{marker}\n").into_bytes())
          .with_startup_timeout(Duration::from_secs(15))
          .start()
          .await
          .expect("起動前コピーの可視性検証に失敗した");
      container.rm().await.expect("rm に失敗した");
  }
  ```

  2. 既存パス上書き（同梱 conf 痛点に対応）。`/etc/motd` は通常ファイルで、Docker が start 時に差し替える `/etc/hostname`・`/etc/hosts`・`/etc/resolv.conf` および alpine の symlink（`/etc/os-release`）は使わない:

  ```rust
  /// 初回 start 経路で、初期プロセスが start 前に上書きされた既存ファイルの新内容を読むこと。
  #[tokio::test]
  async fn copy_to_overwrite_visible_before_initial_process() {
      let marker = "COPY_OVERWRITE_BEFORE_START_OK";
      let container = GenericImage::new("alpine", "latest")
          .with_wait_for(WaitFor::message_on_stdout(marker))
          .with_cmd([
              "sh", "-c",
              "test -f /etc/motd && cat /etc/motd && exec tail -f /dev/null",
          ])
          .with_copy_to("/etc/motd", format!("{marker}\n").into_bytes())
          .with_startup_timeout(Duration::from_secs(15))
          .start()
          .await
          .expect("既存パス上書きの起動前可視性検証に失敗した");
      container.rm().await.expect("rm に失敗した");
  }
  ```

  - 順序保証の正はコード上の `create → copy → start`。統合テストは回帰用
  - ファイル欠落で初期プロセスが即終了する場合、Linux の Log 待機は `StartupTimeout` ではなく **EndOfStream** になる。欠落時の期待を必須アサーションにはしない
  - 既存の往復テストは回帰用として残す

- macOS 実測結果が本 issue に追記され、コピー位置の変更（+ 同型テスト）**または** 制約の文書化（上記草稿）+ 別 issue 起票が済んでいる
- 文書・rustdoc の copy タイミングが実装と一致している（変更対象表のとおり）
- ソース・docs・README・`CHANGES.md`・rustdoc・テストに issue 番号を書かない (`shiguredo-issues` 規約)
- `CHANGES.md` に影響範囲（OS 差・親 dir 前提の破壊を含む）を書いた `[CHANGE]` がある
- `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass する

## macOS 実測結果

- 実施日: 2026-07-23
- OS: macOS 26.5.2 (Build 25F84)
- 手順: `create_container` → `bootstrap_container` の後、`start_process` 前に既存の `copy_to_sources`（XPC `containerCopyIn`）を実行
- 1.0.0 での結果: 失敗。エラー全文:
  `Client(Xpc("XPC error invalidState: container c-80831-1784778212691883000-0 is not running"))`
- 1.1.0 での再実測: 同じく失敗。エラー全文:
  `Client(Xpc("XPC error invalidState: container c-67112-1784779224025071000-0 is not running"))`
  - Apple container CLI: 1.1.0 (build: release)
  - container-apiserver: 1.1.0 (build: release)
- 判定: bootstrap 後・start_process 前の移動は不可（1.0.0 / 1.1.0 とも）。コピー位置は start_process 後のまま維持する
- 残作業: 起動前にファイルを見せる手段は別 issue（`issues/0045-add-macos-prestart-file-visibility.md`）へ切り出した

## 解決方法

Linux の `AsyncRunner::start` で `copy_to_sources_linux` を `create_container` 成功後・`start_container` 前に移し、起動前投入を公開契約にした。失敗時は Keep-gated 明示 rm のみ（コピー個別の巻き戻しはしない）。

macOS は Apple container 1.0.0 / 1.1.0 で bootstrap 後・`start_process` 前の `containerCopyIn` を実測し、いずれも `invalidState: ... is not running` で失敗したためコピー位置は現状維持とした。制約は README / `docs/TESTCONTAINERS.md` / SKILL / rustdoc に文書化し、残作業は `issues/0045-add-macos-prestart-file-visibility.md` へ切り出した。

変更ファイル:

- `src/runners/async_runner.rs`: Linux の呼び出し順とコメント。macOS コメントを実測結果に合わせて更新
- `src/core/image/image_ext.rs` / `src/core/image.rs` / `src/core/copy.rs`: タイミング契約の rustdoc
- `tests/container_linux.rs`: `copy_to_visible_before_initial_process` / `copy_to_overwrite_visible_before_initial_process` を追加
- `docs/TESTCONTAINERS.md` / `skills/shiguredo-container/SKILL.md` / `README.md` / `CHANGES.md`: OS 差を含む契約記述
