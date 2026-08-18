# 機能追加: Linux で copy_file_from / copy_to を実装する

- Priority: Medium
- Created: 2026-07-21
- Completed: 2026-07-23
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-copy
- Polished: 2026-07-22

## 目的

Linux (Docker Engine API) バックエンドで `ContainerAsync::copy_file_from` と `ImageExt::with_copy_to` (実体は `ContainerRequest::copy_to_sources`) を実装する。
Docker Engine API の archive エンドポイント (`GET/PUT /containers/{id}/archive`) を tar 経由で叩き、コンテナとホスト間の単一ファイル転送を配線する。
macOS (Apple container) は XPC `containerCopyIn` / `containerCopyOut` で既に配線済み。本 issue で Linux 側の穴を埋め、公開 API `copy_file_from` / `with_copy_to` を両プラットフォームで機能させる。

## 優先度根拠

ファイルコピーはテストのセットアップ (設定ファイルの投入) や検証 (生成物の取得) で頻繁に使う機能。
Linux 側の代替手段は限定的で、bind mount はホストパス依存で macOS と非対称、`exec` は `DockerClient::exec` (`src/core/client/docker_client.rs:220-318`) が stdin 送信非対応のため `sh -c 'cat > /dst'` パターンで代替できない。
一方で macOS 側は既に動作しており、Linux で未実装でも即座に破綻するわけではない。公開 API 面の整理 (`0028`) やクリーンアップ契約 (`0035`) と比較すると緊急性は低いため Medium。

## 現状

- `ContainerAsync::copy_file_from` の Linux 分岐は `Err(Error::other("copy_file_from() is not implemented on Linux"))` を返す (`src/core/containers/async_container.rs:277-281`)
- `AsyncRunner::start` の Linux 分岐は `container_req.copy_to_sources()` が非空なら `Err(Error::other("copy_to() is not implemented on Linux"))` を **`create_container` の前に fail-fast で** 返している (`src/runners/async_runner.rs:251-255`)。実装時はこの位置から実際の呼び出しを移す必要がある (詳細は「設計方針」参照)
- `CopyToContainer` は Linux で未配線のため `#[cfg_attr(target_os = "linux", expect(dead_code))]` でフィールド未読を許容している (`src/core/copy.rs:91-92`)
- `CopyFromContainerError` の `EmptyArchive` / `UnsupportedEntry` variant は現状構築経路がない (`src/core/copy.rs:257-258`)。issue `0009-other-dead-error-variants-policy.md` の 23 行目で「Linux (Docker) 実装が入れば構築経路が生まれる」と存置の根拠にされている
- `CopyToContainer` (`src/core/copy.rs:86-96`) と `CopyFileFromContainer` (`src/core/copy.rs:191-194`) の doc は「macOS では xxx」「Linux では未実装」のみを述べており、Linux 経路の記述を追記する必要がある。`src/core/copy.rs:1-5` のモジュール冒頭コメントは macOS 側の tar 構築未実装について述べているだけで Linux 未実装の記述はない (本 issue では変更しない)
- Docker Engine API には `GET /containers/{id}/archive?path={path}` (取り出し、レスポンスは `application/x-tar`) と `PUT /containers/{id}/archive?path={dir}` (投入、リクエストは tar) がある
- `DockerClient::request` は現状 GET / POST / DELETE のみを送っており PUT に対応していない (`src/core/client/docker_client.rs:363-383`)
- `encode_docker_api_request` は body 有り時に `Content-Type: application/json` を無条件で付与している (`src/core/client/docker_client.rs:397-401`)。tar 送信用に一般化が必要
- 既存テスト `tests/container_linux.rs:200-206` の `unimplemented_boundaries_return_err` は `copy_file_from` が Linux で Err を返すことを検証している
- `linux_unsupported_request_reason` (`src/runners/async_runner.rs:512-549`) は `copy_to_sources` を扱っていない (fail-fast は 253-255 行で別途行われているため)。本 issue では変更しない
- 主 crate `Cargo.toml` の `[dev-dependencies]` に `proptest` は入っていない (PBT は `pbt/` サブクレートに閉じている)
- `README.md:42` は Linux バックエンドを「Docker Engine (Docker Engine API 互換)」「Podman 等 API 互換ランタイムも可」と記載している。本 issue はテスト・検証範囲を Docker Engine (`/var/run/docker.sock`) のみに限定し、Podman compat socket は検証しない (「非対応」宣言はしない)

