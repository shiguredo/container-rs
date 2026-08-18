# 調査: macOS で初期プロセス起動前にファイルを可視化する経路を実測する

- Priority: Medium
- Created: 2026-07-23
- Completed: 2026-08-01
- Model: Cursor Grok 4.5
- Branch: feature/debug-macos-prestart-file-visibility
- Polished: 2026-08-01
- Reporter: @voluntas

## 目的

macOS (Apple container) で、初期プロセスが起動時に読むファイルを利用側の起動待ちシェル無しで投入完了した状態にする経路を、4 候補について実測する。実測結果に応じて、選定候補に対応する実装 issue（機能追加・仕様変更・ドキュメント修正のいずれか）を `create-issue` スキル経由で起票する。全案 NG の場合は候補 4b と macOS 制約の文書化 issue を起票する。

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
- `bootstrap_container` 後・`start_process` 前の `containerCopyIn` は Apple container 1.0.0 / 1.1.0 で `XPC error invalidState: container ... is not running` として失敗する（build 種別は 1.1.0 が release と記録済み。1.0.0 は 0041 に build 記録なし）。エラーは現在時制（`is`）で、`XpcClient::container_state` 内の `running = state == "running"` 判定も現在時制を採用している。実測ログ全文と build 番号は 0041 の `## macOS 実測結果` を参照
- `XpcClient::copy_in` 自体には状態ゲートは無く、拒否は Apple 側 apiserver のハンドラで行われている
- 現行文書は「Linux のみ起動前投入」を公開契約とし、macOS の start 後 copy は「レースあり」または「利用側の起動待ち等が別途必要になり得る」と明記済み (`docs/TESTCONTAINERS.md` は「レースあり」、`README.md` と `src/core/image/image_ext.rs` の `with_copy_to` rustdoc は「起動待ち等が別途必要になり得る」)

### 既存 `Mount::bind_mount` の技術特性と制約

- `Mount::bind_mount(host_path, container_path)` は `MountType::Bind` として `mount_cfg`（`src/core/client/container_cfg.rs` の `mount_cfg`）で XPC の `virtiofs` にマップされ、`containerCreate` の段階で `ContainerCfg.mounts` に載る。したがって `bootstrap` / `start_process` より前に materialize される
- `tests/container_macos.rs` の `xpc_alpine_with_bind_mount` は新規パス（`/data/mounted.txt`）への単一ファイル bind mount が成立することを実証済み。ただし `WaitFor` を使わず起動後の `exec` で `cat` 検証する型のため、**初期プロセス起動時のタイミング保証は未検証**
- **virtiofs は既存 non-empty ディレクトリ配下の子だけを bind して差し替える形式ができない**（未検証の仮定。リポジトリ内に実測記録・一次資料は無く、候補 1 の実測で再検証する）。既存イメージが持つ `/etc/mosquitto/mosquitto.conf` を単一ファイル bind で差し替えると、`/etc/mosquitto` ディレクトリ全体が bind source で覆われる。新規パスへの単一ファイル bind は成立する
- `Mount::bind_mount` は host_path の絶対性を検証していない (`Mount::bind_mount` 関数)。相対パスは apiserver 側の cwd で解決されるため、`copy_in` の `absolutize_host_path` のような呼び出し側 cwd 基準の解決は自動では入らない。利用者・内部実装のいずれも host_path は絶対パスで渡す
- `AccessMode::ReadOnly` を指定すれば読み取り専用で mount できる（既定は ReadWrite）

### 本 issue で扱う残ケースと mqtt-rs 要件との対応

| mqtt-rs 要件 | 候補 1（bind mount 誘導） | 候補 2（with_copy_to 切替） | 候補 3（新 API） | 候補 4a（XPC 呼び位置変更） |
|------|------|------|------|------|
| mosquitto.conf 差し替え（既存パス単一 File） | 部分成立（`/etc/mosquitto` 全体差し替えで対処） | 検討（virtiofs の子だけ bind 不可制約と衝突） | 候補 2 と同じ制約 | 成立（既存経路のまま呼び位置が変わるだけ） |
| EMQX 証明書投入（新規 3 ファイル） | 成立（新規 dir を bind） | 検討（同上） | 候補 2 と同じ制約 | 成立 |
| インメモリ bytes 直接投入（`CopyDataSource::Data`） | 不成立（利用側で tempfile 必要） | 検討（一時ファイル寿命が変わる） | 検討 | 成立 |

