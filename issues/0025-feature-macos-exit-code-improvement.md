# 機能追加: macOS で exit_code の都度 containerWait 取得を対応する

- Priority: Low
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/macos-exit-code-improvement
- Polished:

## 目的

macOS (Apple Container) バックエンドで `ContainerAsync::exit_code` の精度を改善する。現状はバックグラウンド wait の観測済みキャッシュのみを返すため、コンテナが停止済みでもバックグラウンド wait が未完了の場合は `None` を返し得る。

## 優先度根拠

現状でもバックグラウンド wait が完了していれば正しい exit code を返すため、実用上の問題は限定的。停止直後の即時取得が必要なケースは稀であり、`ExitWaitStrategy` はポーリングで対応している。Low。

## 現状

- `exit_code()` は `WaitState` のキャッシュを参照し、未観測時は `None` を返す
- コンテナが停止済みでも、バックグラウンドの `containerWait` スレッドがまだ完了していない場合は `None` になる
- 停止済みかつ未観測の場合に都度 `containerWait` を呼ぶ経路は無い (runtime client が解放済みの場合にブロック・失敗するため)

## 設計方針

- 停止済みかつ未観測の場合に、タイムアウト付きで `containerWait` を呼ぶ経路を追加する
- `containerWait` の呼び出しは XPC のブロッキング呼び出しになるため、`spawn_blocking` で実行する
- タイムアウト (例: 5 秒) 内に exit code が取得できなかった場合は `None` を返す
- runtime client が解放済みの場合のエラーハンドリングを適切に行う

## 完了条件

- [ ] コンテナ停止後に `exit_code()` が確実に exit code を返すこと
- [ ] タイムアウト内に取得できなかった場合は `None` を返すこと (エラーにしない)
- [ ] 単体テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
