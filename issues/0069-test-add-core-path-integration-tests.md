# テスト: stop の SIGTERM 経路・CmdWaitFor 未検証バリアント・volume_mount / ReadOnly の統合テストを追加する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/add-core-path-integration-tests
- Polished: {YYYY-MM-DD}

## 目的

「実装はあるが一度も実機を通っていない」主要経路の統合テストを追加し、バックエンドごとの実挙動の欠陥を検出できるようにする。

## 現状

- `ContainerAsync::stop_with_timeout` の検証は `Some(0)` (SIGKILL) のみ。既定の `None` (SIGTERM + 30 秒)・負値・60 秒超のグレースは全 OS で未テスト (`tests/helpers/mod.rs` の規約が `Some(0)` を強制した結果、SIGTERM の graceful 経路が一度も実機検証されていない)
- `CmdWaitFor::Exit { code: None }` と `Duration` 系 (`seconds` / `millis`) が全 OS 未テスト。macOS の exec 経路 (`XpcClient::exec`) の `StdErrMessage` も未テスト
- `Mount::volume_mount` と `AccessMode::ReadOnly` の実ランタイム経由の統合テストが皆無 (bind / tmpfs のみ)。`ImageExt::with_copy_to` の rustdoc が ReadOnly 併用を推奨しているのに実機検証が無い

## 設計方針

各バックエンドの実装が完全に分離 (XPC vs Docker Engine API) しているため、片 OS の検証はもう片方の保障にならない。macOS と Linux の両方で必要な経路を検証する。

## 完了条件

- `stop` / `stop_with_timeout` の SIGTERM 経路 (None / 正のグレース / 負値) が検証される
- `CmdWaitFor` の全バリアントが検証される
- `volume_mount` と `AccessMode::ReadOnly` が実機で検証される

## 解決方法

- `trap 'exit 0' TERM` を仕込んだ init で graceful 停止を検証するテストを追加する (片 OS でも可)
- `CmdWaitFor::exit()` (終了コード不問)・`CmdWaitFor::millis` のテストを追加し、macOS の exec で `message_on_stderr` のテストを追加する
- Linux で `volume_mount` と ReadOnly bind を各 1 本追加する (macOS の volume は別の既知バグの修正後に検証する)