0043（親ディレクトリ自動作成・ディレクトリ投入）は投入能力の話で、本 issue のタイミング契約とは別。0043 は実装済み（macOS は Apple container 1.1.0 でホストディレクトリの再帰投入を実測済み）のため、EMQX 証明書のような「複数ファイルの配置」は既存 `with_copy_to` のディレクトリ投入で対応できる。本 issue が扱うのは、その配置を「起動前」に行う経路の実測に限る。

## 設計方針

本 issue は調査 issue（other カテゴリ）。実装候補ごとの詳細設計は、実測完了後に選定候補の実装 issue（`create-issue` スキル経由で起票）で扱う。本 issue の範囲は「4 候補の実測 → 選定 → 実装 issue の起票」まで。

### 検討候補と実施順（破壊コスト昇順）

以下 4 案（候補 1〜4a）を **破壊コストの低い順** に実測し、成立した案を採用する。上位案が成立すれば以降の実測は不要。副次効果の差は実測結果に併記し、将来の再検討で判断根拠が残るようにする。候補 4b は本 issue のスコープ外の参考案（全案 NG 時のみ別 issue として起票）。

**候補 4a: Apple container 1.1 / 1.2 系で `containerCopyIn` の状態ゲートを再実測する（現状維持系・破壊なし）**

- 0041 では `containerBootstrap` 後・`containerStartProcess` 前のみ実測した
- 未実測なシーケンス:
  - **シーケンス 1**: `containerCreate` 直後（bootstrap より前）の `containerCopyIn`。0041 のエラーは現在時制 `is not running` のため、created 状態も同判定で拒否される見込みだが、apiserver 側の状態ゲート実装は未確認
  - 参考: `containerCreate` → 短命 `containerStartProcess` → `containerStop` → `containerCopyIn` → 再 `containerStartProcess` のシーケンス 2 は、**実測対象外とする**。初期プロセスが投入前に一瞬走ってしまう副作用で本 issue の目的（起動前にファイルを読ませる）と構造的に衝突し、成立しても採用しないため実測しない
- 副次効果: 既存 `containerCopyIn` の `fileMode` パラメータで mode 直接指定・書き込み反映あり・0027 の uid/gid 反映（コピー後 exec の `chown_after_copy`。非ゼロ時のみ）は現状維持。ただし 4a 成立時は `chown_after_copy` の exec が start 前のコンテナに対して実行されるため、boot 前の process 生成が拒否される場合は uid/gid が黙って非反映になる（chown 失敗は warn のみ）。この挙動は 4a の実測で確認し、`## macOS 実測結果` に記録する
- 成立時の実装 issue: 仕様変更（`feature/change-macos-copy-to-before-start-via-xpc`）

**候補 1: `with_mount(Mount::bind_mount)` を推奨手順として文書化する（doc 相当）**

- ライブラリ側の実装コードは変更しない
- `with_copy_to` の rustdoc / docs / SKILL / README に「macOS で起動時にファイルを見せたい場合は `Mount::bind_mount` を使うこと」の節を追加
- ホスト側一時ファイルは利用者側で管理する前提。`CopyDataSource::Data` 相当のインメモリ投入は本案では扱わない（利用者側で `tempfile` クレート等でラップ）
- 利点: `Mount::bind_mount` は Linux でも `HostConfig.Binds` にマップされるため、両 OS 共通テストコードがそのまま書ける（ディレクトリ全体 bind ケースに限る）
- 制約: virtiofs の既存パス子だけ bind 不可のため、mosquitto.conf 差し替えは `/etc/mosquitto` 全体をホスト側で組み立てる形になる。host_path は絶対パスかつ実ファイル / 実ディレクトリを渡す前提（symlink 挙動は未検証）
- 副次効果: 既存 `with_copy_to` の macOS 挙動は現状維持（start 後 copy・レースあり）。virtiofs はコンテナ稼働中にホスト側ファイルを書き換えるとコンテナ内の見え方が即時変わる（`with_copy_to` のスナップショット投入とは意味論が異なる）。この差異は候補 1 成立時の文書化（docs / rustdoc / README）で明記する
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
  - 書き込み反映の抑止（`AccessMode::ReadOnly` 既定）: 「起動時に読んだ conf を後で書き換える」ユースケースは動かなくなる。Linux 側 `with_copy_to` は依然として書き込み可能なので、両 OS 非対称になる。ReadOnly 既定の根拠（virtiofs がホスト側ファイルを直接見せるため、コンテナからの書き込みがホスト側一時ファイルへ反映されるのを防ぐ、等）は実測時に確定して本 issue に記録し、実装 issue の設計に引き継ぐ
  - mode 反映方法の変更（virtiofs 経由）: `CopyDataTempFile` の可視性・モジュール配置、Drop 順序、一時ファイル寿命の状態機械、`Image::copy_to_sources` への波及、`with_mount` との衝突検出などの詳細設計は実装 issue で扱う
  - symlink 拒否: Linux 側と挙動を揃える（実装 issue で `copy_to_sources_linux` の `symlink_metadata` 判定を移植）
