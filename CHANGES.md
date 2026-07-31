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
- [CHANGE] Runtime 内 Drop でコンテナ削除の完了を `DROP_REMOVE_TIMEOUT` (5 秒) 内で待つように変更する
  - @voluntas
- [CHANGE] 未使用の公開型 `CgroupnsMode` を削除する
  - @voluntas
- [ADD] Linux で `Healthcheck` / `ImageExt::with_health_check` / `HealthWaitStrategy` の Linux 分岐に対応する
  - @voluntas
- [ADD] `with_copy_to` で親ディレクトリ自動作成とディレクトリ一括投入に対応する
  - @voluntas
- [ADD] Linux で HTTP ログストリームを demux して `stdout` / `stderr` / `LogConsumer` / `WaitFor::Log` を有効化する
  - @voluntas
- [ADD] Linux で Docker Engine API の archive エンドポイント経由の `copy_file_from` / `with_copy_to` を実装する
  - @voluntas
- [ADD] tokio Runtime 内から削除完了を待てる同期 `rm_blocking` を追加する
  - @voluntas
- [ADD] Linux で exec の stdout / stderr 取得と `CmdWaitFor::StdOutMessage` / `StdErrMessage` を実装する
  - @voluntas
- [ADD] Linux で exec の `with_env_vars` を実装する
  - @voluntas
- [ADD] Linux で `get_bridge_ip_address` を実装する
  - @voluntas
- [ADD] Linux で `exit_code` を実装する
  - @voluntas
- [FIX] Linux で `with_exposed_port` / `Image::expose_ports` がホストポート公開に反映されないのを修正する
  - @voluntas
- [FIX] manifest 選択フォールバックで attestation manifest (`architecture: "unknown"`) を除外する
  - @voluntas
- [FIX] FD リーク・タイムアウト欠如・無限ループ・OOM リスク・バリデーション不足のランタイム安全性を修正する
  - @voluntas
- [FIX] macOS の logs() 取得失敗時ロールバックが Keep ゲート無しで remove するのを修正する
  - @voluntas
- [FIX] Linux ログストリームの fallback fd の use-after-close 競合をフォールバック廃止で解消する
  - @voluntas

### misc

- [UPDATE] canary.py のバージョン変換ロジックを純粋関数 `next_canary_version` として抽出し unittest テストを追加する
  - @voluntas
- [UPDATE] canary.py の不正確なコメントと関数名 `git_operations_after_build` を `git_tag_and_push` に修正する
  - @voluntas
- [FIX] canary.py の確認プロンプトで空入力がキャンセル扱いになるのと dry-run が非対話で実行できないのを修正する
  - @voluntas
