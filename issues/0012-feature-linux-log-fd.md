# 機能追加: Linux で HTTP ログストリームを demux して stdout/stderr/LogConsumer/WaitFor::Log を有効化する

- Priority: High
- Created: 2026-07-21
- Completed: 2026-07-23
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-log-stream
- Polished: 2026-07-23

## 目的

Linux (Docker Engine API) バックエンドで、コンテナのログを取得して次を有効化する。

- `ContainerAsync::stdout()` / `ContainerAsync::stderr()` (`AsyncBufRead`)
- `ContainerAsync::stdout_to_vec()` / `ContainerAsync::stderr_to_vec()`
- `Container::stdout()` / `Container::stderr()` (`blocking` feature)
- `WaitFor::Log` (`message_on_stdout` / `message_on_stderr` / `message_on_either_std`)
- `ImageExt::with_log_consumer` (`LogConsumer` へのフレーム配信)

現状は `AsyncRunner::start` の Linux 分岐が `ContainerAsync::new` に `stdout_fd: None, stderr_fd: None` を渡し、さらに `WaitFor::Log` 使用時と `with_log_consumer` 使用時は start 入口で fail-fast エラーになるため、いずれの API も Linux では利用不能になっている。

## 優先度根拠

`WaitFor::Log` は testcontainers の中核的な ready 条件であり、これが Linux バックエンドで使えないと、多くのイメージで実質的に利用可能な ready 条件が `WaitFor::Nothing` / `WaitFor::seconds` / `WaitFor::Http` (feature 依存) に限られる。特に `WaitFor::Log` を前提とする実 OSS イメージ (`nginx` の "start worker processes" など) では現状 Linux CI での動作テストが書けない。Linux バックエンドの実用性を大きく損なうため High。

## 現状

- `AsyncRunner::start` の Linux 分岐 (`src/runners/async_runner.rs`) は `ContainerAsync::new` に `stdout_fd: None, stderr_fd: None` を渡し、ログストリームを一切保持しない
- `AsyncRunner::start` の Linux 分岐は `ready_conditions_require_log_fds` が true のとき、start 入口で `"log wait is not supported on Linux (log file descriptors are unavailable)"` を返して fail-fast する
- `AsyncRunner::start` の Linux 分岐は `linux_unsupported_request_reason` の中で `!log_consumers.is_empty()` を検出したとき、`"with_log_consumer() is not supported on Linux (log file descriptors are unavailable)"` を返して fail-fast する
- `ContainerAsync::stdout()` / `stderr()` は FD が None のため `tokio::io::empty()` を返す (呼んでも空リーダーになる)
- `ContainerAsync::stdout_to_vec()` / `stderr_to_vec()` は上記のため常に空の `Vec<u8>` を返す
- `DockerClient` にはログ取得メソッドが存在しない。`DockerClient::request` は `ResponseAccumulator` が Content-Length / Chunked の完了までブロックする一括読取で、long-lived な streaming レスポンスには適さない
- 既存テスト `log_wait_fails_fast_on_start` (`tests/container_linux.rs`) が「Log 待機は Linux で未対応」を assert している
- `docs/TESTCONTAINERS.md` は Docker Engine API 列で `stdout` / `stderr` / `stdout_to_vec` / `stderr_to_vec` / `LogFrame` / `LogSource` / `LogWaitStrategy` / `with_log_consumer` / `WaitFor::Log` / `message_on_*` / `log()` を「未実装」または「部分対応 (空リーダー)」と記載している
- `README.md` の Linux 警告は「ログ待機」を未対応項目として明記している
- Linux 側では `WaitState` に書き込むバックグラウンドタスクが無く、`exit_code_hint()` は常に `None` を返す (macOS の `spawn_exit_code_waiter` は `#[cfg(target_os = "macos")]`)

## 設計方針

本 issue 中の節番号や設計判断の内部識別子はコミットメッセージ・PR 本文・ソースコードコメントに持ち込まない (shiguredo-issues の「issue 番号・issue への言及をソースコードに持ち込まないこと」に準じる)。コードコメントには **選んだ実装方針の理由そのもの** (deadlock 回避のため、OOM 回避のため、TCP FIN で自然 EOF する仕様のため、等) を書く。

### 1. Docker Engine API 側の I/O 層 (下位)

- Docker Engine API の `GET /containers/{id}/logs` を利用する
  - クエリ: `stdout=1&stderr=1&follow={follow}&tail=all`
  - `follow=false` は「呼び出し時点の全ログを 1 回で読み切る」ためのモード
  - `follow=true` はログ待機・LogConsumer 配信で使う。コンテナ終了時に Docker Engine 側が接続を close することで自然に EOF する
- URL のバージョン prefix (`/v1.NN/`) は既存の `DockerClient` の他メソッドの慣習に合わせる (現状 prefix 無しなら prefix 無しで統一する)
- 既存の `DockerClient::request` は流用しない。`ResponseAccumulator` (`src/core/client/http_decode.rs`) は Content-Length / Chunked 完了までブロックする一括読取で、long-lived な streaming には使えないため、`logs` 用に別経路を作る
- 実装方針: **`spawn_blocking` 内で `std::os::unix::net::UnixStream` を長時間保持し、demux 後のフレームを `tokio::sync::mpsc` チャネル (bounded) で非同期側へ渡す**
  - 既存 `DockerClient::request` の pattern (`spawn_blocking` + `std::os::unix::net::UnixStream` + `Connection: close`) と一貫性を保つ
  - キャンセルは (a) 呼び出し側が mpsc の受信を落とす (b) 後述 8 節の停止経路で `UnixStream::shutdown(Shutdown::Both)` を明示的に発行する、の 2 経路で行う
- HTTP レスポンスヘッダは `\r\n\r\n` を境界に読み分ける。Content-Type は次の場合分けで扱う:
  - `application/vnd.docker.multiplexed-stream` / `application/octet-stream` / **空** のいずれか → multiplex demux で処理する (古い Docker Engine (API < 1.42 相当) は Content-Type を空 or `application/octet-stream` で返すため、Docker CLI と同じ挙動を保つ)
  - `application/vnd.docker.raw-stream` (TTY 有効時) → 範囲外、`std::io::Error::other` で上位に返す
  - その他 (400 系応答含む) → `std::io::Error::other` で上位に返す
