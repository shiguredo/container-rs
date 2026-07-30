# 変更履歴

- UPDATE
  - 後方互換がある変更
- ADD
  - 後方互換がある追加
- CHANGE
  - 後方互換のない変更
- FIX
  - バグ修正

## develop

- [CHANGE] MSRV (`rust-version`) を 1.88.0 から 1.93.0 に上げる
  - @voluntas
- [CHANGE] Linux の `with_copy_to` 投入を create 後・start 前完了の公開契約へ変更する（macOS は start 後のまま）
  - @voluntas
- [CHANGE] `WaitContainerError::Unhealthy` を `Unhealthy(String)` 形式に変更する
  - @voluntas
- [ADD] Linux で `Healthcheck` / `ImageExt::with_health_check` / `HealthWaitStrategy` の Linux 分岐に対応する
  - @voluntas
- [ADD] `with_copy_to` で親ディレクトリ自動作成とディレクトリ一括投入に対応する
  - @voluntas
- [ADD] Linux で HTTP ログストリームを demux して `stdout` / `stderr` / `LogConsumer` / `WaitFor::Log` を有効化する
  - @voluntas
- [ADD] Linux で Docker Engine API の archive エンドポイント経由の `copy_file_from` / `with_copy_to` を実装する
  - @voluntas
- [FIX] Linux で `with_exposed_port` / `Image::expose_ports` がホストポート公開に反映されないのを修正する
  - @voluntas

### misc

- [FIX] canary.py の確認プロンプトで空入力がキャンセル扱いになるのと dry-run が非対話で実行できないのを修正する
  - @voluntas