- 副次効果: virtiofs 経由で mode / uid / gid が Apple 実装依存になる・書き込み反映抑止・0027 の `chown_after_copy` が読み取り専用 mount 上の chown になり失敗し得る（現状は warn のみ。候補 2 は `copy_in` 経路を mount 注入に置き換えるため、`chown_after_copy` 相当の後処理を継続する場合の言及）等の相互作用が変わる可能性
- 成立時の実装 issue: 仕様変更（`feature/change-macos-copy-to-before-start`）

**候補 4b: Apple container 1.2 系以降のリリース待ち（本 issue のスコープ外・参考）**

- 1.2.0 は公開済みだが、`containerBootstrap` 後・`containerStartProcess` 前の `containerCopyIn` が 1.2 系で緩和されるかは未確認（候補 4a の実測で確認する）
- 4a が不成立で、1.2 系で同シーケンスの `containerCopyIn` が緩和される場合、既存経路の呼び位置変更だけで済む
- 全案 NG となった場合、本 issue の担当が別 issue（Priority Low、Apple container 1.2 系で `containerCopyIn` の緩和が確認されたリリースの公開後に着手）として起票する

### macOS 実測プロトコル

各候補について実施し、結果を `## macOS 実測結果` に追記する。実測手段は次のとおり
- 候補 4a シーケンス 1（`containerCreate` 直後の `containerCopyIn`）: CLI（`container create` / `container cp` / `container delete`。成功判定時は `container run` 相当で初期プロセスを起動し、起動時読込を確認する）
- 候補 4a の 1.2 系での 0041 と同シーケンス再実測・4a 成立時の chown 挙動確認: **XPC 直接呼び出しの PoC コード**（`containerBootstrap` 後の状態は CLI では作れないため。0041 の実測もライブラリの XPC 直接呼び出しだった）。`chown_after_copy` はライブラリ実装であり CLI 経路には無いため、この確認は PoC コードでのみ可能
- 候補 1 / 2 / 3: PoC コード（本 issue の `feature/debug-` ブランチ上）

実測環境は Apple container の build 番号（`container --version` / `container-apiserver --version`。build は release / preview / self-built のいずれかを明記）と macOS バージョンを記録する。0041 は 1.0.0 / 1.1.0 の 2 バージョンで実測した実績があるので、候補 4a は実測時点で入手可能な release（1.2.0 を含む）と preview / self-built を可能な範囲で複数バージョン記録する。

**候補 4a の判定（Apple container 1.1 / 1.2 系）**

1. シーケンス 1: `containerCreate` → `containerCopyIn`（bootstrap より前）を試す。拒否時は orphan コンテナを `container delete <id>` で明示的に掃除する。CLI がクライアント側で拒否する場合は XPC 直接呼び出しの PoC でも確認する（4a の真偽は apiserver 側の判定で決まるため）
2. 1.2 系では、0041 と同シーケンス（`containerBootstrap` 後・`containerStartProcess` 前）の `containerCopyIn` も再実測し、候補 4b の判定材料（1.2 系での緩和の有無）として `## macOS 実測結果` に記録する
3. シーケンス 1 が成功し、初期プロセスが投入内容を読めれば 4a 成立。成功時も orphan コンテナを `container delete <id>` で掃除する
4. シーケンス 1 が失敗しても、1.2 系で bootstrap 後・start 前の `containerCopyIn` が成功した場合（4b の緩和条件を満たす）は、既存経路の呼び位置変更の実装 issue（仕様変更 `feature/change-macos-copy-to-before-start-via-xpc`）を起票して分岐 A として closed にする
5. 全て失敗なら XPC エラー全文を記録し、候補 1 に進む