## 設計方針

### 全体スコープ

- 対象は **単一 regular file** のみ。ディレクトリの再帰コピー・シンボリックリンク・特殊ファイル (device / fifo / hardlink) は対象外
- tar / HTTP ともに **メモリ完結** (MVP)。ストリーミング対応 (大容量ファイル向け) は本 issue の範囲外。ペイロードサイズ上限は設けず、`Vec<u8>` として全読みする (OOM リスクは MVP の範囲で受容)
- 公開 API (`ContainerAsync::copy_file_from` / `ImageExt::with_copy_to` / `CopyTargetOptions` / `CopyFileFromContainer` / `CopyFromContainerError` / `CopyToContainerError`) のシグネチャは変更しない (variant 追加もしない)
- テスト・検証範囲は Docker Engine (`/var/run/docker.sock`) のみ。Podman compat socket は本 issue で検証しない (README で言及されている互換性そのものは変更しない)
- 大ディレクトリを `copy_file_from` に誤指定した場合、tar 全体を一旦メモリロードしてから `IsDirectory` を返す挙動になる (`X-Docker-Container-Path-Stat` を使った前判定は行わない)。本 issue では対応しないが、実運用で問題化した時点で別 issue として起票する

### tar 実装

配置と命名:

- 新ファイル `src/core/client/docker_tar.rs` を追加する (Docker 依存の色を持たせるため `src/core/client/` 配下、macOS 側からは共有しない)
- `src/core/client.rs` に `#[cfg(target_os = "linux")] pub(crate) mod docker_tar;` を追加する (既存の `docker_client` / `xpc_client` と同じくプラットフォーム別にゲートし、macOS lib ビルドで `dead_code` を回避する)
- 公開 (`pub(crate)`) 関数シグネチャは以下で固定する:
  ```rust
  pub(crate) fn build_single_file_ustar(
      name: &str,
      data: &[u8],
      mode: u32,
      uid: u32,
      gid: u32,
  ) -> std::result::Result<Vec<u8>, CopyToContainerError>;

  pub(crate) fn parse_first_regular_file_from_ustar(
      bytes: &[u8],
  ) -> std::result::Result<Vec<u8>, CopyFromContainerError>;
  ```
- 単体テスト・PBT はどちらも `src/core/client/docker_tar.rs` 内の `#[cfg(test)] mod tests` に置く。理由: `build_single_file_ustar` / `parse_first_regular_file_from_ustar` は `pub(crate)` のため、別 crate である `pbt/` からは呼べない。`shiguredo-rust` 規約「PBT は `pbt/` に置く」との衝突については、対象関数が crate 内部型 (`pub(crate)`) であることを優先し、テスト種別を混在させる
- 主 crate `Cargo.toml` の `[dev-dependencies]` に `proptest = "1.11"` を追加する (現状 `pbt/Cargo.toml` にのみ入っているため)
- PBT は `(name, data, mode, uid, gid)` の任意入力について encode → decode のラウンドトリップで元データが復元されることを検証する

実装する範囲 (書き込み・読み取り両方):

