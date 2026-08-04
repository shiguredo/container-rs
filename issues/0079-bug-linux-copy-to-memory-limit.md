# バグ: Linux の `with_copy_to` にメモリ上限がなく巨大ファイルで OOM し得る

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-linux-copy-to-memory-limit
- Polished: {YYYY-MM-DD}

## 目的

issue 0064 で修正した「無制限メモリ蓄積」と同じクラスの問題が Linux の `with_copy_to` 経路に残っているのを修正する。

## 現状

- 0064 で以下に 64 MiB 上限を設定済み: Linux のログ 1-shot 取得 (`docker_log_stream.rs`)、`copy_file_from` (tar 全体、`docker_client.rs`)、イメージ pull 進捗 (`docker_client.rs`)、exec 出力 (`docker_client.rs`)
- 一方、Linux の `with_copy_to` (`src/runners/async_runner.rs` の `copy_to_sources_linux`) はコピー対象のホストファイルを `std::fs::read` / `tokio::fs::read` で**全体をメモリ読み込み**してから ustar に詰める実装で、サイズ上限が無い
- 巨大なホストファイルや巨大ディレクトリ配下を `with_copy_to` で投入すると、テストプロセスが OOM し得る (exec / ログ経路と非対称)
- あわせて、ホストパスが存在しない / シンボリックリンクの場合のエラー (`CopyToContainerError::IoError`) にパス情報が含まれず、「どのパスが失敗したか」が特定できない (同一関数内の `name_err` はパス入りで不統一)

## 設計方針

- exec / ログと同じ 64 MiB 上限をコピー対象 1 ファイルあたりに適用し、超過時は切り詰めず明示エラーにする
- ディレクトリ一括投入は配下のファイルを列挙して 1 ファイルずつ上限判定する
- エラーにホストパスを含める

## 完了条件

- 64 MiB 超のファイルを `with_copy_to` で投入した場合に、OOM せず明示エラーになること (Linux の単体または統合テスト)
- パス不存在・シンボリックリンク拒否のエラーメッセージに該当ホストパスが含まれること

## 解決方法

- `copy_to_sources_linux` でファイル読み込み前にメタデータからサイズを確認し、上限 (64 MiB、既存の `DOCKER_RESPONSE_BODY_LIMIT` と同値) 超過ならエラーを返す
- 読み込み失敗時は `std::io::Error` にパスを付与して返す (または `IoError` を拡張してパスを保持する)
- テスト: `tests/container_linux.rs` に上限超過のテストを追加する (sparse ファイルで 64 MiB + 1 バイトのファイルを生成して投入し、エラーを検証する)
