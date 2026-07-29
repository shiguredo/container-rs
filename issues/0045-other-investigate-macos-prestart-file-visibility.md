# 調査: macOS で初期プロセス起動前にファイルを可視化する経路を実測する

- Priority: Medium
- Created: 2026-07-23
- Completed: {YYYY-MM-DD}
- Model: Cursor Grok 4.5
- Branch: feature/debug-macos-prestart-file-visibility
- Polished: 2026-07-29
- Reporter: @voluntas

## 目的

macOS (Apple container) で、初期プロセスが起動時に読むファイルを利用側の起動待ちシェル無しで投入完了した状態にする経路を、4 候補について実測する。実測結果に応じて、選定候補に対応する実装 issue（機能追加・仕様変更・ドキュメント修正のいずれか）を `create-issue` スキル経由で起票する。全案 NG の場合は候補 4b を Priority Low の別 issue として起票する。

親 issue 0041 で Linux は `with_copy_to` を create 後・start 前完了の公開契約にした。macOS は同 issue の実測で `containerCopyIn` が running 必須と確定したため、同契約に揃えられなかった。本 issue はその残作業として、`with_copy_to` 以外の経路も含めて実測する。

## 優先度根拠

利用者フィードバック (mqtt-rs の macOS 移行)。Linux 側は 0041 で解消したが、macOS 側では次のケースが未解消のまま残る。

- **mosquitto**: 起動時に読む `mosquitto.conf` を差し替えたいが、`with_copy_to` は start 後投入のため差し替えが間に合わず、イメージ同梱 conf で起動してしまう
- **EMQX**: QUIC 用証明書を初期プロセス起動時に配置したいが、`with_copy_to` は start 後投入のため「EMQX の起動が遅いからたまたま間に合う」レース依存になる
- **両 OS 共通テスト**: 同一テストコードを両 OS で回す前提が崩れる

Medium (Linux は 0041 で解消済みのため High にはしない。ただし mqtt-rs の macOS ローカル開発体験に直接効くため Low にはしない)。

## 現状

### 0041 で確定した macOS 側の挙動

- `AsyncRunner::start` の macOS 分岐は `start_process` 成功後に `copy_to_sources`（XPC `containerCopyIn`）を呼ぶ (`src/runners/async_runner.rs` の `copy_to_sources` 呼び出し箇所)
- `bootstrap_container` 後・`start_process` 前の `containerCopyIn` は Apple container 1.0.0 / 1.1.0 (release) で `XPC error invalidState: container ... is not running` として失敗する。エラーは現在時制（`is`）で、`XpcClient::container_state` 内の `running = state == "running"` 判定も現在時制を採用している。実測ログ全文と build 番号は 0041 の `## macOS 実測結果` を参照
- `XpcClient::copy_in` 自体には状態ゲートは無く、拒否は Apple 側 apiserver のハンドラで行われている
- 現行文書は「Linux のみ起動前投入」を公開契約とし、macOS の start 後 copy は「レースあり」と明記済み (`README.md`、`docs/TESTCONTAINERS.md`、`src/core/image/image_ext.rs` の `with_copy_to` rustdoc)

### 既存 `Mount::bind_mount` の技術特性と制約

- `Mount::bind_mount(host_path, container_path)` は `MountType::Bind` として `mount_cfg`（`src/core/client/container_cfg.rs` の `mount_cfg`）で XPC の `virtiofs` にマップされ、`containerCreate` の段階で `ContainerCfg.mounts` に載る。したがって `bootstrap` / `start_process` より前に materialize される
- `tests/container_macos.rs` の `xpc_alpine_with_bind_mount` は新規パス（`/data/mounted.txt`）への単一ファイル bind mount が成立することを実証済み。ただし `WaitFor` を使わず起動後の `exec` で `cat` 検証する型のため、**初期プロセス起動時のタイミング保証は未検証**
- **virtiofs は既存 non-empty ディレクトリ配下の子だけを bind して差し替える形式ができない**。既存イメージが持つ `/etc/mosquitto/mosquitto.conf` を単一ファイル bind で差し替えると、`/etc/mosquitto` ディレクトリ全体が bind source で覆われる。新規パスへの単一ファイル bind は成立する
- `Mount::bind_mount` は host_path の絶対性を検証していない (`Mount::bind_mount` 関数)。相対パスは apiserver 側の cwd で解決されるため、`copy_in` の `absolutize_host_path` のような呼び出し側 cwd 基準の解決は自動では入らない。利用者・内部実装のいずれも host_path は絶対パスで渡す
- `AccessMode::ReadOnly` を指定すれば読み取り専用で mount できる（既定は ReadWrite）

