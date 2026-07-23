# 機能追加: with_copy_to の親ディレクトリ自動作成とディレクトリ投入に対応する

- Priority: Low
- Created: 2026-07-23
- Completed: {YYYY-MM-DD}
- Model: Claude Fable 5
- Branch: feature/add-copy-to-parents-and-directory
- Polished: 2026-07-23
- Reporter: @voluntas

## 目的

`with_copy_to` の投入能力を本家 testcontainers-rs 相当に近づける。公開 API シグネチャは変えず、拒否していた入力・失敗していたパスを受理する。

1. **親ディレクトリ自動作成** — コピー先の親がイメージ内に無くても成功する（Linux の穴埋め。macOS は既に `createParents: true`）
2. **ディレクトリ一括投入** — `CopyDataSource::File` がホストディレクトリのとき、配下を再帰的にコンテナへ投入する

TLS 証明書一式や設定ファイル群の投入で効く。

## 優先度根拠

利用者フィードバック (mqtt-rs の移行) 由来だが、既存パスへの単一ファイル投入で回避できているため「あればよい」水準。Low。

## 現状

### Linux (`copy_to_sources_linux`, `src/runners/async_runner.rs`)

- closed `0013` の割り切りで **単一 regular file のみ・親ディレクトリ自動作成なし**
- `target.path` を dirname / basename に分割し、basename 1 エントリの ustar を `PUT /containers/{id}/archive?path=<dirname>` で送る
- dirname が存在しないと Docker Engine がエラーを返す（`CHANGES.md` の 0041 `[CHANGE]` 括弧「親 dir 不在パスは失敗する」と同じ）。本 issue 完了時にこの括弧は能力追加と矛盾するため書き換えが必要（後述）
- `CopyDataSource::File` がディレクトリ / symlink / 非 regular file の場合は `PathNameError` で拒否（おおよそ `async_runner.rs:815-816`）
- 関数コメントも「単一 regular file のみ・親自動作成なし」と明記（おおよそ L769）
- タイミング契約は create 後・start 前（closed `0041`）。本 issue はタイミングを変更しない
- 制約注記は docs / SKILL / rustdoc に散在（更新対象は設計方針で列挙）

### macOS (`copy_to_sources` → XPC)

- `containerCopyIn` に `createParents: true` を渡しており、親ディレクトリは自動作成される（`src/core/client/xpc_client.rs:125`）
- `CopyDataSource::File` をそのまま `copy_in` に渡す。**クライアント側に regular file バリデーションは無い**。ディレクトリを渡したときの XPC 受理・再帰の有無は未確認
- `uid` / `gid` は XPC 非反映（open `0027` の範囲。本 issue では触らない）

### tar (`src/core/client/docker_tar.rs`)

- `build_single_file_ustar` は typeflag `'0'` の単一エントリのみ。`name` は basename、`prefix[155]` 未使用、99 バイト超過は `PathNameError`
- typeflag `'5'` (directory) は読み取り検出のみ（書き込みなし）
- tar / HTTP ともメモリ完結（0013 の MVP。本 issue も継承）

### 本家 testcontainers-rs との差分（事実）

| 項目 | testcontainers-rs | container-rs（現状） |
|------|-------------------|----------------------|
| PUT `path` | 常に `"/"` | dirname（親が無いと失敗） |
| tar エントリ名 | 先頭 `/` を除いた相対パス | basename のみ |
| ディレクトリソース | `append_dir_all` で再帰 | Linux は拒否 / macOS は未確認 |
| 親作成 | 相対パス抽出で実質対応 | Linux なし / macOS `createParents` |

## 設計方針

### スコープと非ゴール

- **対象**: Linux の親ディレクトリ自動作成、Linux のディレクトリ一括投入、macOS ディレクトリ投入の実測と公開契約の確定
- **非ゴール**: macOS の起動前可視性（0045）、macOS の uid/gid 反映（0027）、`CopyToContainerCollection`、ストリーミング tar、PAX / GNU LongLink、公開 API シグネチャ変更、variant 追加

### Linux PUT 方式（本家互換に固定）

**採用**: 常に `PUT .../archive?path=/&copyUIDGID=true`。tar エントリ名は `make_path_relative(target.path)`。

**不採用**: 存在する祖先を探してその dirname に PUT する方式。

`make_path_relative`: 先頭の `/` を **すべて** 除去する（本家同様 `trim_start_matches('/')`）。空になったら `PathNameError`（target が `/` のみ等）。

dirname / basename 分割は廃止する。target が `file_name()` を持たないパス（例: `/tmp/..`）は現行どおり拒否する。