- typeflag `\0` / `0` (regular file): 書き込み・読み取り両対応
- typeflag `5` (directory): 読み取り検出のみ (`IsDirectory` 判定用)、書き込みは行わない
- ファイル名 (`name[100]`) の byte length は **99 以下を許容し、100 以上は拒否** する (POSIX 上は 100 バイトジャストで NUL 終端を省略可だが、GNU tar / BSD tar のうち NUL 終端を強く要求する実装との相互運用性を優先する)。判定は `basename.as_bytes().len() < 100` で行う。超過時は書き込み `CopyToContainerError::PathNameError` / 読み取り `CopyFromContainerError::UnsupportedEntry("long name")`
- `prefix[155]` フィールドは **使わない** (書き込みは basename のみ、dirname は Docker path クエリで別送するため 99 バイト以内に収まれば prefix は不要)
- `mode`: 8 進 ASCII 7 桁 + NUL (フィールド長 8 バイト、下位 12 ビット `mode & 0o7777`)
- `uid` / `gid`: 8 進 ASCII 7 桁 + NUL (フィールド長 8 バイト)
- `size`: 8 進 ASCII 11 桁 + NUL (フィールド長 12 バイト)
- `mtime`: 常に 0 (エポック) を書き込む。理由: テストの再現性を優先する。実運用でコピー後のファイル mtime が `1970-01-01` になる旨は `docs/TESTCONTAINERS.md` に注記する
- `magic`: `"ustar\0"` (フィールド長 6 バイト)、`version`: `"00"` (フィールド長 2 バイト)。読み取り時は `magic` を検証せず typeflag のみで判定する
- `uname` / `gname`: 空文字列 (フィールド長 32 バイト、全 NUL 埋め)。uid/gid を優先する
- チェックサム (`chksum[8]`):
  - **計算方法**: `chksum` フィールドの 8 バイトを ASCII 空白 (`0x20`) で埋めた状態のヘッダ 512 バイト全体について、各 byte を `u32` として符号なしで加算した合計値 (最大 `512 * 0xFF = 130560 = 0o377000`、6 桁 8 進に収まる)
  - **書き込み方法**: 上記合計を 8 進 ASCII 6 桁 + NUL + 空白の 8 バイト構造で書く。例: 合計が `0o123456` なら `[0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x00, 0x20]`
  - **読み取り方法**: `chksum[8]` の 8 バイトを、先頭の `0x20` / `'0'..='9'` を octal 数値として parse (最初の NUL または 0x20 で停止) し、計算値と **数値として等値比較** する。バイト単位の等値比較はしない (書式差「6 桁+NUL+空白」/「7 桁+NUL」のいずれも受理する)
- archive 終端: 512 バイト NUL ブロックを 2 個 (合計 1024 バイト)
- データ本体の 512 バイト境界パディング: データ長を 512 の倍数になるまで NUL で埋める
- basename バリデーション:
  - `basename.as_bytes()` に `0x00` を含む → `CopyToContainerError::PathNameError`
  - basename が空文字列 → `CopyToContainerError::PathNameError`
- デコーダは **`Vec::with_capacity()` を使わない** (`shiguredo-rust` 規約: 「入力バイナリデータをデコードする際にはメモリを事前割り当てするメソッドを原則使用しない」)。破損入力の size フィールドが極端に大きい場合の OOM を避けるため `Vec::new()` で足りない分だけ拡張する

実装しない範囲:

- PAX 拡張ヘッダ、GNU LongLink、sparse、hardlink、symlink、device、fifo
- 自前 gzip 展開・圧縮 (Docker からの `Content-Encoding: gzip` / `application/tar+gzip` 応答は `CopyFromContainerError::UnsupportedEntry("gzip")` で拒否)

### `DockerClient` へのメソッド追加

- `pub(crate) async fn copy_from(&self, id: &str, path: &str) -> Result<Vec<u8>>` — `GET /containers/{id}/archive?path=<percent-encoded path>` を送り、レスポンスボディの生 tar を返す
- `pub(crate) async fn copy_to(&self, id: &str, dir: &str, tar: Vec<u8>) -> Result<()>` — `PUT /containers/{id}/archive?path=<percent-encoded dir>&copyUIDGID=true` で tar を送る。`Content-Type: application/x-tar` を付ける
  - `copyUIDGID` の値: Moby の `daemon/archive.go` の `containerCopyExtract` を参照し、tar ヘッダの uid/gid が反映される値 (現状の Moby 実装では `copyUIDGID=true` が該当) を指定する。統合テストで `stat -c '%u %g'` により反映を実測検証すること
  - `path` クエリ値は `percent_encode_component` を使う (`/` は `%2F` になるが Docker Engine 側で URL-decode されて `/data` として解釈される)