- 常駐 `?follow=true` セッションは 1 コンテナ = 1 blocking スレッド固定占有。既存の `DockerClient::request` (短時間 spawn_blocking) と違い長期占有になることに注意。既定 blocking プール (512 スレッド) に対して同時起動コンテナ数は十分小さいため枯渇はしないが、CI runner のスレッド / メモリ消費への影響を実装レビューで確認すること
- 内部エラー (`UnixStream::connect` 失敗、demux で `payload_len` 破損検出、HTTP 400 系応答) はすべて `std::io::Error::other` に包んで `AsyncRead` / `AsyncBufRead` の Read エラーとして上位へ伝播する。上位は既存の `WaitLogError::Io(std::io::Error)` にそのまま乗る

### 2. multiplex フレームの demux

- Docker Engine API のログフレーム形式:
  - ヘッダ 8 バイト = `stream_type (1B: 0=stdin/1=stdout/2=stderr) + reserved (3B, 0 埋め) + payload_len (4B big-endian u32)`
  - ヘッダの後に `payload_len` バイトの payload が続く
- demux 実装は新規モジュール `src/core/client/docker_log_stream.rs` に置く (`docker_client.rs` / `xpc_client.rs` と並ぶ位置で意味を明確化)
  - 親モジュールに `mod docker_log_stream;` を追加し、可視性は `pub(crate)` に留める
  - 単体テスト用に「共有バッファと独立オフセットリーダーを作る」薄い `pub(crate)` API (例: `pub(crate) fn new_shared_log_buffer_for_test(capacity: usize) -> (SharedLogBuffer, LogReader, LogReader)`) をモジュール境界に露出し、外部モジュールの単体テストからテストできるようにする
  - 新規トレイトは作らない (`AsyncRead` を実装する構造体で足りる)
  - 新規マクロも作らない
- payload バッファに `Vec::with_capacity(payload_len as usize)` を使わないこと。破損入力・悪意のある入力で size に極端値が来ると OOM を招く。`Vec::new()` からの逐次 read か固定サイズ read で組む
- demux 内部で状態遷移用の Enum (例: `ReadingHeader` / `ReadingPayload`) を持つ場合は `#[derive(Debug, Clone, Copy)]` を基本とする
- 単体テストは demux モジュールと同じファイル内の `#[cfg(test)] mod tests` に置き、cfg 制約を付けない (両 OS で実行する純ロジックテスト)

### 3. `ContainerAsync` への配線 (上位)

`ContainerAsync::new` の macOS 側シグネチャ (`stdout_fd: Option<RawFd>, stderr_fd: Option<RawFd>`) を Linux でも維持できないため、両バックエンドを同一の抽象で扱う。

- 引数を Copy 不可 Enum に置き換える: 例 `enum ContainerLogSource { None, Fd { stdout: RawFd, stderr: RawFd }, DockerStream(Arc<DockerLogsHandle>) }`
  - macOS 側呼び出し 2 箇所 (`AsyncRunner::start` と `refresh_log_streams`) を `ContainerLogSource::Fd { .. }` に更新
  - macOS で `logs()` 呼び出しが失敗した場合の warn + None 続行パス (`async_runner.rs` 現行 217-220 付近) は `ContainerLogSource::None` にマップする
  - Linux 側呼び出し 1 箇所 (`AsyncRunner::start`) を `ContainerLogSource::DockerStream(..)` に更新
  - `Client::MacOs(_) / Client::Linux(_)` の match 分岐で既存パターンと揃うため、shiguredo-rust の「Enum で十分」方針にも合う
- `ContainerAsync` 内部フィールドの変更:
  - `stdout_fd: Mutex<Option<RawFd>>` / `stderr_fd: Mutex<Option<RawFd>>` を `log_source: Mutex<ContainerLogSource>` に統合する
  - `Drop` (現状は `libc::close(fd)`) は Enum を match して `Fd { .. }` variant では現状どおり FD を close、`DockerStream(..)` variant では `DockerLogsHandle` の drop で HTTP コネクションと demux タスクを閉じるように書き換える
  - `log_consumers: Option<Arc<Vec<Box<dyn LogConsumer>>>>` (現状 `#[cfg(target_os = "macos")]`) は Linux でも保持できるように `cfg` を外し、再 start 時の consumer 再武装 (8 節) を Linux 側でも成立させる。macOS 側の機能挙動は変わらない (Linux でもフィールドを保持するだけの差)
- `stdout()` / `stderr()` / `stdout_sync()` / `stderr_sync()` は `log_source` を match して以下を返す:
  - `Fd { .. }` variant では既存の `FdReader` / `FollowFdReader` (pread ベース) を返す
  - `DockerStream(..)` variant では 4 節の共有バッファに対する独立オフセットリーダー (`follow=true` 相当) または 4 節末尾の 1-shot リーダー (`follow=false` 相当) を返す
  - `None` variant では空リーダーを返す (現状 macOS の未取得パスと同等)
- `refresh_log_fds` は macOS 固有名なので `refresh_log_streams` に **リネーム** する。**macOS 側の実装内容は変更せず、関数名だけ変える** (Linux 側の実装は 8 節の 6 手順で追加する)
- `ready_conditions_require_log_fds` / `log_fd_required_error` は macOS 側でのみ意味を持つため関数ごと `#[cfg(target_os = "macos")]` で macOS 限定にする (Linux では対応する fail-fast そのものを 9 節で撤去する)
- `logs_terminated(&self) -> bool` (`pub(crate)`) を `ContainerAsync` に追加する。`log_source` を match して、`Fd`/`None` variant では常に `false`、`DockerStream` variant では demux 側の terminated フラグを返す。この関数は `LogWaitStrategy` の EOF 判定 (5 節) から呼ばれるのみで公開 API には出さない

### 4. 共有バッファ + 独立オフセットリーダー (follow=true 経路)

Linux の `follow=true` 経路は 1 本の HTTP ストリームを demux して次のように扱う。

**起動タイミング**: HTTP `?follow=true` セッションと demux タスクの spawn、および `LogConsumer` 配信タスクの初回 spawn は **`AsyncRunner::start` (async fn) 内** で行う。`log_consumers` (`ContainerRequest`) は `AsyncRunner::start` 内で `std::mem::take` して取り出し、共有バッファに接続する consumer 配信タスクを demux タスクと同時に spawn した上で、`Arc<DockerLogsHandle>` を組み立ててから `ContainerAsync::new` に渡す。`ContainerAsync::new` は同期関数のままにする (macOS の `spawn_exit_code_waiter` が `std::thread::spawn` で成立している構造を維持)。この結果、`ContainerAsync::new` の Linux 分岐からは consumer タスクの spawn ロジックは削除される (macOS 分岐の spawn は残す)。