### 親ディレクトリ自動作成（Linux）

相対パスの各祖先を typeflag `'5'` のエントリとしてファイルより先に書き、その後に regular file を書く。directory エントリのパスは **末尾 `/` 付き**に固定する（例: `var/` → `var/app/` → `var/app/conf/` → `var/app/conf/app.toml`）。directory エントリの `size` は常に 0。

### ディレクトリ投入のターゲット意味論（本家互換に固定）

ホストディレクトリ `source` と target `/data/certs` のとき、コンテナ内は `/data/certs/<source からの相対パス>`。

組み立て（単一ファイルと同じく `root` の祖先も書く）:

1. `root = make_path_relative(target)`（例: `data/certs`）
2. `root` の祖先 directory（例: `data/`）を単一ファイル規則と同じく先に `append_directory` する
3. 続けて `root/` の directory エントリを書く（空ディレクトリならここで終了）
4. 非空なら `source` を深さ優先で walk し、配下は `root + "/" + strip_prefix(source)`（ディレクトリは末尾 `/`、ファイルはなし）
5. `source` 自身のディレクトリ名は target に既に含まれている前提で二重に挟まない

- target は絶対パス必須。末尾 `/` は現行どおり拒否
- `CopyDataSource::Data` はディレクトリになり得ない（現状維持）

### ustar `name[100]` / `prefix[155]`（0013 からの変更点）

相対パス化により 99 バイトを超えうるため、**`prefix[155]` を使う**。

分割アルゴリズム（決定的・1 通り）:

1. 入力は相対パス（先頭 `/` なし）。directory なら末尾 `/` を含む
2. バイト長が 99 以下なら `name = 全体`、`prefix = 空`
3. 超過時: パス要素境界（`/`）での合法分割すべてを列挙し、そのうち **`name` のバイト長が最大**のものを採る（「末尾から最初の合法」＝最短 name ではない）。合法条件は `name` が 1..=99 バイト・`prefix` が 1..=155 バイト。`name` 空は禁止（現行 `build_single_file_ustar` と同じ）。directory の末尾 `/` は常に `name` 側に残し、分割点にしない
4. 読側結合は `prefix + "/" + name`（`prefix` 空なら `name` のみ）。分割点の `/` は `name` / `prefix` のどちらにも含めない
5. 要素境界で割れない（単一要素が 99 超など）場合は `PathNameError`
6. `name` は NUL 終端を要する現行どおり 99 バイト以下。`prefix` は 155 バイト以下（フィールド満杯可、余剰は NUL 埋め）

PAX / GNU LongLink は対象外。

### symlink / 特殊ファイル（固定）

`symlink_metadata` で判定し、symlink・device・fifo・socket・非 regular file（ディレクトリはディレクトリソース時のみ許可）は **検出時点で `PathNameError`（fail-fast）**。スキップしない。

walk 中の権限エラー・消失は `IoError`。配下パスの非 UTF-8 は `PathNameError`。

### `CopyTargetOptions` の適用規則

macOS の uid/gid 非反映は **現状維持**（0027 の範囲外）。

| 対象 | mode | uid / gid (Linux) | uid / gid (macOS) |
|------|------|-------------------|-------------------|
| 親作成用の中間 directory エントリ | `0o755` 固定 | `target.uid` / `target.gid` | 非反映 |
| ディレクトリソースの directory エントリ | `0o755` 固定 | 同上 | 非反映（XPC が dir を受理する場合。`fileMode` の効きは実測で文書化） |
| 単一 regular file / ディレクトリ配下の各 regular file | `target.mode`（現行どおり全ファイルに適用） | 同上 | `fileMode` = `target.mode` のみ |
| mtime | 常に 0（epoch）。0013 継承 | — | XPC 依存 |

container-rs は `mode` が常に設定済み（既定 `0o644`）のため、単一ファイル経路と揃え全 regular file に `target.mode` を適用する。docs に明記する。

### `docker_tar` API

本番の `copy_to_sources_linux` は **常に** 下記ビルダーだけを使う（単一 File/Data も中間 directory + file をここに積む）。

```rust
pub(crate) struct UstarBuilder { /* 内部バッファ */ }

impl UstarBuilder {
    pub(crate) fn new() -> Self { ... }

    pub(crate) fn append_directory(
        &mut self,
        relative_path_with_trailing_slash: &str,
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> Result<(), CopyToContainerError> { ... }

    pub(crate) fn append_file(
        &mut self,
        relative_path: &str,
        data: &[u8],
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> Result<(), CopyToContainerError> { ... }

    pub(crate) fn finish(self) -> Result<Vec<u8>, CopyToContainerError> { ... }
}
```