**候補 1 の判定**

1. `Mount::bind_mount` を使った mosquitto 相当の統合テスト（起動時読込 conf 差し替え）を試作する。target は次の 3 種類を試す
   - `/etc/mosquitto` 全体を差し替える形
   - 新規パスへの単一ファイル bind
   - あわせて **既存パスへの単一ファイル bind**（`/etc/mosquitto/mosquitto.conf` 直接 bind）も試し、`## 現状` の「子だけ bind 不可」制約を検証する（制約が誤っていれば mosquitto 要件が「部分成立」から「成立」に変わるため、対応表の判定にも影響する）
2. `WaitFor::message_on_stdout(marker)` パターンで「初期プロセスが起動時に bind mount 内容を読めた」ことを確認する
3. mqtt-rs の残ケース（mosquitto.conf 差し替え・EMQX 証明書）が上記手順で成立するか、利用側手順の複雑度が受容可能かを mqtt-rs 側で再確認する
4. mosquitto と EMQX の両ケースが成立すれば候補 1 で足りるとみなす。`Data` 相当が本案の対象外である点を Reporter が受容できるかを明示的に確認する（受容できない場合は候補 3 の実測へ進む）

**候補 3 の判定**

1. 新規 API のシグネチャを `with_copy_to_before_start(target: impl Into<CopyTargetOptions>, source: impl Into<CopyDataSource>) -> ContainerRequest<I>` で仮固定する PoC を作る
2. macOS 側は候補 2 の実装経路（bind mount + `ContainerAsync` 保持）、Linux 側は既存 `copy_to_sources_linux` にディスパッチ。候補 3 は候補 2 より先に実測するため、候補 3 の PoC で構築した経路（bind mount 注入・一時ファイル寿命管理）をそのまま候補 2 の実測に流用する
3. 統合テストは 0041 Linux 側と同型（`copy_to_visible_before_initial_process` / `copy_to_overwrite_visible_before_initial_process` の macOS 版、`with_copy_to_before_start` に置換）。既存パス上書きテスト（`copy_to_overwrite_...` 相当）は virtiofs 制約の再検証を兼ねるため、失敗しても候補 3 は成立し得る（新規パス・Data の可視性が満たせれば成立）。失敗の有無は `## macOS 実測結果` に記録する
4. 新 API は macOS で候補 2 と同じ意味論（書き込み反映抑止）を持つため、候補 3 の実測では候補 2 の PoC 不変条件リスト（mode 反映・書き込み反映抑止・一時ファイル cleanup・Drop 順序・`with_mount` 衝突検出）も検証する。候補 3 が成立した場合は候補 2 の実測が不要になるため、この検証が候補 3 自身の仕様根拠として `## macOS 実測結果` に残る

**候補 2 の判定（PoC 不変条件リスト）**

「既存テスト全 pass」ではなく、次の不変条件を全て満たす PoC を作れれば成立とする。target のパスは virtiofs の子だけ bind 不可制約と衝突しないよう選ぶ（例: `/data/payload.txt` で mount target は `/data`）。ただし「既存パス上書き」は制約の再検証を兼ねるため、成立可否を判定結果として記録する（制約が誤っていれば既存パス上書きも成立し得る）。

- **File ソース**: `.with_copy_to("/data/payload.txt", host_file_path)` が start 前に見え、初期プロセスから読める
- **Data ソース**: `.with_copy_to("/data/payload.txt", format!("{marker}\n").into_bytes())` が start 前に見え、初期プロセスから読める

