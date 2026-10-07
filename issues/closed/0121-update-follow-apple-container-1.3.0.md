# Apple container 1.3.0 のリリースに追従する

- Created: 2026-08-25
- Completed: 2026-08-25
- Branch: feature/update-follow-apple-container-1.3.0
- Polished: {YYYY-MM-DD}

## 目的

Apple container 1.3.0 (2026-08-20 タグ) がリリースされた。1.2.2..1.3.0 の変更が本クレートの macOS (XPC) 経路に影響しないことを確認・記録し、ドキュメントを実態に合わせる。

## 現状

- README.md の要件表は「Apple container 1.2.0 以上必須」で、macOS 26 (Apple Silicon) を要件としている。
- CI の test-apple-container ジョブは `brew upgrade container` により最新版を検証する。Homebrew は現時点で 1.2.2 のため、1.3.0 はまだ CI で検証されていない。
- 本クレートは Apple container の apiserver を XPC で直接呼び出しており、CLI (`container run` / `container pull` のフラグ構文) には依存していない。

## 設計方針

1.2.2..1.3.0 の 13 commits (52 ファイル) を調査した結果、本クレートが利用する XPC ルートと JSON スキーマの変更は確認できなかった。影響を受けるのは以下に限られ、いずれも本クレートの経路とは無関係である。

- レジストリ scheme: `RequestScheme` の `auto` が廃止され `.https` が既定になった。これは Swift の ClientAPI (CLI) の話であり、XPC の `imagePull` ルートの引数 (`imageReference` / `insecureFlag` / `maxConcurrentDownloads`) は不変。本クレートは `insecureFlag: false` 固定で送るため挙動は変わらない。
- tmpfs: CLI の `tmpfsMounts()` が `dest:opts` 形式のパースと正規化比較による dedupe に変更。CLI 側の実装であり、本クレートは XPC の Filesystem JSON を直接送るため無関係。
- ボリューム: `VolumesService.volumeDiskUsage` がボリューム名の検証を行うようになった。本クレートは `volumeDiskUsage` ルートを呼び出さない。
- disk usage 系ルートの ID 検証: テストの追加のみで、サーバー側の enforce 自体は 1.2.0 から存在する。本クレートはコンテナ ID を start 時に `is_valid_container_id` で一括検証済み。
- その他: 既定カーネルの更新 (Kata 3.32.0)、containerization 0.40.1 → 0.41.0 (ImageStore 内部)、K8s 周りのリファクタ、ドキュメント再編。

したがってコード修正は不要と判断する。最小要件の 1.2.0 は 1.3.0 で廃止された機能が無いため維持する。

## 完了条件

- 1.2.2..1.3.0 の差分を調査し、1.3.0 起因のコード修正が不要かどうかが確認できること。
- 調査結果がドキュメントの実態と整合し、追記が必要な修正が特定されていること (未検出の場合はその旨)。

## 解決方法

1.2.2..1.3.0 の差分を精査し、本クレートが利用する XPC ルート (containerCreate / containerBootstrap / containerStartProcess / containerCopyIn / containerLogs / containerWait / imagePull / volumeCreate など) と JSON スキーマ (ContainerCfg / Filesystem / ContainerJSON) に変更が無いことを確認した。

- コード修正は不要。最低要件と README.md / docs/TESTCONTAINERS.md / skills/shiguredo-container/SKILL.md の注記は 1.3.0 でも正しいため変更しない。
- 動作検証は Homebrew が 1.3.0 を配布した時点から CI の test-apple-container (`brew upgrade container`) が自動的に行う。
