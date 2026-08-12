# バグ: Linux の create 経路で環境変数の重複が解決されず `with_env_var` による上書きが効かない可能性がある

- Created: 2026-08-04
- Completed: 2026-08-12
- Branch: feature/fix-linux-create-env-merge
- Polished: 2026-08-12

## 目的

Linux (Docker) で `Image::env_vars` と `ImageExt::with_env_var` が同名キーを持つ場合に、リクエスト側の値で上書きされることを保証する。

## 現状

- `src/runners/async_runner.rs` の `build_container_config` は `ContainerRequest::env_vars()` (Image 側 → リクエスト側の chain) を KEY=VALUE の `Vec<String>` に変換したまま、同名キーの重複を畳まずに `Config.Env` へ渡している
- macOS の create 経路 (`src/core/client/container_cfg.rs` の `build_config`) は同名キーを BTreeMap に畳んで「リクエスト勝ち」にする修正が済んでいるが、Linux 経路は未対応のため経路間で挙動が非対称
- Docker Engine / runc が `Env` の重複エントリを解決するかは未検証。解決しない場合、コンテナ内の `getenv` は envp の先頭 (Image 側) を返し、`with_env_var` による上書きが効かない
- 本バグが発現するのは `Image::env_vars` を既定値つきで実装した Image impl を利用する場合に限られる。`GenericImage` は既定 env を持たず、`build_container_config` が `Config.Env` に渡すのは `ContainerRequest::env_vars()` の出力のみで、OCI イメージ config の ENV (Dockerfile の `ENV` 命令) は送信対象に含まれない (Linux では `Config.Env` が非空の場合 Docker Engine はイメージ config の ENV を適用しないため、GenericImage では重複渡しは発生しない)

## 設計方針

- macOS 修正 (closed 0074) と同じ実装方針とする: `ContainerRequest::env_vars()` の chain を BTreeMap に畳み、同名キーは後から来た値 (リクエスト側) が勝つようにしてから `Config.Env` に渡す
- 検証分岐は完了条件 1 に一元化する (Docker Engine 側で重複が解決される実挙動が確認できた場合は、観測結果を本 issue の解決方法に記録したうえで、macOS との規則統一を目的とする BTreeMap 畳み込みを適用する)

## 完了条件

- Linux で `Image::env_vars` と `with_env_var` が同名キーを持つ場合に、コンテナの init プロセス (create 経路) の環境でリクエスト側の値が観測されること
  - 実機統合テストで検証する。統合テストを先に書き、修正前の実行結果を確認してから修正する (Linux 統合テストは CI (test-linux-docker) で常時実行されるため、修正前の失敗確認はローカル Linux 環境 (Linux マシン / VM 等) で `cargo test --all-features` を実行して行い、失敗テストのまま push しない。`RUN_CONTAINER_TESTS` は macOS 専用ゲートであり Linux テストには効かない)
  - 修正前から成功した場合は、観測結果と調査結果を本 issue の解決方法に記録する
- ライブラリが `Config.Env` に送信する env の畳み込み規則が macOS と Linux で同一になること (どちらも BTreeMap に畳み、後から来た値が勝つ。検証は `mod linux_tests` の単体テストで行い、期待値は macOS 側の `build_config` の単体テストと同値になること)
- `CHANGES.md` の develop に `[FIX]` エントリが追加されていること

## 解決方法

- 統合テストを先に書き、修正前の実行結果 (失敗または Docker Engine 側での解決) を確認してから、以下の実装・テストを追加する (実装手順の順に記載)
- Linux の統合テスト (`tests/container_linux.rs`) に、既定 env を返すテスト専用 Image impl を用意し、「Image 既定 env と `with_env_var` が同名キーを持つ場合にコンテナの init プロセスでリクエスト側が勝つ」ことを検証するテストを追加する。構成は macOS 側の `alpine_create_env_request_wins_over_image_default` (`tests/container_macos.rs`) と同様とし、init プロセスの環境を直接観測する (シェル経由の展開 (`sh -c 'echo $VAR'` 等) はシェルが重複を後勝ちで解決し得るため検証にならない。`with_env_vars` を指定する exec 経由でも exec 側の畳み込みにより修正前からリクエスト勝ちが成立してしまうため、create 経路の検証としては使えない)
- `src/runners/async_runner.rs` の `build_container_config` で env を BTreeMap に畳んでから `KEY=VALUE` に変換する
- `src/runners/async_runner.rs` の `mod linux_tests` (`#[cfg(all(test, target_os = "linux"))]` の既存テストモジュール) に、畳み込み結果が `ContainerConfig.env` に反映され、同名キーが 1 件に畳まれてリクエスト側の値が残ることを確認する単体テストを追加する (`build_container_config` は `#[cfg(target_os = "linux")]` のため macOS 側の `mod tests` には置けない。モジュール doc コメントも更新する)
- テスト専用 Image impl は、既存の `tests/container_macos.rs` / `container_cfg.rs` の同名フィクスチャと内容を揃えること (統合テスト 2 ファイル + 単体テスト 2 モジュールの 4 箇所で複製されるが、0095 のスコープ外のため本 issue では内容を揃えて配置する)
- `CHANGES.md` に `[FIX]` エントリ (macOS 側エントリと対になる文言) を追加する
- 注記: exec 経路の env マージの macOS / Linux 分岐共通化を予定している 0096 (refactor) とは実装対象が異なるが、実装順序によっては干渉し得る (0074 と同じ注記)

## 解決方法 (実装)

- 統合テスト (`alpine_create_env_request_wins_over_image_default`) を先に追加し、修正前 (重複 Env のまま送信) の実行結果を確認した。**修正前から成功した** (Docker Engine 側が Env の重複を後勝ちで解決する実挙動を観測)。設計方針の検証分岐どおり、観測結果を記録したうえで macOS との規則統一を目的とする BTreeMap 畳み込みを適用した
- `build_container_config` で env を BTreeMap に畳んでから `KEY=VALUE` に変換するようにした (macOS の `build_config` と同一実装。chain 順: Image 側 → リクエスト側、後勝ち)
- `mod linux_tests` に単体テスト (`env_vars_are_folded_with_request_winning_in_config_env`) と `DefaultEnvImage` フィクスチャを追加した。期待値は macOS 側の単体テストと同値。修正前コードでは重複 4 要素になるため失敗し、検証力がある
- 統合テストは修正前から成功するため、テストコメントに「BTreeMap 畳み込みの有無を検出せず、コンテナ実挙動の固定化を目的とする。畳み込み規則は単体テストが検証する」旨を明記した
- 実装コメントには観測事実 (重複 Env でも Docker Engine が解決するが、ランタイム実装依存の挙動を排除し経路間の規則を統一する) を記載した
- `CHANGES.md` に `[FIX]` エントリを追加した
- 検証: Linux テスト一式 268 件・macOS 側 321 件全 pass を確認した