**`std::mem::take(&mut image.log_consumers)` の cfg 分岐**: `async_container.rs` の該当行 (現状は cfg 無し) を `#[cfg(target_os = "macos")]` で macOS 限定に切り替える。Linux 側は `AsyncRunner::start` 内で take して `ContainerLogSource::DockerStream(..)` に格納した上で `ContainerAsync::new` に渡す。両プラットフォームで take が二重に走らない (実装バグにならない) ことを保証すること。

**`AsyncRunner::start` 内での後続失敗時のロールバック**: HTTP セッション / demux / consumer の spawn 後、`ContainerAsync` 構築 or ready 待ちなど後続ステップで失敗した場合は、`cleanup_on_ready_failure` / `remove` を呼ぶ前に必ず `log_stop.store(true)` + demux 側 `UnixStream::shutdown(Shutdown::Both)` を明示発行してからロールバックする。ContainerAsync が組み立てられる前の失敗も同様。

**起動失敗時のフォールバック**: `AsyncRunner::start` の Linux 分岐で HTTP `?follow=true` セッション起動に失敗した場合、macOS 側 (`async_runner.rs` 206-220 付近) と同型の 2 分岐にする:

- `ready_conditions_require_log_fds` に相当する Linux 側判定 (Log 待機 or `with_log_consumer` があるとき) が true の場合: `remove` してから明示エラーで start 全体を失敗させる
- そうでなければ: `tracing::warn!` を出して `ContainerLogSource::None` にフォールバックして start を続ける (`stdout()` / `stderr()` が空リーダーを返す macOS 相当の挙動)
- Linux 側判定関数 (例: `linux_log_stream_required`) は 9 節で撤去する fail-fast の判定を差し替える形で新設する (macOS 側 `ready_conditions_require_log_fds` の Linux 対応)

**共有バッファ**: stdout / stderr 別に `Arc<std::sync::Mutex<SharedLogBuffer>>` (`std::sync::Mutex`) を持つ。async 文脈で保持したまま await しない運用を守る (`poll_fill_buf` 内で「lock 取得 → 必要バイトのコピー or ヘッド位置更新 → 即 drop」で完結させ、lock 保持中に await を跨がない)。内部データ構造は `VecDeque<u8>` で「バッファ先頭の絶対オフセット (`u64`)」と「末尾の絶対オフセット (`u64`)」を持つ。各リーダーは「読み進めるべき絶対オフセット (`u64`)」を保持し、バッファ先頭より前なら先頭 drop が発生していたと判定する。

**追記通知 + 終端通知**: stdout / stderr 別に `tokio::sync::Notify` を持ち、demux タスクは以下 3 契機で該当ストリームの `notify_waiters` を必ず呼ぶ:

- 書き込み後 (追記通知)
- **TCP EOF を検知して terminated フラグを立てた直後** (終端通知)
- **`log_stop.load()` を検出して demux ループを抜ける直前 / `UnixStream::shutdown(Shutdown::Both)` 発行を検出したとき** (終端通知)

リーダー側は次の契約に従う:

- `Notify::notified()` を保持したまま `poll_fill_buf` を走らせ、追記待ちのとき Pending を返す
- **buffer 空 + terminated=true のとき `Poll::Ready(Ok(&[]))` を返す (EOF)。同条件で `Read::read` は `Ok(0)` を返す**

これがないと、demux 終了時に `notify_waiters` が呼ばれず `LogWaitStrategy::wait_until_ready` が永久に Pending のまま `logs_terminated()` の EOF 判定に到達しない。

**独立オフセット**: 各 `stdout()` / `stderr()` (`follow=true`) 呼び出しは、該当ストリームの共有バッファに対する独立オフセットの `AsyncBufRead` リーダーを返す。`with_log_consumer` 用の配信タスクも同じ経路で独立オフセットのリーダーを内部で保持する。

**共有バッファの上限とオーバーフロー方針**:
- 上限は既定 **8 MiB × stream** (stdout / stderr それぞれ)。将来の外部設定化は本 issue の範囲外
- 上限到達時は **先頭 drop (drop-oldest)** 方針とする (バッファ先頭の絶対オフセットを進める)
- 先頭 drop 済み領域を読もうとしたリーダーは **skip して継続** し (自リーダーの絶対オフセットをバッファ先頭に合わせる)、初回のドロップ検出時のみ `tracing::warn!` でログ出力する (Read エラーで terminate させると `stdout_to_vec` や `LogWaitStrategy` が「バッファ overflow のせいで」失敗する副作用が出るため。ユーザー観点で「取りこぼしはあるが読み進みは継続」の方が testcontainers 用途に合う)

**follow=false (1-shot) の扱い**: `stdout(false)` / `stderr(false)` は 4 節の共有バッファには接続せず、**呼び出しごとに新規 `?follow=false&tail=all` の 1-shot HTTP セッションを張る専用リーダー** を返す (常駐タスクの進行タイミングに依存しない決定的な挙動にする)。実装は 1 節末尾のとおり `spawn_blocking` 内で `UnixStream` + demux で完了まで読み切ってから `Vec<u8>` として返す。

**1-shot リーダーの状態機械**:

```
enum State {
    NotStarted,
    Running { rx: tokio::sync::oneshot::Receiver<std::io::Result<Vec<u8>>> },
    Done { data: Vec<u8>, pos: usize },
}
```

- 初回 `poll_read` で `NotStarted → Running` に遷移し `spawn_blocking` を起動する (リーダーを作って即 drop する場合に副作用を残さない遅延起動)
- `Running` では `rx` に対して `Future::poll` を呼び、Ready を得たら `Done` に遷移
- `Done` では `pos` を進めながら残りを返す
- `Drop` 時は `rx` を drop するだけ (`spawn_blocking` は完了まで走るが結果は捨てられる)

`stdout_to_vec()` を N 回呼ぶと N 回 HTTP セッションが張られる (呼び出し側で結果を保持する運用を想定。ループで N 回呼ぶ運用は避ける旨を Doc コメントに明記する)。

**`stdout()` / `stderr()` の Doc コメント**: 両プラットフォームを 1 コメントに 2 段落で列挙する (公開 API の doc は cfg で条件分岐させないのが慣習)。macOS 側は「pread により常にログ先頭から読む」、Linux 側 (`follow=true`) は「demux 開始時点以降のログを先頭から読む。8 MiB 上限で先頭が drop された場合は取りこぼした旨を `warn` ログに出力し、読み進みは継続する。再 start (`refresh_log_streams`) で demux 開始時点が更新されると、以前に取得した古いリーダーは新バッファに接続されない」、Linux 側 (`follow=false`) は「呼び出しごとに新規 HTTP セッションを張って現時点までの全ログを取得するため、ループで N 回呼ばず結果を保持すること」。