- HTTP 経路の変更:
  - `encode_docker_api_request` の第 3 引数を `Option<(&[u8], &'static str)>` (body と content-type のタプル) に一般化する
  - `DockerClient::request` は **JSON 固定のまま残す** (シグネチャ変更しない)。tar 送信用に新規 `pub(crate) async fn request_with_content_type(&self, method: &str, path: &str, body: Vec<u8>, content_type: &'static str) -> Result<Response>` を追加する。既存の JSON 呼び出し (`create_container` / `start_container` / `stop` / `remove` / `exec` / `pull_image` / `container_state`) は変更しない。理由: 既存呼び出し 5〜8 箇所への影響を最小化し、`copy_to` 専用の書き換えに閉じ込める
  - PUT メソッドは既存 `Method::new(method)` が動的処理のため追加の変更は不要
  - chunked 応答は既存 `ResponseAccumulator` (`src/core/client/http_decode.rs` の `BodyKind::Chunked` 分岐) が復号済みのため独自処理は不要
  - `Expect: 100-continue` は送らない (`shiguredo_http11` 側でも自動送出しない)

### エラーマッピング

- レスポンス status code:
  - `200`: 成功
  - `404`: `ClientError::ContainerNotFound(id)` に寄せる (パス由来 404 と id 由来 404 の区別はレスポンス message でしか付かないが、実運用上大半は id 起因のため既存 `container_state` (`src/core/client/docker_client.rs:327-332`) と同じ扱いにする)
  - `200` / `404` 以外の 4xx / 5xx: `ClientError::Other` (既存 `stop` / `remove` と同じ扱い)
- tar パース時のエラー (`copy_file_from` 側):
  - 最初のエントリの typeflag が `5` (directory) → `CopyFromContainerError::IsDirectory`
  - regular file エントリが 1 つも無い (空 tar / 全て directory 等) → `CopyFromContainerError::EmptyArchive`
  - 未対応 typeflag (`1` hardlink / `2` symlink / `3` char device / `4` block device / `6` fifo など) → `CopyFromContainerError::UnsupportedEntry("<typeflag名>")`
  - チェックサム不整合・ヘッダ破損・データ長不足 → `CopyFromContainerError::Io(io::Error::new(InvalidData, ...))`
  - `Content-Encoding: gzip` 応答 → `CopyFromContainerError::UnsupportedEntry("gzip")`
- tar 構築時のエラー (`copy_to` 側):
  - basename が 100 バイト以上・空文字列・NUL 混入 → `CopyToContainerError::PathNameError`
  - `CopyDataSource::File` がディレクトリ・symlink・特殊ファイル → `CopyToContainerError::PathNameError`
  - ファイル読み取り I/O 失敗 → `CopyToContainerError::IoError`
- `copy_to_sources_linux` (`AsyncRunner` から呼ぶ関数) は `crate::core::error::Result<()>` を返し、`CopyToContainerError` は `Error::other(err)` でラップして伝播させる (macOS 側 `copy_file_from` が `Error::other(CopyFromContainerError::IsDirectory)` を使うのと同型)

### `ContainerAsync::copy_file_from` の Linux 分岐

- 事前検証: `source` (絶対パス想定) が空 or 相対パスなら `Error::other("copy_file_from path must be absolute")` で早期 Err にする (Docker Engine の 400 相当を明示エラーで返す)
- `client.copy_from(&id, &source).await?` で tar 生バイトを取得
- `parse_first_regular_file_from_ustar(&bytes)` の返した `Vec<u8>` を `Cursor::new(...)` に載せて `CopyFileFromContainer::copy_from_reader` に渡す (tokio 1.x は `Cursor<T: AsRef<[u8]> + Unpin>` に `AsyncRead` を実装。`Vec<u8>` は所有型で `'static` bound を満たす)
- ディレクトリ判定は tar 内 typeflag のみで行う (レスポンスヘッダ `X-Docker-Container-Path-Stat` は利用しない)
- 一時ファイルは使わない

