# バグ: macOS のコンテナ作成経路で環境変数の重複が解決されず `with_env_var` による上書きが効かない

- Created: 2026-08-04
- Completed: 2026-08-04
- Branch: feature/fix-macos-create-env-merge
- Polished: 2026-08-04

## 目的

macOS (Apple container) で `Image::env_vars` と `ImageExt::with_env_var` が同名キーを持つ場合に、リクエスト側の値で上書きされることを保証する。

## 現状

- `ContainerRequest::env_vars` は `Image::env_vars` とリクエスト側 `env_vars` の **chain 連結のみ**で、同名キーの重複を畳まない (`src/core/containers/request.rs` の `env_vars`)
- macOS の create 経路 (`src/core/client/container_cfg.rs` の `build_config`) は `env_vars()` の結果を `KEY=VALUE` の `Vec<String>` にそのまま畳むため、Image 側とリクエスト側で同名キーがあると `["FOO=image 側", "FOO=req 側"]` が重複したまま XPC に送られる
- 一方 exec 経路 (`src/core/containers/async_container.rs` の `exec`) は `ContainerRequest::env_vars` を BTreeMap に畳んで「リクエスト勝ち」を保証しており、**経路間で挙動が非対称**
- 裏付け (一次資料): Apple container の `Sources/Services/RuntimeLinux/Server/RuntimeService.swift` の `configureInitialProcess` / `configureProcessConfig` は環境変数の配列 (init プロセスでは `ContainerConfiguration.initProcess.environment`、exec では `ProcessConfiguration.environment`) をそのままランタイムへ渡し、重複解決しない (ssh 用 env の追加時のみ `contains` で重複回避している)。ソース: https://github.com/apple/container/blob/main/Sources/Services/RuntimeLinux/Server/RuntimeService.swift
- コンテナ内の C ライブラリ (`glibc` / `musl`) の `getenv` は envp の重複エントリの**先頭を返す**ため、chain 順 (Image 側 → リクエスト側) のまま送ると `with_env_var` による上書きが**効かない可能性が高い**
- 本バグが発現するのは `Image::env_vars` を既定値つきで実装した Image impl を利用する場合に限られる。`GenericImage` は既定 env を持たず、OCI イメージ config の ENV (Dockerfile の `ENV` 命令) は `build_config` に含まれないため、本修正の対象外

## 設計方針

- `build_config` の env 構築で BTreeMap に畳み、Image 側を基底 → リクエスト側で `insert` する (exec 経路と同じ「リクエスト勝ち」規則) ようにする
- `ContainerRequest::env_vars` の chain 実装は本家 testcontainers-rs 0.27.3 と同じ構造のため、公開アクセサのシグネチャは変更しない (送信側の畳み込みのみ修正する)
- 本 issue の対象は macOS (Apple container) のみとする。Linux (Docker) 経路 (`src/runners/async_runner.rs` の `ContainerConfig` 構築) も同じ chain を `Config.Env` に渡すため、同様の問題が起きる可能性は否定できないが、Docker Engine 側での Env の解釈は本 issue の検証範囲外とし、必要なら別途検証する
- 注意: exec 経路の env マージの macOS / Linux 分岐共通化を予定している 0096 (refactor) とは実装対象が異なるが、実装順序によっては干渉し得る

## 完了条件

- macOS で `Image::env_vars` と `with_env_var` が同名キーを持つ場合に、コンテナの init プロセス (create 経路) の環境でリクエスト側の値が観測されること (実機統合テスト。修正前は失敗し、修正後に成功すること。修正前から成功する場合はゲスト側で重複が解決されていることを示すため、原因を調査して対応を判断する)
- exec 経路と create 経路の env 解決規則が同一になること (どちらも `Image::env_vars` とリクエスト側 env の chain を BTreeMap に畳み、後から来た値が勝つ: create 経路はリクエスト側 `env_vars`、exec 経路は `ExecCommand` の env)

## 解決方法

- `src/core/client/container_cfg.rs` の `build_config` で env 構築を BTreeMap に畳む実装に変更した。`ContainerRequest::env_vars()` が返す Image 側 env → リクエスト側 env の chain を `BTreeMap<String, String>` に collect することで、同名キーは後から来た値 (リクエスト側) が勝つ。`KEY=VALUE` 形式の `Vec<String>` は BTreeMap から変換する
- 単体テスト `env_vars_are_folded_with_request_winning_in_init_env` を追加した。既定 env を返すテスト専用 Image impl `DefaultEnvImage` (`FOO` / `IMAGE_ONLY`) を使い、`with_env_var` で `FOO` を上書きしたとき `init_env` が `["FOO=from_request", "IMAGE_ONLY=from_image_only", "REQ_ONLY=from_request_only"]` に畳まれることを検証する (衝突キーの後勝ち・Image 側のみのキーの素通し・リクエスト側のみのキーの素通し)
- 統合テスト `alpine_create_env_request_wins_over_image_default` を `tests/container_macos.rs` に追加した。既定 env を持つテスト専用 Image impl を使い、`printenv FOO IMAGE_ONLY` を init プロセスにしてコンテナ stdout を検証する。`WaitFor::message_on_stdout` を ready 条件に指定して出力到着を決定的に待つ。実機で修正前は失敗 (FOO が Image 側の値のまま)・修正後は成功を確認した