### 本 issue で扱う残ケースと mqtt-rs 要件との対応

| mqtt-rs 要件 | 候補 1（bind mount 誘導） | 候補 2（with_copy_to 切替） | 候補 3（新 API） | 候補 4a（XPC 呼び位置変更） |
|------|------|------|------|------|
| mosquitto.conf 差し替え（既存パス単一 File） | 部分成立（`/etc/mosquitto` 全体差し替えで対処） | 検討（virtiofs の子だけ bind 不可制約と衝突） | 候補 2 と同じ制約 | 成立（既存経路のまま呼び位置が変わるだけ） |
| EMQX 証明書投入（新規 3 ファイル） | 成立（新規 dir を bind） | 検討（同上） | 候補 2 と同じ制約 | 成立 |
| インメモリ bytes 直接投入（`CopyDataSource::Data`） | 不成立（利用側で tempfile 必要） | 検討（一時ファイル寿命が変わる） | 検討 | 成立 |

0043（親ディレクトリ自動作成・ディレクトリ投入）は投入能力の話で、本 issue のタイミング契約とは別。ただし EMQX 証明書のような「複数ファイルを起動前に配置」の要件は 0043 と 0045 の両方に触れる。実装順序が「0045 → 0043」だと 0045 単体では複数 `with_copy_to` の連鎖で対応することになる。

## 設計方針

本 issue は調査 issue（other カテゴリ）。実装候補ごとの詳細設計は、実測完了後に選定候補の実装 issue（`create-issue` スキル経由で起票）で扱う。本 issue の範囲は「4 候補の実測 → 選定 → 実装 issue の起票」まで。

### 検討候補と実施順（破壊コスト昇順）

以下 4 案（候補 1〜4a）を **破壊コストの低い順** に実測し、成立した案を採用する。上位案が成立すれば以降の実測は不要。副次効果の差は実測結果に併記し、将来の再検討で判断根拠が残るようにする。候補 4b は本 issue のスコープ外の参考案（全案 NG 時のみ別 issue として起票）。

**候補 4a: Apple container 1.1 系で `containerCopyIn` の状態ゲートを再実測する（現状維持系・破壊なし）**

- 0041 では `containerBootstrap` 後・`containerStartProcess` 前のみ実測した
- 未実測なシーケンス:
  - **シーケンス 1**: `containerCreate` 直後（bootstrap より前）の `containerCopyIn`。0041 のエラーは現在時制 `is not running` のため、created 状態も同判定で拒否される見込みだが、apiserver 側の状態ゲート実装は未確認
  - **シーケンス 2**: `containerCreate` → 短命 `containerStartProcess` → `containerStop` → `containerCopyIn` → 再 `containerStartProcess`。成立見込みは極めて低い（現在時制の `is not running` 判定、および Apple container XPC が停止済みコンテナの再 start を公開しているか不明。加えて、初期プロセスが投入前に一瞬走ってしまう副作用で目的と衝突するため、成立しても採用しない）
- 副次効果: 既存 `containerCopyIn` の `fileMode` パラメータで mode 直接指定・書き込み反映あり・0027 の uid/gid 非反映は現状維持
- 成立時の実装 issue: 仕様変更（`feature/change-macos-copy-to-before-start-via-xpc`）

**候補 1: `with_mount(Mount::bind_mount)` を推奨手順として文書化する（doc 相当）**

- ライブラリ側の実装コードは変更しない
- `with_copy_to` の rustdoc / docs / SKILL / README に「macOS で起動時にファイルを見せたい場合は `Mount::bind_mount` を使うこと」の節を追加
- ホスト側一時ファイルは利用者側で管理する前提。`CopyDataSource::Data` 相当のインメモリ投入は本案では扱わない（利用者側で `tempfile` クレート等でラップ）
- 利点: `Mount::bind_mount` は Linux でも `HostConfig.Binds` にマップされるため、両 OS 共通テストコードがそのまま書ける（ディレクトリ全体 bind ケースに限る）
- 制約: virtiofs の既存パス子だけ bind 不可のため、mosquitto.conf 差し替えは `/etc/mosquitto` 全体をホスト側で組み立てる形になる。host_path は絶対パスかつ実ファイル / 実ディレクトリを渡す前提（symlink 挙動は未検証）
- 副次効果: 既存 `with_copy_to` の macOS 挙動は現状維持（start 後 copy・レースあり）
- 成立時の実装 issue: ドキュメント修正（`feature/update-macos-copy-to-doc-with-bind-mount`）