### 5. `LogWaitStrategy` の Linux 対応

`LogWaitStrategy::wait_until_ready` (`src/core/wait/log_strategy.rs`) の既存 EOF 判定は `exit_code_hint().is_some()` を経由するため、Linux では常に `None` で永久ポーリングになる。以下で対応する。

- demux タスクが TCP EOF を検知したら共有バッファに「terminated」フラグを立てる。3 節末尾の `ContainerAsync::logs_terminated()` が demux 側の terminated フラグを返す
- `LogWaitStrategy::wait_until_ready` 内の EOF 判定 (`log_strategy.rs:158` 付近) を `container.exit_code_hint().is_some() || container.logs_terminated()` に変える。これで:
  - macOS 挙動は `exit_code_hint` 経路のまま変わらない (`logs_terminated` は常に false)
  - Linux は demux terminated → `DRAIN_GRACE (2 秒)` → `WaitLogError::EndOfStream` の経路で自然に成立する
  - `LogWaitStrategy` 自体は cfg 分岐や Client 種別分岐を持たず、単一の抽象で両 OS を扱う
- `BothStd` は stdout / stderr の並列読取と合算カウントで動作する。demux が STREAM_TYPE で分離するため同じ payload が両方に書き込まれることは無い。macOS では Apple container の仕様上 stderr FD が VM の bootlog を指していたが、Docker Engine API は STREAM_TYPE で分離されるため Linux では `message_on_stderr` が本当に stderr のみに反応する (macOS より優れた挙動)

### 6. `SyncRunner` 経路 (blocking feature)

- `Container::stdout` / `Container::stderr` は `ContainerAsync::stdout_sync` / `stderr_sync` へ委譲する構造 (`src/core/containers/sync_container.rs`) を維持する
- Linux 側実装: **4 節の共有バッファに対して `std::io::BufRead` adapter を被せる** 方針で統一する (専用ワーカースレッド + 別 HTTP セッションは張らない)。`AsyncBufRead` 版と同じ共有バッファ・同じ terminated フラグを参照する
- **`tokio::sync::Notify` は `std::thread::park` と繋がらないため、sync 側では Notify を使わず `std::thread::park_timeout(50ms)` で周期起床する。起床のたびに buffer と terminated を再確認する。buffer 空 + terminated=true のとき `Ok(0)` を返す** (macOS 側 `FollowFdReader` の同期版と同じ periodic polling 方式で、latency は最大 50ms 程度)
- `follow=true` 側では demux が単一の常駐タスクで動くため、sync/async の複数リーダーが同じフレームを二重に受け取ることは無い
- `follow=false` (`stdout_to_vec` 系) は 4 節末尾の 1-shot 経路を async 版と共有する。sync 側は `block_on_runtime` (`sync_container.rs`) 経由で async 版を駆動する既存構造をそのまま使う
- `Handle::block_on` を `stdout_sync` 実装内で使うと `sync_container::block_on_runtime` の deadlock 検出 (共有 Runtime 再入検出) と衝突するため採用しない
- 既存の `blocking` 系 deadlock 検出テストが Linux でも継続 pass することを保証する

### 7. `with_log_consumer` の行分割セマンティクス

- 行末 `\n` を境界に 1 フレームとして配信する
- 行末の `\n` および `\r` を剥がす (macOS 側 `spawn_log_consumer_task` の現行挙動と同じ)
- TCP FIN で終端した後、最終行に `\n` が無い残余バイトは **捨てる** (macOS で `read_until(b'\n')` が 0 を返す挙動と同じ。互換性のため両プラットフォームで揃える)

### 8. 再 start / stop / rm / Drop 時のストリーム管理

**共通の前置き**: `stop()` / `rm()` / `Drop` のいずれの経路も冒頭で `log_stop.store(true)` と demux 側 `UnixStream` への `Shutdown::Both` を発行する (以下では経路固有の追加手順のみを記す)。

**`UnixStream::shutdown` を発行するための所有権モデル**: demux タスクは `spawn_blocking` 内で `UnixStream` を own する。外部スレッドから同じ socket に `shutdown` を打つため、**`DockerLogsHandle` の構築時に `UnixStream::try_clone()` した複製を保持** し、外部側から `.shutdown(Shutdown::Both)` を発行する。`try_clone` 失敗時は `.as_raw_fd()` を保持して `libc::shutdown(fd, SHUT_RDWR)` にフォールバックする。

**再 start 時の `refresh_log_streams` (Linux)**: race 防止と失敗時整合性のため、**旧経路の停止を `Docker::start_container` (docker restart) より前に行う**。以下の順序を守る。競合 (race) 防止のため、`log_source: Mutex<ContainerLogSource>` のロックを手順 4 で **取得したまま** 手順 5-6 まで保持する。

1. 旧 `log_stop.store(true)` を発行し、旧 demux タスクへ `Shutdown::Both` を発行して旧経路の read を打ち切る
2. `Docker::start_container` を呼んでコンテナを再起動する (失敗時は旧 stream は既に停止済みで整合性が取れる。呼び出し元に `Err` 伝播)
3. 新しい `log_stop: Arc<AtomicBool>` を用意し、新規 HTTP `follow=true` セッションを張り新 demux タスクを起動して新しい共有バッファを用意する。失敗時は `Docker::stop_container` を呼んで巻き戻し、`Err` を上位に伝播する (docker 側だけ動いて log が古い、というちぐはぐな状態を残さない)
4. 新規経路の起動が成功したら、`log_source` の Mutex を lock する
5. `log_source` の中身を新 `DockerStream(..)` に差し替え、Mutex を解放する
6. `log_consumers` が Some のとき、新共有バッファに接続する新 consumer 配信タスクを新 `log_stop` に紐づけて再 spawn する (macOS の `refresh_log_fds` 相当。macOS 側の `spawn_log_consumer_task` 2 本再 spawn と対応)

macOS 側の `refresh_log_streams` (旧 `refresh_log_fds`) は **中身を変更せず名前だけリネームする**。Linux 側の上記手順を macOS 側にも適用してはいけない。

**`stop()` (Linux)**: 共通の前置きのみ (追加手順なし)。

**`rm()` (Linux)**: 共通の前置きを **`client.remove` より前に** 実行する。remove の await 中に demux 側の完了フラグが立つため、後続の Drop での polling がほぼ即抜けする。

