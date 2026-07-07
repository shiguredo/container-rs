# 機能追加: Linux でプライベートレジストリ認証を対応する

- Priority: Medium
- Created: 2026-07-22
- Completed:
- Model: Cursor Grok 4.5
- Branch: feature/add-registry-auth
- Polished:

## 目的

Linux (Docker Engine API) でプライベートレジストリ (例: プライベートな `ghcr.io` パッケージ) からイメージを pull できるようにする。
本家 testcontainers-rs と同様に、Docker の既存認証設定を読んで pull に載せる。

## 優先度根拠

公開イメージだけなら不要だが、プライベートレジストリのイメージをテストで使う需要がある。
本家は標準対応している一方、本クレートは認証経路が無くプライベート pull ができない。
公開レジストリ利用のブロッカーではないため Medium。

## 現状

- `DockerClient::pull_image` (`src/core/client/docker_client.rs`) は
  `POST /images/create?fromImage=...&tag=...` を認証ヘッダ無しで送る
- `DOCKER_AUTH_CONFIG` / `DOCKER_CONFIG` / `~/.docker/config.json` を読む処理が無い
- `docker_credential` 等の依存も無い
- macOS (XPC `imagePull`) の認証は本 issue の対象外

## 設計方針

本家 testcontainers-rs に揃える。

1. 認証設定の読み込み優先順
   - `DOCKER_AUTH_CONFIG` (config.json 相当の JSON 文字列)
   - `DOCKER_CONFIG` ディレクトリ配下の `config.json`
   - それ以外は `~/.docker/config.json`
2. イメージ参照からレジストリホストを抽出する
   (先頭コンポーネントに `.` または `:` があればホスト、無ければ Docker Hub)
3. 取得した資格情報を Docker Engine API の pull に載せる
   (`X-Registry-Auth` 相当。username/password または identity token)
4. 依存は最小方針を守る。本家の `docker_credential` + bollard と同等の責務を、
   自前実装か最小依存のどちらで満たすかを実装時に判断する
5. 資格情報の実値をログや `Debug` に出さない

前提: レジストリ付き参照 (例: `ghcr.io/org/image:tag`) の path / query エンコードが
正しくないと inspect / pull が壊れる。未エンコードの修正は別バグ対応として扱い、
本 issue では混ぜない。

## 完了条件

- [ ] `DOCKER_AUTH_CONFIG` / `DOCKER_CONFIG` / 既定 `config.json` から資格情報を読める
- [ ] Linux の pull 時にレジストリ認証が付与される
- [ ] 認証が無い / 公開レジストリの既存挙動を壊さない
- [ ] 資格情報がログ・Debug に露出しない
- [ ] 単体テストで設定読取・レジストリ抽出・ヘッダ付与を検証する
  (実プライベートレジストリへの統合は環境依存のため必須としない)
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