**候補 3: 新規公開 API `with_copy_to_before_start(target, source)` を追加する（add 相当）**

- 既存 `with_copy_to` の macOS 挙動（start 後 copy）は変更しない
- 新 API は macOS では bind mount 経由、Linux では既存 `copy_to_sources_linux` にディスパッチ
- `Image` トレイトに対称メソッド `copy_to_sources_before_start` を default 実装（`std::iter::empty()`）付きで追加
- `ContainerRequest` に `copy_to_sources_before_start: Vec<CopyToContainer>` を追加、image 側と request 側を chain する getter を実装
- 詳細設計（Deprecated 化しない根拠、rustdoc 書き分け、Linux 側の意味論同義扱いなど）は実装 issue で扱う
- 副次効果: 既存 `with_copy_to` の macOS 挙動は現状維持。新 API は macOS では書き込み反映抑止（後述の候補 2 と同じ意味論）
- 成立時の実装 issue: 機能追加（`feature/add-macos-copy-to-before-start`）

**候補 2: `with_copy_to` の macOS 実装を bind mount 経由に切り替える（change 相当）**

「透過的」ではない切替（意味論変化を伴う）。

- `AsyncRunner::start` の macOS 分岐で、`copy_to_sources` を「temp dir への `write_copy_data_temp` + `ContainerCfg.mounts` への virtiofs mount 注入」に切り替える
- 主な意味論変化:
  - 投入タイミング: `with_copy_to` の投入が start 前完了になる
  - 書き込み反映の抑止（`AccessMode::ReadOnly` 既定）: 「起動時に読んだ conf を後で書き換える」ユースケースは動かなくなる。Linux 側 `with_copy_to` は依然として書き込み可能なので、両 OS 非対称になる
  - mode 反映方法の変更（virtiofs 経由）: `CopyDataTempFile` の可視性・モジュール配置、Drop 順序、一時ファイル寿命の状態機械、`Image::copy_to_sources` への波及、`with_mount` との衝突検出などの詳細設計は実装 issue で扱う
  - symlink 拒否: Linux 側と挙動を揃える（実装 issue で `copy_to_sources_linux` の `symlink_metadata` 判定を移植）
- 副次効果: virtiofs 経由で mode/uid/gid が Apple 実装依存・書き込み反映抑止・0027 相互作用が変わる可能性
- 成立時の実装 issue: 仕様変更（`feature/change-macos-copy-to-before-start`）

**候補 4b: Apple container 1.2 系以降のリリース待ち（本 issue のスコープ外・参考）**

- 4a が不成立で、1.2 系で `containerBootstrap` 後・`containerStartProcess` 前の `containerCopyIn` が緩和される場合、既存経路の呼び位置変更だけで済む
- 1.2 系のリリース時期は未定。全案 NG となった場合、本 issue の担当が別 issue（Priority Low、Apple container 1.2 系の公開リリース確認後に着手）として起票する

### macOS 実測プロトコル

各候補について実施し、結果を `## macOS 実測結果` に追記する。実測環境は Apple container の build 番号（`container --version` / `container-apiserver --version`。build は release / preview / self-built のいずれかを明記）と macOS バージョンを記録する。0041 は 1.0.0 / 1.1.0 の 2 バージョンで実測した実績があるので、候補 4a は実測時点で入手可能な release と preview / self-built を可能な範囲で複数バージョン記録する。

**候補 4a の判定（Apple container 1.1 系）**

1. シーケンス 1: `containerCreate` → `containerCopyIn`（bootstrap より前）を試す。拒否時は orphan コンテナを `container delete <id>` で明示的に掃除する
2. シーケンス 2: 参考実測のみ（成立しても採用しない）
3. シーケンス 1 が成功し、初期プロセスが投入内容を読めれば 4a 成立
4. 全て失敗なら XPC エラー全文を記録し、候補 1 に進む

**候補 1 の判定**

