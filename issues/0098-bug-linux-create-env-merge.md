# バグ: Linux の create 経路で環境変数の重複が解決されず `with_env_var` による上書きが効かない可能性がある

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-linux-create-env-merge
- Polished: {YYYY-MM-DD}

## 目的

Linux (Docker) で `Image::env_vars` と `ImageExt::with_env_var` が同名キーを持つ場合に、リクエスト側の値で上書きされることを保証する。

## 現状

- `src/runners/async_runner.rs` の `build_container_config` は `ContainerRequest::env_vars()` (Image 側 → リクエスト側の chain) を `KEY=VALUE` の `Vec<String>` に畳まずに `Config.Env` へ渡している
- macOS の create 経路 (`src/core/client/container_cfg.rs` の `build_config`) は同名キーを BTreeMap に畳んで「リクエスト勝ち」にする修正が済んでいるが、Linux 経路は未対応のため経路間で挙動が非対称
- Docker Engine / runc が `Env` の重複エントリを解決するかは未検証。解決しない場合、コンテナ内の `getenv` は envp の先頭 (Image 側) を返し、`with_env_var` による上書きが効かない

## 設計方針

- macOS と同一の方針とする: `ContainerRequest::env_vars()` の chain を BTreeMap に畳んでから `Config.Env` に渡す
- Docker Engine 側で重複が解決される実挙動が確認できた場合は、その検証結果を記録して対応を判断する
- 公開アクセサのシグネチャは変更しない (macOS 修正と同じ方針)

## 完了条件

- Linux で `Image::env_vars` と `with_env_var` が同名キーを持つ場合に、コンテナの init プロセス (create 経路) の環境でリクエスト側の値が観測されること (実機統合テスト。修正前は失敗し、修正後に成功すること。修正前から成功する場合はゲスト側で重複が解決されていることを示すため、原因を調査して対応を判断する)
- macOS と Linux の create 経路の env 解決規則が同一になること (どちらも BTreeMap に畳み、後から来た値が勝つ)

## 解決方法

- `src/runners/async_runner.rs` の `build_container_config` で env を BTreeMap に畳んでから `KEY=VALUE` に変換する
- `src/runners/async_runner.rs` の `#[cfg(test)]` モジュールに、畳み込み結果が `ContainerConfig.env` に反映され、同名キーが 1 件に畳まれてリクエスト側の値が残ることを確認する単体テストを追加する (既定 env を返すテスト専用 Image impl が必要)
- Linux の統合テスト (`tests/container_linux.rs`) に、既定 env を返すテスト専用 Image impl を用意し、「Image 既定 env と `with_env_var` が同名キーを持つ場合にコンテナの init プロセスでリクエスト側が勝つ」ことを検証するテストを追加する
