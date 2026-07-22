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

- [ADD] Linux で HTTP ログストリームを demux して `stdout` / `stderr` / `LogConsumer` / `WaitFor::Log` を有効化する
  - @voluntas
- [FIX] Linux で `with_exposed_port` / `Image::expose_ports` がホストポート公開に反映されないのを修正する
  - @voluntas

### misc