1. `Mount::bind_mount` を使った mosquitto 相当の統合テスト（起動時読込 conf 差し替え）を試作する。target は `/etc/mosquitto` 全体を差し替える形と、新規パスへの単一ファイル bind の 2 種類を試す
2. `WaitFor::message_on_stdout(marker)` パターンで「初期プロセスが起動時に bind mount 内容を読めた」ことを確認する
3. mqtt-rs の残ケース（mosquitto.conf 差し替え・EMQX 証明書）が上記手順で成立するか、利用側手順の複雑度が受容可能かを mqtt-rs 側で再確認する
4. 両方成立すれば候補 1 で足りるとみなす。`Data` 相当が本案の対象外である点を Reporter が受容できるかを明示的に確認する

**候補 3 の判定**

1. 新規 API のシグネチャを `with_copy_to_before_start(target: impl Into<CopyTargetOptions>, source: impl Into<CopyDataSource>) -> ContainerRequest<I>` で仮固定する PoC を作る
2. macOS 側は候補 2 の実装経路（bind mount + `ContainerAsync` 保持）、Linux 側は既存 `copy_to_sources_linux` にディスパッチ
3. 統合テストは 0041 Linux 側と同型（`copy_to_visible_before_initial_process` / `copy_to_overwrite_visible_before_initial_process` の macOS 版、`with_copy_to_before_start` に置換）

**候補 2 の判定（PoC 不変条件リスト）**

「既存テスト全 pass」ではなく、次の不変条件を全て満たす PoC を作れれば成立とする。target のパスは virtiofs の子だけ bind 不可制約と衝突しないよう選ぶ（例: `/data/payload.txt` で mount target は `/data`）。

- **File ソース**: `.with_copy_to("/data/payload.txt", host_file_path)` が start 前に見え、初期プロセスから読める
- **Data ソース**: `.with_copy_to("/data/payload.txt", format!("{marker}\n").into_bytes())` が start 前に見え、初期プロセスから読める
- **既存パス上書き**: `/etc/motd` のような既存パス上書きが成立するか
- **mode 反映**: virtiofs 経由で `CopyTargetOptions.mode` が反映される（既存 `xpc_alpine_with_copy_to_target_options` の判定条件を満たす）
- **一時ファイル cleanup**: `ContainerAsync` の Drop / 明示 `rm` 完了後にホスト側の `shiguredo_container_copy_*.bin` が残らない（Keep 時は残る）
- **Drop 順序**: コンテナが稼働中に一時ファイルが unlink されない（remove 完了後 unlink）
- **書き込み反映抑止**: `AccessMode::ReadOnly` 既定で内部 mount を注入した場合、コンテナ内でファイルへ書き込むと失敗する
- **`with_mount` との衝突検出**: 同一 target への二重登録が `build_config` 段階で明示エラーになる

### 判定後の運用

- **候補 4a / 1 / 3 / 2 のいずれか成立**: 該当する実装 issue を `create-issue` スキル経由で起票し、その番号を本 issue の `## 調査結果` に記録する。本 issue はそこで close する
- **全案 NG**: 候補 4b を Priority Low の別 issue として `create-issue` スキル経由で起票し、番号を `## 調査結果` に記録する。あわせて `docs/TESTCONTAINERS.md` / rustdoc / README の macOS 制約記述を「現状維持で運用する」旨に整える別の実装 issue（`feature/update-...`）を起票し、番号を記録する。本 issue はそこで close する

## 完了条件

- 4 候補全てについて実測結果が `## macOS 実測結果` に追記されている（各候補について「実施日、macOS バージョン、Apple container CLI / apiserver のバージョンと build、試したシーケンス、成功/失敗、XPC エラー全文、判定」）。ただし上位案が成立した時点で以降の実測は不要（未実施として明示的に残す）
- 選定候補（分岐 A / B）または「全案 NG」の判定が本文に記録されている
- 判定に応じた実装 issue（分岐 A / B の場合は選定候補の実装 issue、全案 NG の場合は候補 4b + 文書化 issue）が `create-issue` スキル経由で起票され、番号が `## 調査結果` に記録されている
- 本 issue のブランチ（`feature/debug-macos-prestart-file-visibility`）で PoC 用の実測コードが変更されている場合、実測後に revert または別ブランチへ退避する（本 issue は調査 issue のためコード変更は成果物ではない。`feature/debug-` ブランチはマージしない）

## macOS 実測結果

（実測時に候補ごとに追記する。項目: 実施日、macOS バージョン、Apple container CLI / apiserver のバージョンと build（release / preview / self-built）、試したシーケンス、成功/失敗、XPC エラー全文、判定）

## 調査結果

（実測完了時に、選定候補と起票した実装 issue の番号を記録する）

## 解決方法

（実測・実装 issue 起票が完了した時点で過去形に書き直す）