**`Drop` (Linux)**:
1. 共通の前置き (`log_stop.store(true)` + `Shutdown::Both`)
2. `tokio::runtime::Handle::try_current()` で分岐する:
   - **ランタイム内** (`Ok(_)`): 完了フラグを待たない (0035 の「Runtime 内 Drop は削除完了を保証しない」契約と同型で、ログタスク完了も保証しない)。`Shutdown::Both` は発行済みなので、demux/consumer タスクは Runtime shutdown 時に abort されるか、TCP FIN を検知して自然終了する
   - **ランタイム外** (`Err(_)`): demux/consumer タスクの完了フラグ (`Arc<AtomicBool>`) を polling する。各周は **`if flag.load() { return; } thread::sleep(50ms);` の順** で組み、フラグが既に立ち上がっているときは 0 回 sleep で即抜ける。最大 20 回 (計 1 秒) で timeout し、超過時は諦めて Drop を返す (`Shutdown::Both` は発行済み)。1 秒の根拠: Docker Engine の `POST /containers/{id}/stop` は SIGTERM 送信 → grace 期間 (既定 10 秒) 後 SIGKILL の流れで、`Shutdown::Both` を発行済みの demux は TCP FIN を数十 ms〜数百 ms で受け取れるため 1 秒あれば十分。CI テストの許容遅延にも収まる

**完了検証手段**: Docker Engine 側のログセッション状態を直接観測する API は無く、CI で `ss -x` / `lsof` は環境依存で不安定。以下の間接検証で置き換える (**完了条件では代表として `Arc<AtomicBool>` の完了フラグの観測** のみを要求する。`Arc::strong_count` と mpsc `RecvError` は補助的観測):
- demux タスク / consumer タスクの完了フラグ (`Arc<AtomicBool>`) が `true` になっていること
- `Arc::strong_count(&handle)` が期待値まで落ちていること
- mpsc 受信側で `RecvError` (送信側 drop) を観測できること

### 9. 既存 fail-fast の撤去

- `src/runners/async_runner.rs` の Linux 分岐で次の 2 箇所を撤去する
  - `ready_conditions_require_log_fds` による `WaitFor::Log` 拒否
  - `linux_unsupported_request_reason` の `log_consumers` チェックによる `with_log_consumer` 拒否
- 撤去箇所は 4 節「起動失敗時のフォールバック」に置き換える (Linux 側の新判定関数で fail-fast + `remove` を行う)
- `#[allow(...)]` ではなく `#[expect(...)]`、`.unwrap()` ではなく `.expect("MESSAGE")` を使う (shiguredo-rust 規約)

### 10. テスト実装ノート

- `LogConsumer` のテスト実装は「本番と同じインターフェイス (`LogConsumer` trait) を実装し、受信フレームを `Arc<Mutex<Vec<Vec<u8>>>>` に記録するだけ」の付随観測用の実装として書く。**モック・スタブとは異なる**: `DockerClient` や `AsyncRunner` を差し替えず、実 Docker Engine 越しに `LogConsumer` の実装として動作させるため、AGENTS.md「モックやスタブは絶対に利用しないこと」に抵触しない
- 統合テストで使う実イメージは **`alpine:latest`** + `echo` / `sleep` の shell command で固定する (CI の `Pull test images` に既に含まれる)。追加イメージは pull しない
- 統合テストごとに `with_startup_timeout(Duration::from_secs(N))` で **5〜15 秒程度** の短い値を明示的に設定し、`test-linux-docker` ジョブの `timeout-minutes: 15` に対して余裕を保つ
- 追加する統合テスト本数は 10 本超になるため、**cargo test の既定並列度 (`--test-threads`) で pass する設計** にする (`--test-threads=1` は要求しない)。共有リソース (port など) の競合を避けるためテストごとに `with_container_name` を明示しないこと (既存の慣習に従い名前は自動生成に任せる)
- demux パーサと共有バッファ + 独立オフセットリーダーの単体テストは cfg 制約を付けない (両 OS で実行する純ロジックテスト)

### 11. 範囲外 (本 issue で扱わない)

- TTY 有効経路 (`Config.Tty=true`, `application/vnd.docker.raw-stream`) の raw ストリーム対応 (現状 `ContainerRequest` / `ImageExt` に TTY を有効化する公開 API は無く、`CreateContainerBody` も `Tty` フィールドを Docker に送っていない ため、本 issue の実装は常に multiplex 前提で組む。外部で TTY 有効に作成されたコンテナに attach するケースは範囲外 = 挙動未定義とする)
- Docker Engine `/containers/{id}/wait` によるバックグラウンド exit code 監視 (`ContainerAsync::exit_code` の Linux 実装は open issue 0015 に切り出し済み)。本 issue はログ待機に必須の範囲だけを含める。**依存関係**: 5 節の設計 (`logs_terminated` フラグ経由の EOF 判定) により 0015 と独立に本 issue は完結する。0015 完了後は `exit_code_hint` 経由の EOF 判定も Linux で動くようになるが本 issue の実装には影響しない
- Docker Engine `/containers/{id}/logs` の `since` / `until` / `timestamps` 相当パラメータ
- 共有バッファ上限の外部設定化 (現状 `8 MiB × stream` の定数固定)

### 12. PR 運用方針

本 issue の実装規模は下位 HTTP streaming + demux + 共有バッファ + fan-out + LogWaitStrategy 拡張 + SyncRunner + 再 start / stop / rm / Drop フロー + Doc + TESTCONTAINERS 更新に及ぶ。shiguredo-git の「1 issue = 1 branch = 1 PR」原則を守りつつ、**単一 PR にまとめて proposal する** 方針とする (段階的な複数 PR には分割しない)。ただしレビュー可読性のため、以下の順序で **1 branch 内のコミットを段階的に積む** ことを推奨する:

1. demux モジュールと単体テスト
2. 共有バッファ + 独立オフセットリーダーと単体テスト
3. `docker_log_stream` の HTTP streaming (`follow=true` / `follow=false`)
4. `ContainerLogSource` enum 導入と macOS 側の呼び出し 2 箇所書き換え (macOS 側は既存挙動不変)
5. Linux 側 `AsyncRunner::start` の HTTP セッション起動 + fail-fast 撤去 + 起動失敗フォールバック
6. `LogWaitStrategy` の EOF 判定拡張 (`logs_terminated` 追加)
7. `refresh_log_streams` rename + Linux 側の 6 手順実装
8. `rm()` / `stop()` / `Drop` の Linux 側ストリーム後始末
9. SyncRunner (`stdout_sync` / `stderr_sync`) の Linux 実装
10. 統合テスト追加 + 既存 `log_wait_fails_fast_on_start` 削除
11. Doc / TESTCONTAINERS / CHANGES 更新

