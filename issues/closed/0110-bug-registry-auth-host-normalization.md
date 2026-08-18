# バグ: レジストリ認証のホスト名比較が大文字小文字を区別し Docker Hub 参照を正規化できない

- Created: 2026-08-12
- Completed: 2026-08-12
- Branch: feature/fix-registry-auth-host-normalization
- Polished: {YYYY-MM-DD}

## 目的

イメージ参照の先頭コンポーネントが `Docker.IO` のような大文字小文字混在の場合でも、Docker Hub の認証情報 (`index.docker.io` キー) を正しく解決できるようにする。

## 現状

`src/core/client/registry_auth.rs` の Docker Hub 判定は先頭コンポーネントを文字列比較している。

```rust
if first == "docker.io" || first == "index.docker.io" {
```

- `Docker.IO/org/img` のような大文字小文字混在の参照は正規化されず、認証情報のキー (`auths` のキー) と不一致になる
- docker CLI はホスト名を小文字化して照合する (DNS ホスト名は大文字小文字を区別しない) ため、`docker login Docker.IO` で保存した config との照合が失敗し得る
- 同クレート内の `normalize_image_reference` (xpc_client.rs) は `docker.io` 正規化を持つが、この判定は大文字小文字を区別する

## 設計方針

- ホスト名相当の比較を小文字化して行う (先頭コンポーネントの判定・正規化・auths キーの照合すべて)
- 大文字小文字の正規化がどこで行われるべきか (参照の正規化時 or 照合時) を実装時に確認して統一する

## 完了条件

- `Docker.IO/org/img` 形式の参照で Docker Hub の認証情報が解決されること
- 既存のレジストリ認証テストが従来どおり通ること

## 解決方法

対応不要のため closed にする (polish-issue の必要性判断 + 反対尋問で「不要」判定)。

- 一次資料 (docker/cli の `login.go` / `file_store.go` / `file.go`、moby の `auth.go` / `config.go`) で、docker CLI の認証照合に小文字化が存在しないことを確認した。`docker login Docker.IO` はキーを入力どおり `"Docker.IO"` で保存する
- `auths_key("Docker.IO/org/img")` は `"Docker.IO"` を返し、docker login Docker.IO の保存キーと既に一致するため、issue が主張する失敗シナリオは発生しない
- 実測でも `docker pull Docker.IO/library/alpine:latest` は認証以前に daemon レベルで失敗し、docker 自体が大文字小文字混在参照を解決できない
- 小文字化の提案は 0076 の「docker CLI と同じ規則に合わせるため、非正規化キーへのフォールバックは行わない」方針に反する
