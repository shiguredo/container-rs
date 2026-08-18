# 機能追加: Linux でプライベートレジストリ認証を対応する

- Priority: Medium
- Created: 2026-07-22
- Completed: 2026-08-01
- Model: Cursor Grok 4.5
- Branch: feature/add-registry-auth
- Polished: 2026-07-29

## 目的

Linux (Docker Engine API) でプライベートレジストリ (例: プライベートな `ghcr.io` パッケージ) からイメージを pull できるようにする。
本家 testcontainers-rs 0.27.3 と同様に、Docker の既存認証設定を読んで pull に載せる。

## 現状

- `DockerClient::pull_image` (`src/core/client/docker_client.rs`) は
  `POST /images/create?fromImage=...&tag=...` を認証ヘッダ無しで送る
- `DOCKER_AUTH_CONFIG` / `DOCKER_CONFIG` / `~/.docker/config.json` を読む処理が無い
- `docker_credential` 等の依存も無い。ただし `base64ct` は optional 依存として既に存在する (`http_wait_plain` feature 用)
- macOS (XPC `imagePull`) の認証は本 issue の対象外
- macOS 側の `normalize_image_reference` (`src/core/client/xpc_client.rs`) はレジストリホスト判定に `first.contains('.') || first.contains(':') || first == "localhost"` の 3 条件を使用している。Linux 側の実装でもこの判定と整合する必要がある
- `encode_docker_api_request` (`src/core/client/docker_client.rs`) は固定ヘッダ (`Host`, `Connection`, 任意で `Content-Type`/`Content-Length`) のみ構築し、任意ヘッダを追加する手段がない。`X-Registry-Auth` を付与するにはシグネチャ変更または `pull_image` 専用のリクエスト構築が必要

## 設計方針

本家 testcontainers-rs 0.27.3 に揃える。

1. 認証設定の読み込み優先順
   - `DOCKER_AUTH_CONFIG` (config.json 相当の JSON 文字列)
   - `DOCKER_CONFIG` ディレクトリ配下の `config.json`
   - それ以外は `~/.docker/config.json`
   - config.json の `auths` の静的エントリのみを読む。`credHelpers` / `credsStore` の credential helper 呼び出し（プロセス起動を伴う）は対象外とする
   - config.json の `auth` フィールドは base64 デコードして `username:password` に分割する
2. イメージ参照からレジストリホストを抽出する
   (先頭コンポーネントに `.` または `:` があるか、`localhost` であればホスト、無ければ Docker Hub。macOS 側の `normalize_image_reference` と同一の判定規則。Docker Hub の `auths` 参照キーは `https://index.docker.io/v1/`)
3. 取得した資格情報を Docker Engine API の pull に載せる
   (`X-Registry-Auth` ヘッダに base64 エンコードした JSON (`username`/`password`/`serveraddress`/`identitytoken`) を設定する)
4. 依存は最小方針を守る。本家の `docker_credential` + bollard と同等の責務を、
   自前実装か最小依存のどちらで満たすかを実装時に判断する。`base64ct` は既存依存として再利用可能だが、現在は `http_wait_plain` feature の optional 依存であるため、non-optional への昇格またはレジストリ認証用 feature の新設が必要
5. 資格情報の実値をログや `Debug` に出さない

## 完了条件

- [ ] `DOCKER_AUTH_CONFIG` / `DOCKER_CONFIG` / 既定 `config.json` から資格情報を読める
- [ ] Linux の pull 時にレジストリ認証が付与される
- [ ] 認証が無い / 公開レジストリの既存挙動を壊さない
- [ ] 資格情報がログ・Debug に露出しない
- [ ] 単体テストで設定読取・レジストリ抽出・ヘッダ付与を検証する
  (実プライベートレジストリへの統合は環境依存のため必須としない。モック・スタブは使用しない)
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の関連箇所が実装済みに更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

- `src/core/client/registry_auth.rs` を新規作成し、Docker 認証設定の読み込み・レジストリ判定・X-Registry-Auth ヘッダ構築を実装した
- `DockerClient::pull_image` に認証ヘッダ付与を追加した
- `request_with_extra_headers` / `encode_docker_api_request_with_headers` を追加した
- `base64ct` を non-optional 依存に昇格した
- 単体テストで auths キー抽出・base64 デコード・ヘッダ構築を検証した
- CHANGES.md に [ADD] エントリを追加した
