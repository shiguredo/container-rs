# Apple container 1.3.0 のリリースに追従する

- Created: 2026-08-25
- Completed: {YYYY-MM-DD}
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

- 1.3.0 起因のコード修正が不要であることが確認できること。
- Homebrew が 1.3.0 を配布した時点で test-apple-container が通過するか、ソースビルドによるローカル検証が記録されていること。
- ドキュメント (README.md / docs/TESTCONTAINERS.md / skills/shiguredo-container/SKILL.md) と CHANGES.md が 1.3.0 追従の実態を反映していること。

## 解決方法

- 1.3.0 で動作検証し、問題が無ければ以下を更新する:
  - README.md の要件表: 検証済みバージョンの記録を追加する (最小要件 1.2.0 は維持)
  - docs/TESTCONTAINERS.md と skills/shiguredo-container/SKILL.md: 同等の注記
  - CHANGES.md: UPDATE として追従内容を記載する
- 1.3.0 で問題を検出した場合、その issue を別途起票して対応する。
