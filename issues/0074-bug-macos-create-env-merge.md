# バグ: macOS のコンテナ作成経路で環境変数の重複が解決されず `with_env_var` による上書きが効かない

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-create-env-merge
- Polished: {YYYY-MM-DD}

## 目的

macOS (Apple container) で `Image::env_vars` と `ImageExt::with_env_var` が同名キーを持つ場合に、リクエスト側の値で上書きされることを保証する。

## 現状

- `ContainerRequest::env_vars` は `Image::env_vars` とリクエスト側 `env_vars` の **chain 連結のみ**で、同名キーの重複を畳まない (`src/core/containers/request.rs` の `env_vars`)
- macOS の create 経路 (`src/core/client/container_cfg.rs` の `build_config`) は `env_vars()` の結果を `KEY=VALUE` の `Vec<String>` にそのまま畳むため、Image 側とリクエスト側で同名キーがあると `["FOO=image 側", "FOO=req 側"]` が重複したまま XPC に送られる
- 一方 exec 経路 (`src/core/containers/async_container.rs` の `exec`) は BTreeMap に畳んで「リクエスト勝ち」を保証しており、**経路間で挙動が非対称**
- 裏付け (一次資料): Apple container の `Sources/Services/RuntimeLinux/Server/RuntimeService.swift` は `ProcessConfiguration.environment` を **配列のままランタイムへ渡し、重複解決しない** (ssh 用 env の追加時のみ `contains` で重複回避している)。Linux の `execve` セマンティクスでは envp の重複エントリは**先頭が優先**されるため、chain 順 (Image 側 → リクエスト側) のまま送ると `with_env_var` による上書きが**効かない可能性が高い**
- 実イメージ (postgres 等) が既定 env を定義している場合、`with_env_var("PGPORT", ...)` のような一般的な利用が macOS で反映されない

## 設計方針

- `build_config` の env 構築で BTreeMap に畳み、Image 側を基底 → リクエスト側で `insert` する (exec 経路と同じ「リクエスト勝ち」規則) ようにする
- `ContainerRequest::env_vars` の chain 実装は本家 testcontainers-rs 0.27.3 と同じ構造のため、公開アクセサのシグネチャは変更しない (送信側の畳み込みのみ修正する)

## 完了条件

- macOS で `Image::env_vars` と `with_env_var` が同名キーを持つ場合に、コンテナ内でリクエスト側の値が観測されること (実機統合テスト)
- exec 経路と create 経路の env 解決規則が同一になること

## 解決方法

- `src/core/client/container_cfg.rs` の `build_config` で env を BTreeMap に畳んでから `KEY=VALUE` に変換する
- macOS の統合テスト (`tests/container_macos.rs`) に「Image 既定 env と `with_env_var` が同名キーを持つ場合にリクエスト側が勝つ」テストを追加する (例: 既定 env を持つ軽量イメージで `sh -c 'echo $VAR'` を exec して検証)