- `finish` は archive 終端の NUL 512 バイトブロックを **2 個だけ** 一度書く（0013 と同じ。`append_*` では書かない）
- `append_*` のたびにヘッダ + データをバッファへ書き、呼び出し側は Data を clone せず借用のまま渡せる（0013 の Data 借用を維持）
- ディレクトリ walk は「1 ファイル読んで `append_file` → 次へ」とし、全ファイルを同時に `Vec` へ載せない（ピークメモリは「最大ファイル + これまでの tar」程度。それでもメモリ完結 MVP として OOM は受容）
- `build_single_file_ustar` は内部で `UstarBuilder` に 1 file だけ積む薄いラッパに置き換えるか、単体テスト移行後に削除する。複数回呼んで連結してはならない（終端ブロックが多重になる）
- walk は `walkdir` を追加せず `std` の `read_dir` 再帰
- エントリ順: 祖先 directory → 子孫

### macOS ディレクトリ投入（決定木）

クライアント側に「緩めるバリデーション」は無い。実測するのは XPC のみ。

1. **実測手順**: alpine 系でホスト一時ディレクトリ（ネスト 1 段・ファイル 2 つ）を `with_copy_to("/data/fixture", dir)` し、start 後に内容を確認する。Apple container のバージョンは実測ログに残す
2. **成功時**: 公開契約を「macOS もディレクトリソース可」とし docs / SKILL / rustdoc に書く。追加のクライアントバリデーションは入れない
3. **失敗時**: docs / SKILL / rustdoc に「macOS は単一 regular file のみ（親作成は可）」と制約を書く。**事前拒否コードは足さない**（文書化のみで完了条件を満たす）

### `PathNameError` Display

現状 `"source is not a regular file: {s}"` 固定。ターゲット検証や長パス拒否にも使われており誤導になる。

- variant 追加はしない
- Display を `"copy path error: {s}"` に変更する（詳細は `s` 側）

### 文書・コメント更新対象

- `docs/TESTCONTAINERS.md`（制約が複数箇所）
- `skills/shiguredo-container/SKILL.md`
- `src/core/copy.rs` モジュール / `CopyToContainer` rustdoc
- `src/core/client/docker_tar.rs` モジュール doc
- `src/runners/async_runner.rs` の `copy_to_sources_linux` 関数コメント
- `src/core/image/image_ext.rs` の `with_copy_to` rustdoc（親作成・ディレクトリ投入・OS 差）
- `CHANGES.md`:
  - 新規 `[ADD]`（親ディレクトリ自動作成・ディレクトリ投入）
  - 既存 0041 `[CHANGE]` の括弧「親 dir 不在パスは失敗する」を削除または「能力は別 ADD」に書き換え、changelog 上で矛盾させない

### 後方互換

- 公開シグネチャは不変
- **セマンティック変更**: ホストディレクトリが Linux で受理される。`PathNameError` Display 文言が変わる
- 既存の「既存親への単一ファイル投入」は成功を維持（内部は path="/" + 相対パスに変わる）

## 完了条件

- Linux: 存在しない親配下への **単一 File** および **Data** 投入が成功する統合テストが pass する（例: `/var/container-rs-copy-missing/a.txt`）。中間ディレクトリの mode が `0o755`、uid/gid が `target` どおりであることも確認する
- Linux: 存在しない親配下への **ディレクトリ** 投入（ネスト・空ディレクトリ含む）が成功し、コンテナ内で directory / 全ファイルの内容と mode / uid / gid が上記規則どおり確認できる統合テストが pass する
- Linux: symlink / 特殊ファイルが `PathNameError`、name+prefix に収まらないパスが拒否されることが単体または統合で確認できる
- 既存の Linux copy 統合テスト（`/tmp/...` 往復・起動前可視・mode/uid/gid）が回帰しない
- `docker_tar` に directory（末尾 `/`）・複数エントリ・prefix 分割（最長 name）・終端ブロックの単体テストがある
- macOS: ディレクトリ投入の実測結果に応じ、対応（文書で可と明記）または制約の文書化が済んでいる
- 上記「文書・コメント更新対象」がすべて現状と矛盾しない（0041 CHANGE の括弧処理を含む）
- `CHANGES.md` に `[ADD]` エントリがある
- `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass する

## 解決方法

設計方針どおり、`UstarBuilder` を追加して `copy_to_sources_linux` を path="/" + 相対パスに置き換え、macOS は実測のうえ文書化する。docs / SKILL / rustdoc / CHANGES（0041 括弧含む）をセット更新する。