## 完了条件

### コード

- [ ] `AsyncRunner::start` の Linux 分岐で `ready_conditions_require_log_fds` による fail-fast を撤去する
- [ ] `AsyncRunner::start` の Linux 分岐で `linux_unsupported_request_reason` の `log_consumers` チェックを撤去する
- [ ] `ready_conditions_require_log_fds` / `log_fd_required_error` を `#[cfg(target_os = "macos")]` で macOS 限定にする
- [ ] Linux 側の新判定関数 (例: `linux_log_stream_required`) を追加し、Log 待機 or `with_log_consumer` 使用時に HTTP `?follow=true` セッション起動失敗を fail-fast + `remove` に振る
- [ ] Log 待機・consumer が無いときの HTTP セッション起動失敗は `tracing::warn!` + `ContainerLogSource::None` にフォールバックして start を続ける
- [ ] 新規モジュール `src/core/client/docker_log_stream.rs` を追加し、`/containers/{id}/logs` を `spawn_blocking` + `std::os::unix::net::UnixStream` + `tokio::sync::mpsc` (bounded) の構成で叩けるようにする
- [ ] `follow` フラグでリアルタイム追随 (`?follow=true`) と 1-shot 取得 (`?follow=false&tail=all`) を切り替える
- [ ] multiplex ヘッダ (8 バイト) を demux して stdout / stderr を分離する。TTY モードは範囲外
- [ ] demux モジュールに「共有バッファと独立オフセットリーダーを作る」薄い `pub(crate)` テスト用 API を露出する
- [ ] Content-Type が `application/vnd.docker.multiplexed-stream` / `application/octet-stream` / 空 のいずれかであれば multiplex demux で処理する。`application/vnd.docker.raw-stream` (TTY 有効) およびその他 (400 系応答含む) は `std::io::Error::other` に包んで `WaitLogError::Io` として上位に返す
- [ ] `ContainerAsync::new` の引数を `enum ContainerLogSource { None, Fd { .. }, DockerStream(..) }` に置き換え、macOS 側呼び出し 2 箇所と Linux 側呼び出し 1 箇所を更新する。macOS の `logs()` 失敗 warn 経路は `ContainerLogSource::None` にマップする
- [ ] `stdout_fd` / `stderr_fd` フィールドを `log_source: Mutex<ContainerLogSource>` に統合する
- [ ] `log_consumers` フィールドの `#[cfg(target_os = "macos")]` を外し、Linux でも保持する (macOS 側の機能挙動は不変)
- [ ] HTTP `?follow=true` + demux タスク起動 + `LogConsumer` 初回配信タスク spawn を `AsyncRunner::start` (async fn) 内で行い、`Arc<DockerLogsHandle>` を組み立ててから `ContainerAsync::new` に渡す
- [ ] `ContainerAsync::new` の Linux 分岐から consumer タスクの spawn ロジックを削除する (macOS 分岐の spawn は残す)
- [ ] `stdout()` (`follow=true`) / `stderr()` (`follow=true`) が共有バッファに対する独立オフセットの `AsyncBufRead` を返す
- [ ] `stdout()` (`follow=false`) / `stderr()` (`follow=false`) は呼び出しごとに新規 `?follow=false&tail=all` HTTP セッションを張る専用リーダーを返す (first poll で `spawn_blocking` を起動する遅延起動方式)。内部状態機械は `enum State { NotStarted, Running { rx: oneshot::Receiver<..> }, Done { data, pos } }` の 3 状態。Drop は `rx` を drop するのみ
- [ ] 追記通知 (`tokio::sync::Notify::notify_waiters`) は書き込み時・TCP EOF で terminated フラグを立てた直後・`log_stop.load()` を検出したときの 3 契機で必ず発行する
- [ ] リーダーは buffer 空 + terminated=true のとき `Poll::Ready(Ok(&[]))` (async) / `Ok(0)` (sync) を返す
- [ ] `DockerLogsHandle` は demux 側 `UnixStream` と外部側 `UnixStream` を `try_clone()` した 2 本で保持し、外部側から `.shutdown(Shutdown::Both)` を発行できるようにする。`try_clone` 失敗時は `.as_raw_fd()` + `libc::shutdown(fd, SHUT_RDWR)` にフォールバック
- [ ] `async_container.rs` の `std::mem::take(&mut image.log_consumers)` (現状 cfg 無し) を `#[cfg(target_os = "macos")]` で macOS 限定に切り替え、Linux 側は `AsyncRunner::start` 内で take する (macOS 側 consumer が二重 spawn / spawn 漏れしないことを担保する)
- [ ] SyncRunner の `stdout_sync` / `stderr_sync` の Linux 実装は `tokio::sync::Notify` を使わず `std::thread::park_timeout(50ms)` で周期起床し、buffer 空 + terminated=true のとき `Ok(0)` を返す
- [ ] `AsyncRunner::start` 内で HTTP セッション / demux / consumer タスクを spawn した後、後続ステップ (`ContainerAsync` 構築 or ready 待ち) で失敗した場合も `cleanup_on_ready_failure` / `remove` を呼ぶ前に必ず `log_stop.store(true)` + `Shutdown::Both` を発行してからロールバックする
- [ ] `Drop` の polling ループは各周を `if flag.load() { return; } thread::sleep(50ms);` の順で組み、フラグ既立ち上げ時は 0 回 sleep で即抜ける (最大 20 回で timeout)
- [ ] 共有バッファは `Arc<std::sync::Mutex<SharedLogBuffer>>` で stdout / stderr 別に 8 MiB 上限で管理し、上限超過時は先頭 drop する
- [ ] 追記通知は stdout / stderr 別の `tokio::sync::Notify` で行う
- [ ] 先頭 drop 済み領域を読もうとしたリーダーは skip して継続し、初回のドロップ検出時のみ `tracing::warn!` を出す
- [ ] `stdout_to_vec()` / `stderr_to_vec()` は `?follow=false&tail=all` の 1-shot 取得経路で `spawn_blocking` 内で完了まで読み切って `Vec<u8>` を返す
- [ ] `Container::stdout()` / `stderr()` (`blocking` feature) は 4 節の共有バッファに `std::io::BufRead` adapter を被せる方式で同じログを返す (専用ワーカースレッド + 別 HTTP セッションは張らない)
- [ ] `WaitFor::message_on_stdout` / `message_on_stderr` / `message_on_either_std` が Linux で成立する
- [ ] `ContainerAsync::logs_terminated(&self) -> bool` を `pub(crate)` で追加し、`log_source` の variant を match して macOS では常に false、Linux では demux 側の terminated フラグを返す
- [ ] `LogWaitStrategy::wait_until_ready` の EOF 判定を `exit_code_hint().is_some() || logs_terminated()` に拡張する (cfg 分岐なし、単一の抽象で両 OS を扱う)
- [ ] `with_log_consumer` が行単位 (`\n` 境界、行末 `\n`/`\r` 剥がし、末尾非改行残余は捨てる) で `LogFrame::StdOut` / `LogFrame::StdErr` を Linux で受信する
- [ ] `refresh_log_fds` を `refresh_log_streams` にリネームする (macOS 側呼び出し行 `async_container.rs:435` 付近も追随する)。macOS 側の実装内容は変更せず、関数名だけ変える
- [ ] Linux 分岐 `refresh_log_streams` で 8 節の 6 手順 (新 stop → 新経路起動 → Mutex lock → 旧停止 → 差し替え → consumer 再 spawn) を実装する
- [ ] `ContainerAsync::start` の Linux 分岐 (再起動時) で `refresh_log_streams` を呼ぶ (必須)
- [ ] `ContainerAsync::stop` の Linux 分岐で `log_stop.store(true)` + demux タスクの `UnixStream::shutdown(Shutdown::Both)` を発行する
- [ ] `ContainerAsync::rm` の Linux 分岐で `log_stop.store(true)` + `Shutdown::Both` を先に発行してから `client.remove` を呼ぶ
- [ ] `ContainerAsync::Drop` の Linux 分岐で `log_stop.store(true)` + `Shutdown::Both` を発行し、ランタイム内では完了フラグを待たず、ランタイム外では `std::thread::sleep(50ms)` × 20 回 (計 1 秒) で完了フラグを polling して timeout 超過時は諦める