### `AsyncRunner::start` の Linux 分岐 (`copy_to_sources`)

呼び出し位置と rollback:

- 現在の `src/runners/async_runner.rs:253-255` の fail-fast は **削除のみ** 行う (create 前チェックのため実際の copy 呼び出しは移せない)
- Linux 用の `copy_to_sources_linux` 関数を新設し、macOS 側 `copy_to_sources` (`src/runners/async_runner.rs:649-676`) と対称構造にする
- 呼び出し位置は **`start_container` (283-292 行) の成功後、`ContainerAsync::new` (301-308 行) の前** に挿入する (macOS 版が `start_process` 後に呼んでいる (155-157 行) のと揃える。停止中コンテナへの投入も Docker API 上は可能だが、macOS との挙動差を減らすため running 状態で行う)
- 失敗時は macOS 側 (157-168 行) と同じ Keep-gated 明示 rm パターンで rollback する:
  ```rust
  if let Err(e) = copy_to_sources_linux(&client, &id, &container_req).await {
      if matches!(
          crate::core::env::Config.command(),
          crate::core::env::Command::Remove,
      ) && let Err(rm_err) = client.remove(&id, true).await
      {
          tracing::warn!("failed to remove container {id} during rollback: {rm_err}");
      }
      return Err(e);
  }
  ```

`copy_to_sources_linux` の処理:

- macOS 側 (`src/runners/async_runner.rs:654-656`) と同様に `Vec<&CopyToContainer>` を先に `collect()` してから for ループする (iterator を future に持ち込むと `AsyncRunner::start` の future が `!Send` になるため)
- 各 `CopyToContainer` について:
  1. `target.path` を `dirname` (親ディレクトリ) / `basename` (ファイル名) に分割する:
     - `target.path` が絶対パスでない → `CopyToContainerError::PathNameError`
     - 末尾がスラッシュ (`file_name()` が `None` を返す) → `CopyToContainerError::PathNameError` (silent strip はしない)
     - `basename.as_bytes()` の長さが 100 以上、空、または NUL を含む → `CopyToContainerError::PathNameError`
  2. `source` から byte 列を取り出す:
     - `CopyDataSource::File(path)`: `tokio::fs::symlink_metadata(path)` で `file_type()` を取り、`is_symlink() == false` かつ `is_file() == true` を確認する。symlink・ディレクトリ・特殊ファイルなら `CopyToContainerError::PathNameError`。regular ならば `tokio::fs::read(path)` でメモリに読み込む (`tokio::fs::metadata` は symlink を追跡してしまうため使わない)
     - `CopyDataSource::Data(data)`: `data: &Vec<u8>` を借用で受け、`&[u8]` として tar buf へ書き込む (`Vec` 全体の move は発生しない。macOS 側の `write_copy_data_temp` に相当する一時ファイル書き出しは不要)
  3. `build_single_file_ustar(basename, data, target.mode, target.uid, target.gid)` で 1 エントリの tar を組む
  4. `client.copy_to(&id, dirname, tar).await?`

親ディレクトリの自動作成は行わない (macOS 側 `containerCopyIn` の `createParents: true` (`src/core/client/xpc_client.rs:125`) との挙動差は Linux 側の制約として `docs/TESTCONTAINERS.md` に注記する)。

### 実装順序と他 issue との相互作用

issue `0028-refactor-public-api-surface.md` (High) が `CopyTargetOptions.path` / `mode` / `uid` / `gid` を `pub(crate)` に落として `path()` / `uid()` / `gid()` アクセサを追加する。0028 が先にマージされた場合はアクセサ経由で参照し、本 issue が先の場合は直接フィールド参照で書いて 0028 でアクセサ化と同時に置換する (0028 の先行を推奨)。

## 完了条件

