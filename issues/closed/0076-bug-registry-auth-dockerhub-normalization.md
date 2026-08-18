# バグ: レジストリ認証の Docker Hub 正規化漏れでプライベート Hub イメージの pull が認証なしで実行される

- Created: 2026-08-04
- Completed: 2026-08-05
- Branch: feature/fix-registry-auth-dockerhub-normalization
- Polished: 2026-08-04

## 目的

Linux (Docker Engine API) 経路で、`docker.io/...` / `index.docker.io/...` 形式で参照されたプライベート Docker Hub イメージの pull が、認証ヘッダなしで実行されて失敗する問題を修正する。macOS (XPC) 経路の `imagePull` は認証を渡す仕組み自体が無いため対象外 (認証未対応のままとする)。

## 現状

- `src/core/client/registry_auth.rs` の `auths_key` は、`/` を含む参照の先頭コンポーネントが `.` / `:` を含むか `localhost` の場合に**そのまま**をキーとして返す
  - `auths_key("docker.io/org/private")` → `"docker.io"`
  - `auths_key("index.docker.io/org/private")` → `"index.docker.io"`
- 一方 `docker login` が `~/.docker/config.json` に書き込む Docker Hub のキーは `https://index.docker.io/v1/` であり、docker CLI は `docker.io` / `index.docker.io` をこのキーへ正規化して照合する (`DOCKER_AUTH_CONFIG` で渡す場合も同じキー形式)
- 結果、キー不一致で `x_registry_auth` が `None` になり、プライベート Hub イメージを `docker.io/...` 形式で参照すると `X-Registry-Auth` ヘッダなしで pull され、daemon から `denied` で失敗する (Docker Hub 以外のプライベートレジストリはキーがそのまま一致するため正規化の影響を受けない)
- なお `auths_key("alpine:latest")` 等の `/` なし参照は `https://index.docker.io/v1/` を返すため正しい

## 設計方針

- `auths_key` で `docker.io` / `index.docker.io` を `https://index.docker.io/v1/` に正規化してから照合する (docker CLI と同じ規則。docker/cli の `getAuthConfigKey` が `docker.io` / `index.docker.io` を `authConfigKey` に正規化するのと同じ挙動)
- ホスト判定の 3 条件 (`first.contains('.') || first.contains(':') || first == "localhost"`) は維持し、`docker.io` / `index.docker.io` のみを Docker Hub キーへ正規化する (3 条件の判定規則は macOS 側の `normalize_image_reference` と同一)
- 修正後は `auths_key` が `docker.io` / `index.docker.io` を一切返さなくなるため、`auths` に `"docker.io"` キーで保存された config.json (podman / skopeo 等が書き込む形式) は拾えなくなる。docker CLI と同じ規則に合わせるため、非正規化キーへのフォールバックは行わない (この挙動変化は受け入れる)
- `extract_auth_entry` の `serveraddress` 判定 (`key == "https://index.docker.io/v1/"`) はそのまま使える

## 完了条件

- `auths_key("docker.io/org/img")` と `auths_key("index.docker.io/org/img")` が `https://index.docker.io/v1/` を返すこと (単体テスト)
- `x_registry_auth` が `docker.io/org/private` 形式でも `DOCKER_AUTH_CONFIG` に設定した Docker Hub エントリを拾えること (単体テスト)
- 既存の `auths_key` テスト (`auths_key_docker_hub_for_plain_image` / `auths_key_private_registry`) が引き続き通ること

## 解決方法

- `src/core/client/registry_auth.rs` の `auths_key` に、先頭コンポーネントが `docker.io` / `index.docker.io` の場合に `DOCKER_HUB_AUTH_KEY` (`https://index.docker.io/v1/`) を返す分岐を追加した。docker CLI の `getAuthConfigKey` と同じくこの 2 ドメインのみを正規化し、`registry-1.docker.io` やポート付きホストは正規化しない
- リテラル重複を避けるため `DOCKER_HUB_AUTH_KEY` 定数を導入し、`extract_auth_entry` の `serveraddress` 恒真分岐を `key` の直接使用に置き換えた
- `auths_key` のテストに `docker.io/org/img` / `index.docker.io/org/img` / `registry-1.docker.io` / ポート付きの正規化境界ケースを追加した
- `x_registry_auth` の検証は `wait` モジュールの `run_env_case` と同じ子プロセス分離方式で実装した (親テストが `DOCKER_AUTH_CONFIG` を子テストへ渡して実行。`std::env::set_var` によるプロセス共有環境変数の書き換えは並列テスト下でデータ競合になるため使わない)
- 注意: `src/core/client/registry_auth.rs` の `escape_json_value` を修正する 0086 (bug) も同一ファイルを対象とするため、実装順序によっては干渉し得る
- 完了条件は単体テストのみとし、実レジストリへの pull 検証は環境依存のため対象外とする