### テスト

- [ ] 単体テスト (demux パーサ、cfg 依存無し): 正常フレーム / 部分ヘッダの境界越え / payload 境界越え / 巨大 `payload_len` (`Vec::with_capacity` を使わないことの回帰) / 不正 STREAM_TYPE / `payload_len=0` / 早期 EOF
- [ ] 単体テスト (共有バッファ + 独立オフセットリーダー、cfg 依存無し): 単一リーダーの読み進め / 複数リーダーの独立オフセット / 8 MiB 上限で先頭 drop / drop 済み領域を読もうとしたリーダーが skip して継続する / 追記通知で `Poll::Pending` から復帰
- [ ] 統合テスト (`tests/container_linux.rs`、`test-linux-docker` で実行、`alpine:latest` を使用):
  - alpine で `stdout_to_vec` が期待バイト列を返すこと
  - alpine で `stderr_to_vec` が stderr 単独の期待バイト列を返すこと (macOS では bootlog 混流だが Linux は分離できる)
  - `WaitFor::message_on_stdout` が成立すること
  - `WaitFor::message_on_stderr` が成立すること
  - `WaitFor::message_on_either_std` が成立すること (両ストリーム跨ぎ)
  - `with_log_consumer` で全行が受信できること (`Arc<Mutex<Vec<Vec<u8>>>>` に集める `LogConsumer` の実装をテスト内で用意する。モック・スタブではない)
  - コンテナ停止後にリーダーが EOF に達し、パターン不一致の `WaitFor::message_on_stdout` が `WaitLogError::EndOfStream` で終わること (5 節の `logs_terminated` 経路の回帰)
  - `follow=true` で追記が非同期に届くこと
  - `stop()` 後に demux / consumer タスクが終了していることを、統合テストからは **新規に `stdout(true)` リーダーを開くと即 `Ok(0)` (EOF) を返す** ことで間接観測する (`Arc<AtomicBool>` の完了フラグは `pub(crate)` で統合テストからは読めないため、demux モジュール内の `#[cfg(test)] mod tests` で `pub(crate)` API を経由して直接検証する)
  - `Drop` 後に同上 (`Container` を drop した後に新規リーダーを開いて即 EOF を確認する。ただし Drop 後は `Container` インスタンス自体が消えるため、確認は `Container` を drop する前に別変数に demux モジュール側の `pub(crate)` API 経由で完了フラグを共有しておく方式を単体テストとして書く)
  - 新規テスト `alpine_restart_refreshes_log_streams` を追加。Docker Engine の `POST /containers/{id}/start` は create 時の cmd を再実行するだけで start 時に新コマンドを差し替えられないため、テストは `sh -c 'echo "restart-marker-$(date +%s%N)"; sleep 60'` などタイムスタンプ入り marker を出す cmd を create 時に指定し、初回 start で marker A を取得 → `stop_with_timeout(Some(0))` → `start()` で再起動 → 新リーダーで marker B (marker A と異なる値) が読めることを検証する。旧リーダーは新バッファに接続されないため、新たに開いた 2 つ目のリーダー経由でのみ marker B が観測できることも併せて検証する
  - `blocking` feature 経由で `Container::stdout_to_vec()` / `stderr_to_vec()` が期待バイト列を返す統合テスト
- [ ] 既存テスト `log_wait_fails_fast_on_start` (`tests/container_linux.rs`) を削除する
- [ ] `blocking` feature の deadlock 検出テスト (共有 Runtime 再入検出) が Linux でも継続 pass する
- [ ] macOS 側の既存 `LogWaitStrategy` 関連テストが Linux 分岐追加後も pass する (5 節「macOS 挙動を変えない」の回帰。少なくとも `tests/container_macos.rs` の `with_log_consumer` + `message_on_stdout` 併用テスト、`WaitFor::Log` follow テスト、`xpc_alpine_stdout_to_vec_twice` 相当を含める)
- [ ] macOS ビルド (`cargo build`, `cargo test --no-run`) が両プラットフォームで pass する (`refresh_log_streams` rename に伴う macOS 側呼び出し行の追随を担保する)

### ドキュメント / 変更履歴