- [ ] `src/core/copy.rs` の `#[cfg_attr(target_os = "linux", expect(dead_code))]` (91-92 行) を削除する
- [ ] `src/core/copy.rs` の `CopyToContainer` の doc (86-89 行) と `CopyFileFromContainer` の doc (191-194 行) に Linux 実装 (Docker archive API + 自前 POSIX ustar) の記述を追記し、`CopyFileFromContainer` の「Linux では未実装」記述を実装済み記述に置き換える (モジュール冒頭 1-5 行は macOS 側の tar 未構築についての記述のため本 issue では変更しない)
- [ ] `src/core/client.rs` に `#[cfg(target_os = "linux")] pub(crate) mod docker_tar;` を追加し (既存 `docker_client` / `xpc_client` と同じゲートパターン)、新ファイル `src/core/client/docker_tar.rs` に `build_single_file_ustar` / `parse_first_regular_file_from_ustar` を実装する (デコード時は `Vec::with_capacity` を使わない)
- [ ] 主 crate `Cargo.toml` の `[dev-dependencies]` に `proptest = "1.11"` を追加する
- [ ] `src/core/client/docker_tar.rs` 内の `#[cfg(test)] mod tests` に単体テストと PBT を配置する:
  - 単体: 既知のバイト列との bit-exact 比較 (ゴールデンテスト)・チェックサム破損入力の decode 失敗・空 archive の `EmptyArchive`・typeflag `5` の `IsDirectory`
  - PBT: `(name, data, mode, uid, gid)` の任意入力について encode → decode のラウンドトリップ
- [ ] `DockerClient::copy_from(id, path)` / `DockerClient::copy_to(id, dir, tar)` を追加する
- [ ] `encode_docker_api_request` の第 3 引数を `Option<(&[u8], &'static str)>` に一般化する
- [ ] `DockerClient` に `pub(crate) async fn request_with_content_type` を追加し、`copy_to` から呼び出す (既存 JSON 呼び出しは変更しない)
- [ ] `ContainerAsync::copy_file_from` の Linux 分岐 (`src/core/containers/async_container.rs:277-281`) を Docker archive API 経由で単一 regular file を取得する実装に置き換える。事前に `source` の絶対性検証も入れる
- [ ] `AsyncRunner::start` の Linux 分岐で、`src/runners/async_runner.rs:253-255` の fail-fast を削除し、`copy_to_sources_linux` を `start_container` 成功後・`ContainerAsync::new` 前に挿入する (失敗時は Keep-gated 明示 rm rollback、iterator を collect してから回す)
- [ ] `CopyTargetOptions` の `mode` / `uid` / `gid` が tar ヘッダに反映され、コンテナ内ファイルに適用される (`copyUIDGID` クエリの値は実装時に Moby ソースと実測で決定する)
- [ ] `tests/container_linux.rs` に統合テストを追加する:
  - `with_copy_to` (File / Data) → コンテナ内で `cat` で内容確認 → `copy_file_from` (`Vec<u8>` / `PathBuf`) で回収の往復
  - `CopyTargetOptions::mode` (例: `0o755`) と `uid` / `gid` が `stat -c '%a %u %g'` で確認できる
  - `copy_file_from` の source にディレクトリ (`/etc`) を指定した場合 `IsDirectory` で拒否される
  - 存在しないコンテナ ID (`"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"` のような 64 桁 16 進) への `copy_file_from` が `ContainerNotFound` になる
- [ ] `tests/container_linux.rs` の `unimplemented_boundaries_return_err` から `copy_file_from` の Err 期待 (200-206 行) のみ削除する (`get_bridge_ip_address` / `exit_code` の Err 期待は残す)。sync 側 (`src/core/containers/sync_container.rs:207-213`) の統合テストは追加しない (async 実装が入れば自動対応するため)
- [ ] `docs/TESTCONTAINERS.md` の `copy_file_from` / `with_copy_to` 該当節を Linux 対応済みに更新し、Linux 側の制約 (親ディレクトリ自動作成なし、コピー後 mtime=epoch、大ディレクトリ誤指定時の帯域浪費) を明記する
- [ ] `CHANGES.md` の `## develop` に `[ADD]` エントリと担当者行 (`- @<ユーザー名>`) を追加する (種別順は `CHANGE → ADD → UPDATE → FIX`)
- [ ] `.github/workflows/ci.yml` の `test-linux-docker` job で `cargo test --all-features` が pass する (追加で必要なイメージがあれば `Pull test images` ステップを更新する)
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass する

