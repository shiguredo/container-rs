# 機能追加: with_copy_to の親ディレクトリ自動作成とディレクトリ投入に対応する

- Priority: Low
- Created: 2026-07-23
- Completed: {YYYY-MM-DD}
- Model: Claude Fable 5
- Branch: feature/add-copy-to-parents-and-directory
- Polished: {YYYY-MM-DD}
- Reporter: @voluntas

## 目的

`with_copy_to` の投入能力を testcontainers-rs 相当に近づける。具体的には次の 2 点。

1. コピー先の親ディレクトリが存在しない場合に自動作成する (macOS の `createParents` 相当を Linux でも)
2. ディレクトリ (複数ファイル) の一括投入

TLS 証明書一式や設定ファイル群の投入で効く。利用者フィードバック (mqtt-rs の移行) では既存パスへの単一ファイル投入で足りたため必須ではないが、testcontainers-rs 互換の DX としては穴になっている。

## 優先度根拠

利用者フィードバック由来だが「あればよい」水準で、現状の単一ファイル投入で回避できているため Low。

## 現状

- Linux は issue `0013` (closed) の割り切りで **単一 regular file のみ・親ディレクトリ自動作成なし**。`copy_to_sources_linux` (`src/runners/async_runner.rs:768-`) が `target.path` を dirname / basename に分割し、basename 1 エントリの ustar を `PUT /containers/{id}/archive?path=<dirname>` で送る。dirname が存在しないと Docker Engine がエラーを返す。`CopyDataSource::File` がディレクトリの場合は `PathNameError` で拒否する (`src/runners/async_runner.rs:807-812`)
- 制約は `docs/TESTCONTAINERS.md` に注記済み
- macOS は XPC `containerCopyIn` に `createParents: true` を渡しており (`src/core/client/xpc_client.rs:125,153`)、親ディレクトリは自動作成される。`CopyDataSource::File` にディレクトリを指定した場合の挙動は未確認 (実装時に実測する)
- tar 構築 (`src/core/client/docker_tar.rs`) は typeflag `\0` / `0` (regular file) の書き込みと typeflag `5` (directory) の読み取り検出のみ対応。directory エントリの書き込みは未対応

## 設計方針

- Linux の親ディレクトリ自動作成: tar 内に typeflag `5` の directory エントリを先行させ、ファイルをルートからの相対パスで格納して既存の祖先ディレクトリ (最終的には `/`) に対して `PUT /archive` する方式を第一候補とする。ustar の `name[100]` 制限を超えるパスは `prefix[155]` の利用も含めて実装時に検討する
- Linux のディレクトリ投入: `CopyDataSource::File` がディレクトリの場合に再帰的に walk して複数エントリの tar を構築する。symlink・特殊ファイルの扱い (拒否 or スキップ) は実装時に決めて文書化する
- macOS: ディレクトリ指定時の `containerCopyIn` の挙動を実測し、動くならバリデーションを緩めるだけで済ませる。動かない場合の対応範囲は実測結果を見て決める
- `CopyTargetOptions` (mode / uid / gid) のディレクトリへの適用規則を定義して両 OS で揃える
- 公開 API のシグネチャは変えない (`with_copy_to` の受け口はそのまま、拒否していた入力を受理する方向のみ)

## 完了条件

- Linux で存在しない親ディレクトリ配下への単一ファイル投入が成功する統合テストが pass する
- Linux でディレクトリ (ネスト含む複数ファイル) の投入が成功し、コンテナ内で全ファイルの内容と mode / uid / gid が確認できる統合テストが pass する
- macOS のディレクトリ投入の実測結果に応じて、対応または制約の文書化が済んでいる
- `docs/TESTCONTAINERS.md` の Linux 制約の注記 (親ディレクトリ自動作成なし・単一 regular file のみ) が更新されている
- `CHANGES.md` に `[ADD]` エントリがある
- `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass する

## 解決方法

`src/core/client/docker_tar.rs` に directory エントリ (typeflag `5`) と複数エントリの書き込みを追加し、`copy_to_sources_linux` の dirname / basename 分割を「既存祖先への相対パス格納」に置き換える。ディレクトリソースは walk して tar に畳む。macOS は実測のうえバリデーション緩和または文書化を行う。