「初期プロセスから読める」の確認は候補 1 と同じ `WaitFor::message_on_stdout(marker)` パターンを使う（候補 1 の判定 step 2 参照）。
- **既存パス上書き**: `/etc/motd` のような既存パス上書きが成立するか（`## 現状` の「子だけ bind 不可」制約の再検証を兼ねる）
- **mode 反映**: virtiofs 経由で `CopyTargetOptions.mode` が反映される（既存 `xpc_alpine_with_copy_to_target_options` の判定条件を満たす）。Data ソースの一時ファイルは mode `0o600` で作成されるため、virtiofs 経由ではホスト側ファイルの mode が見える前提で、PoC 側で対象ファイルの mode を `CopyTargetOptions.mode` に揃える chmod 手順を実測プロトコルに含める
- **一時ファイル cleanup**: `ContainerAsync` の Drop / 明示 `rm` 完了後にホスト側の `shiguredo_container_copy_*.bin` が残らない（Keep 時は残る）
- **Drop 順序**: コンテナが稼働中に一時ファイルが unlink されない（remove 完了後 unlink）
- **書き込み反映抑止**: `AccessMode::ReadOnly` 既定で内部 mount を注入した場合、コンテナ内でファイルへ書き込むと失敗する
- **`with_mount` との衝突検出**: 同一 target への二重登録が `build_config` 段階で明示エラーになる

### 判定後の運用

- **候補 4a / 1 / 3 / 2 のいずれか成立（分岐 A）**: 該当する実装 issue を `create-issue` スキル経由で起票し、その番号を本 issue の `## 調査結果` に記録する。本 issue はそこで closed にする
- **全案 NG（分岐 B）**: 候補 4b を Priority Low の別 issue として `create-issue` スキル経由で起票し、番号を `## 調査結果` に記録する。あわせて `docs/TESTCONTAINERS.md` / rustdoc / README の macOS 制約記述を「現状維持で運用する」旨に整える別の実装 issue（`feature/update-...`）を起票し、番号を記録する。本 issue はそこで closed にする

## 完了条件

- 4 候補（候補 4a / 1 / 3 / 2）全てについて実測結果が `## macOS 実測結果` に追記されている（各候補について「実施日、macOS バージョン、Apple container CLI / apiserver のバージョンと build、試した手順、成功/失敗、判定」。XPC エラー全文は 4a の失敗時・その他候補でエラーが発生した場合に追記）。候補 2 / 3 で ReadOnly 既定の根拠が確定した場合はその根拠も記録する。ただし上位案が成立した時点で以降の実測は不要（未実施として明示的に残す）
- 選定候補（分岐 A: 候補 4a / 1 / 3 / 2 のいずれか成立。分岐 B: 全案 NG）の判定が本文に記録されている
- 判定に応じた実装 issue（分岐 A の場合は成立した候補の実装 issue、分岐 B の場合は候補 4b + 文書化 issue）が `create-issue` スキル経由で起票され、番号が `## 調査結果` に記録されている
- 本 issue のブランチ（`feature/debug-macos-prestart-file-visibility`）で PoC 用の実測コードが変更されている場合、実測後に revert する（本 issue は調査 issue のためコード変更は成果物ではない。`feature/debug-` ブランチはマージしない。revert できない事情がある場合のみ、実装 issue の作業ブランチとは別の参照用ローカルブランチへ退避する）

## macOS 実測結果

（実測時に候補ごとに追記する。項目: 実施日、macOS バージョン、Apple container CLI / apiserver のバージョンと build（release / preview / self-built）、試した手順、成功/失敗、判定。XPC エラー全文は 4a の失敗時・その他候補でエラーが発生した場合に追記）

### 候補 4a: 不成立 (Apple container 1.2.0 でも状態ゲートは緩和されず)

- 実施日: 2026-08-01
- macOS: 26.4 (BuildVersion 25E246)
- Apple container: CLI 1.2.0 (build: release, commit: unspeci) / apiserver 1.2.0 (build: release, commit: unspeci)
- 試した手順:
  1. **シーケンス 1 (create 直後・bootstrap より前の `containerCopyIn`)**: CLI (`container create` → `container cp`) と、ライブラリの XPC 直接呼び出し PoC (`containerCreate` → `containerCopyIn`) の両方で確認した
  2. **シーケンス 2 (bootstrap 後・start_process 前の `containerCopyIn`)**: ライブラリの XPC 直接呼び出し PoC (`containerCreate` → `containerBootstrap` → `containerCopyIn`) で確認した (CLI では bootstrap 後の状態を作れないため)
- 結果: **両シーケンスとも失敗**
  - シーケンス 1: `XPC error invalidState: container probe-4a-<id> is not running`
  - シーケンス 2: `XPC error invalidState: container probe-4a-<id> is not running`