- [ ] `README.md` の Linux 警告文言を更新する: 現行「Linux ではライフサイクル (start / exec / stop / rm / Drop) まで動くが、ログ待機・ファイルコピー・bridge IP 取得などは未対応のままである」の「ログ待機・」を除去し「Linux ではライフサイクル (start / exec / stop / rm / Drop) + ログ関連まで動くが、ファイルコピー・bridge IP 取得などは未対応のままである」等の自然な文に整える
- [ ] `docs/TESTCONTAINERS.md` を実装後の実態に合わせて更新する。各 API 行の状態ラベルは基本「対応」に格上げする。ただし `stdout` / `stderr` / `stdout_to_vec` / `stderr_to_vec` については「対応 (8 MiB リング、上限超過で先頭 drop)」等の注記付き、または「部分対応」ラベルのいずれかで実装後の実態を正確に反映する:
  - 「Docker Engine API (Linux) の現状」段落の記述 (ログ FD を渡さない旨)
  - サマリ「Docker Engine API」節の「未配線・未実装が残るもの」からログ関連を除去
  - `with_log_consumer` 行の Docker 列
  - `stdout` / `stderr` / `stdout_to_vec` / `stderr_to_vec` の Docker 列
  - `WaitFor::Log` / `message_on_stdout` / `message_on_stderr` / `message_on_either_std` / `log()` の Docker 列
  - `LogWaitStrategy::stdout` / `stderr` / `stdout_or_stderr` の Docker 列
  - `LogFrame::StdOut` / `StdErr` / `LogSource::StdOut` / `StdErr` の Docker 列
- [ ] `CHANGES.md` の `## develop` に `[ADD]` エントリと担当者行 (`- @ユーザー名`) を追記する (shiguredo-changelog の並び順慣習に従い `[ADD]` は既存 `[FIX]` より上に置く)
- [ ] `ContainerAsync::stdout()` / `stderr()` の Doc コメントを 4 節末尾のとおり両プラットフォーム 2 段落形式で更新する
- [ ] `Container::stdout()` / `stderr()` (`src/core/containers/sync_container.rs`) の Doc コメントも同じ Linux 挙動 (共有バッファ・8 MiB 上限で先頭 drop 時は warn ログのみ・再 start で古いリーダーは新バッファに接続されない・`follow=false` は呼び出しごとに新規 HTTP セッション) を記述する

### 品質ゲート

- [ ] `cargo test --all-features` が pass する
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass する
- [ ] `cargo fmt --all -- --check` が pass する
- [ ] Linux CI (`test-linux-docker`) で追加した統合テストが実行され pass する

## 解決方法

Linux (Docker Engine API) バックエンドでログストリームを実装し、`stdout` / `stderr` /
`stdout_to_vec` / `stderr_to_vec` / `WaitFor::Log` / `with_log_consumer` / 同期 API を有効化した。

### 実装内容

- 新規モジュール `src/core/client/docker_log_stream.rs` を追加。`GET /containers/{id}/logs`
  を `spawn_blocking` 内の `UnixStream` で叩き、multiplex フレーム (8 バイトヘッダ) を demux
  して stdout / stderr 別の共有バッファ (ストリームあたり 8 MiB・drop-oldest) へ書き込む。
- 共有バッファに対する独立オフセットリーダーを実装。async は `tokio::sync::Notify` で追記待ち
  (register → buffer 再確認の順で wakeup 取りこぼしを防止)、sync は `park_timeout(50ms)` 周期起床。
  `follow=false` は呼び出しごとに新規 `?follow=false&tail=all` セッションを張る 1-shot 経路。
- `ContainerLogSource` enum (`None` / `Fd` (macOS) / `DockerStream` (Linux)) を導入し、
  `stdout_fd` / `stderr_fd` フィールドを `log_source: Mutex<ContainerLogSource>` に統合。
  `log_consumers` フィールドの macOS cfg を外し Linux でも保持。
- `AsyncRunner::start` の Linux 分岐でログセッションと LogConsumer 配信タスクを起動。
  Log 待機 / consumer 使用時は起動失敗を fail-fast + remove、それ以外は warn + 空リーダーに
  フォールバック。既存の Log 待機 / with_log_consumer の fail-fast は撤去。
- `LogWaitStrategy` の EOF 判定を `exit_code_hint().is_some() || logs_terminated()` に拡張
  (macOS 挙動は不変、Linux は demux 終端で EOF 判定)。
- 再 start 時の `refresh_log_streams` (Linux) を実装 (旧停止 → restart → 新セッション →
  差し替え → consumer 再 spawn)。`refresh_log_fds` からリネーム (macOS は中身不変)。
- `stop` / `rm` / `Drop` の Linux 分岐でログストリームを停止 (`log_stop` + `UnixStream` の
  `shutdown`)。Drop は Runtime 外で demux / consumer 完了を最大 1 秒 polling。
- レスポンスデコーダは `max_body_size` を無制限化 (既定 10 MiB のままだと合計ログ 10 MiB 超で
  ストリームが強制終了するため)。`max_buffer_size` は 64 KiB 据え置き。

### 変更ファイル

- `src/core/client/docker_log_stream.rs` (新規)、`src/core/client.rs`、`src/core/client/docker_client.rs`
- `src/core/containers/async_container.rs`、`src/core/containers/sync_container.rs` (doc)
- `src/runners/async_runner.rs`、`src/core/wait/log_strategy.rs`
- `tests/container_linux.rs`、`docs/TESTCONTAINERS.md`、`README.md`、`CHANGES.md`、`Cargo.toml` (tokio `sync` feature)

### 追加したテスト

- 単体テスト (`docker_log_stream.rs` 内 `#[cfg(test)]`): demux (正常 / チャンク境界 / 未知
  STREAM_TYPE / 巨大 payload_len / 空 payload)、共有バッファ (drop-oldest / skip / 独立オフセット /
  Notify 起床 / terminated EOF)、Content-Type 判定、異常 EOF でハングしない回帰 (実 Unix ソケット)、
  デコーダ無制限の回帰。
- 統合テスト (`tests/container_linux.rs`): `message_on_stdout` / `message_on_stderr` /
  `message_on_either_std` 成立、`stdout_to_vec` / `stderr_to_vec` の demux 分離、follow リーダー、
  `with_log_consumer` フレーム受信、`EndOfStream` (EOF)、再 start 再武装 (タイムスタンプ marker)、
  stop 後の新規リーダー EOF、同期 API。既存 `log_wait_fails_fast_on_start` は削除。

### 検証

- `cargo clippy --all-targets --all-features -- -D warnings` が macOS / Linux (`x86_64-unknown-linux-gnu`)
  両ターゲットで pass。`cargo fmt --all --check` pass。macOS の単体テスト pass。
- Linux 統合テストは Linux CI (`test-linux-docker`) で実行される (ローカルは macOS のためクロスコンパイル検証のみ)。
- `/review-diff-code` を 3 周実施し、致命的・重要の指摘 (oneshot 異常 EOF ビジーループ、デコーダ
  10 MiB 制限、起動エラー握り潰し、demux panic ガード、consumer 完了フラグ競合、fallback fd 安全性、
  キャンセル時タスクリーク、テスト欠落等) を全て修正済み。
