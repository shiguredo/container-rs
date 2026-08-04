# バグ: レジストリ認証の Docker Hub 正規化漏れでプライベート Hub イメージの pull が匿名化される

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-registry-auth-dockerhub-normalization
- Polished: {YYYY-MM-DD}

## 目的

`docker.io/...` / `index.docker.io/...` 形式で参照されたプライベート Docker Hub イメージの pull が、認証ヘッダなしで実行されて失敗する問題を修正する。

## 現状

- `src/core/client/registry_auth.rs` の `auths_key` は、`/` を含む参照の先頭コンポーネントが `.` / `:` を含むか `localhost` の場合に**そのまま**をキーとして返す
  - `auths_key("docker.io/org/private")` → `"docker.io"`
  - `auths_key("index.docker.io/org/private")` → `"index.docker.io"`
- 一方 `docker login` が `~/.docker/config.json` に書き込む Docker Hub のキーは `https://index.docker.io/v1/` (docker CLI は `docker.io` / `index.docker.io` をこのキーへ正規化して照合する)
- 結果、キー不一致で `x_registry_auth` が `None` になり、プライベート Hub イメージを `docker.io/...` 形式で参照すると `X-Registry-Auth` ヘッダなしで pull され、daemon から `denied` で失敗する (`ghcr.io` 等のプライベートレジストリは正規化の影響を受けない)
- なお `auths_key("alpine:latest")` 等の `/` なし参照は `https://index.docker.io/v1/` を返すため正しい

## 設計方針

- `auths_key` で `docker.io` / `index.docker.io` を `https://index.docker.io/v1/` に正規化してから照合する (docker CLI と同じ規則)
- `extract_auth_entry` の `serveraddress` 判定 (`key == "https://index.docker.io/v1/"`) はそのまま使える

## 完了条件

- `auths_key("docker.io/org/img")` と `auths_key("index.docker.io/org/img")` が `https://index.docker.io/v1/` を返すこと (単体テスト)
- `x_registry_auth` が `docker.io/org/private` 形式でも config.json の Docker Hub エントリを拾えること (単体テスト)

## 解決方法

- `src/core/client/registry_auth.rs` の `auths_key` で、先頭コンポーネントが `docker.io` または `index.docker.io` の場合に `https://index.docker.io/v1/` を返す分岐を追加する
- `auths_key` の単体テストに `docker.io/org/img` / `index.docker.io/org/img` のケースを追加する