- 判定: **候補 4a 不成立**。1.2.0 でも `containerCopyIn` は `startProcess` 後 (running) 必須のままで、状態ゲートは緩和されていない。候補 4b の緩和条件 (1.2 系で bootstrap 後・start 前の copyIn 成功) も満たさないため、4b の判定材料にもならない。候補 1 へ進む

### 候補 1: 成立 (bind mount で起動前ファイル可視化が可能。既存パス子 bind 制約は誤り)

- 実施日: 2026-08-01
- macOS: 26.4 (BuildVersion 25E246)
- Apple container: CLI 1.2.0 (build: release, commit: unspeci) / apiserver 1.2.0 (build: release, commit: unspeci)
- 試した手順: `Mount::bind_mount` を使い、`WaitFor::message_on_stdout(marker)` パターンで「初期プロセスが起動時に bind mount 内容を読めた」ことを確認した (起動コマンドが `cat <path>` で marker を stdout に出す + ready 条件が成立すること)
  1. **新規パスへの単一ファイル bind** (`host_file` → `/data/payload.txt`): 成立
  2. **`/etc/mosquitto` 全体差し替え** (host dir → `/etc/mosquitto`): 成立
  3. **`/etc/mosquitto/mosquitto.conf` 直接 bind** (host file → `/etc/mosquitto/mosquitto.conf`): 成立 (ただし alpine には `/etc/mosquitto` が存在しないため、実質新規パス)
  4. **既存 non-empty ディレクトリ配下の既存ファイル bind** (host file → `/etc/motd`、`/etc` は既存): **成立**。`/etc/motd` がホストファイル内容に差し替わり、`/etc` はディレクトリのまま `/etc/passwd` も無傷 (etc-intact を確認)
- 判定: **候補 1 成立**。`## 現状` の「virtiofs は既存 non-empty ディレクトリ配下の子だけを bind して差し替える形式ができない (未検証の仮定)」は **誤り** であり、mosquitto 要件 (既存パス単一 File 差し替え) は「部分成立」→「成立」に変わる。`CopyDataSource::Data` (インメモリ bytes) の起動前投入のみ候補 1 では対象外 (利用側で tempfile が必要)。Data 相当の受容可否は Reporter 確認待ち (候補 1 判定 step 4)

## 調査結果

- **候補 4a**: 不成立。Apple container 1.2.0 でも `containerCopyIn` は `startProcess` 後 (running) 必須のまま (create 直後・bootstrap 後とも `is not running` で失敗)
- **候補 1: 選定**。`Mount::bind_mount` で起動前ファイル可視化が成立 (新規パス・ディレクトリ全体・既存パス配下の子ファイル bind のいずれも)。「virtiofs は既存 non-empty ディレクトリ配下の子だけを bind できない」という本 issue の仮定は誤りで、mosquitto 要件は「部分成立」→「成立」に変わった
- **起票した実装 issue**: **0057** (ドキュメント: macOS で起動前にファイルを見せる場合は `Mount::bind_mount` を使うことを明記する)

## 解決方法

4 候補のうち候補 4a を XPC 直接呼び出し PoC で実測し、Apple container 1.2.0 でも `containerCopyIn` は `startProcess` 後 (running) 必須のままで不成立と確認した (create 直後・bootstrap 後の両シーケンスで `XPC error invalidState: container ... is not running`)。候補 4b の緩和条件も満たさない。

候補 1 (bind mount) を実測し、新規パス・ディレクトリ全体・既存パス配下の子ファイル bind (`/etc/motd` 差し替え + `/etc/passwd` 無傷) のいずれも、`WaitFor::message_on_stdout(marker)` パターンで初期プロセスが起動時に bind mount 内容を読めることを確認した。「既存 non-empty ディレクトリ配下の子だけを bind できない」という仮定は誤りで、mosquitto 要件は成立に変わった。

Reporter が `CopyDataSource::Data` (インメモリ bytes) の起動前投入が候補 1 の対象外 (利用側で tempfile が必要) であることを受容したため、候補 1 を選定した。ドキュメント修正の実装 issue 0057 を `create-issue` 経由で起票し、本 issue は closed とした。PoC 用の実測コードは調査用の一時コードのため本 issue の成果物ではなく、`feature/debug-` ブランチはマージしない。