## 解決方法

Linux (Docker Engine API) バックエンドで `copy_file_from` と `with_copy_to` を実装した。
Docker Engine API の archive エンドポイント (`GET/PUT /containers/{id}/archive`) を自前 POSIX
ustar 実装で叩き、単一 regular file の転送を配線した。

### 実装内容

- 新規モジュール `src/core/client/docker_tar.rs` を追加。`build_single_file_ustar` (1 エントリ
  ustar 構築) と `parse_first_regular_file_from_ustar` (先頭 regular file 展開) を実装。
  チェックサムは標準 tar 算法 (chksum 空白埋め 512 バイト合計)、`Vec::with_capacity` 不使用。
  uid/gid (8 進 7 桁) / size (8 進 11 桁) の上限超過は入口で fail-fast (暗黙桁詰め防止)。
- `encode_docker_api_request` の第 3 引数を `Option<(&[u8], &'static str)>` (body + content-type)
  に一般化。既存 JSON 呼び出しは不変。`request_with_content_type` / `copy_from` / `copy_to` を追加。
- `ContainerAsync::copy_file_from` Linux 分岐: source 絶対パス検証 → `copy_from` → ustar 展開 →
  `Cursor` 経由で `CopyFileFromContainer` へ。ディレクトリは `IsDirectory`、404 は `ContainerNotFound`。
- `copy_to_sources_linux` を新設し `AsyncRunner::start` の `start_container` 成功後・`ContainerAsync::new`
  前に挿入。target.path の dirname/basename 分割 (絶対パス必須・末尾スラッシュ拒否)、File ソースの
  symlink 検証、`mode` / `uid` / `gid` を tar ヘッダ + `copyUIDGID=true` で反映。失敗時は Keep-gated rm。
- 既存の copy_to fail-fast を削除。`CopyToContainer` の Linux dead_code attr を削除。doc を更新。

### 変更ファイル

- `src/core/client/docker_tar.rs` (新規)、`src/core/client.rs`、`src/core/client/docker_client.rs`
- `src/core/containers/async_container.rs`、`src/core/copy.rs`、`src/runners/async_runner.rs`
- `tests/container_linux.rs`、`docs/TESTCONTAINERS.md`、`CHANGES.md`、`Cargo.toml` / `Cargo.lock` (proptest dev-dep)

### 追加したテスト

- 単体テスト + PBT (`docker_tar.rs` 内 `#[cfg(test)]`): ゴールデン (フィールド配置)・ラウンド
  トリップ・チェックサム破損・空 archive・directory・gzip・特殊 typeflag・truncated header/data・
  不正 8 進・uid/gid 上限超過拒否・不正名拒否。PBT は (name, data, mode, uid, gid) のラウンドトリップ。
- 統合テスト (`tests/container_linux.rs`): Data/File コピー往復・PathBuf ターゲット・mode/uid/gid 反映
  (`stat` 検証)・`IsDirectory`・`ContainerNotFound`。`unimplemented_boundaries_return_err` から
  `copy_file_from` の Err 期待を削除。

### 検証

- `cargo clippy --all-targets --all-features -- -D warnings` が macOS / Linux 両ターゲットで pass。
  `cargo fmt --all --check` pass。macOS 単体テスト pass。
- レビューで Docker Desktop (Linux VM) 実地検証: 自然ファイルは typeflag '0' (PAX 無し)、`/etc` は
  typeflag '5' → `IsDirectory` を確認。
- `/review-diff-code` を実施し、致命的・重要 (uid/gid/size 暗黙桁詰め、エラーパステスト欠落、
  末尾スラッシュ silent strip) を修正済み。
- 設計上の既知の割り切り: 404 は id 由来・path 由来を区別せず `ContainerNotFound` に寄せる
  (既存 `container_state` と同じ扱い、issue 設計方針どおり)。メモリ完結 (MVP、ストリーミング未対応)。
- Linux 統合テストは Linux CI (`test-linux-docker`) で実行される。
