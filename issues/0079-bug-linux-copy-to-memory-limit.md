# バグ: Linux の `with_copy_to` にメモリ上限がなく巨大ファイルで OOM し得る

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-linux-copy-to-memory-limit
- Polished: 2026-08-04

## 目的

issue 0064 で修正した「無制限メモリ蓄積」と同じクラスの問題が Linux の `with_copy_to` 経路に残っているのを修正する。

## 現状

- 64 MiB 上限は既に設定済み: exec 出力 (0055 で導入)、Linux のログ 1-shot 取得・`copy_file_from` (tar 全体)・イメージ pull 進捗 (0064 で設定)
- 一方、Linux の `with_copy_to` (`src/runners/async_runner.rs` の `copy_to_sources_linux`) はコピー対象のホストファイルを `std::fs::read` / `tokio::fs::read` で**全体をメモリ読み込み**してから ustar に詰める実装で、サイズ上限が無い
- 巨大なホストファイルや巨大ディレクトリ配下を `with_copy_to` で投入すると、テストプロセスが OOM し得る (exec / ログ経路と非対称)

## 設計方針

- exec / ログと同じ 64 MiB 上限をコピー対象 1 ファイルあたりに適用し、超過時は切り詰めず明示エラーにする (per-file チェックの成功境界は「ちょうど 64 MiB は成功」。ただし tar 全体上限により、ヘッダ・トレーラ・祖先ディレクトリのオーバーヘッド分手前で失敗し得るのは `copy_from` と同じ既知の挙動)
- ディレクトリ一括投入は配下のファイルを列挙して 1 ファイルずつ上限判定する。あわせて、1 ソース (`CopyToContainer` 1 件) につき生成する tar 全体の蓄積にも 64 MiB 上限を適用する (`copy_from` の tar 全体 64 MiB 上限と対称。`UstarBuilder` の蓄積を上限付きにする)
- `CopyDataSource::Data` (メモリ内データ) は読み込みを伴わないため per-file 読み込み上限の対象外だが、tar 全体の蓄積上限は受ける
- per-file の上限超過エラーにホストパスを含める (tar 全体上限のエラーはビルダー側で検出されるため、tar エントリ名で報告する)

## 完了条件

- 64 MiB 超のファイル (単一・ディレクトリ配下とも) を `with_copy_to` で投入した場合に、OOM せず明示エラーになること (Linux の統合テスト)
- 1 ソース (`CopyToContainer` 1 件) につき生成する tar 全体の蓄積が 64 MiB を超える場合も明示エラーになること

## 解決方法

- `copy_to_sources_linux` でファイル読み込み前にメタデータからサイズを確認し、上限 (64 MiB、既存の `DOCKER_RESPONSE_BODY_LIMIT` と同値) 超過ならエラーを返す (メタデータ確認と読み込みの間にファイルが成長する TOCTOU に備え、読み込み側でも上限を担保する)。per-file の上限超過エラーには該当ホストパスを含める
- `UstarBuilder` の蓄積に 64 MiB 上限を設け、tar 全体の超過をエラーにする (上限判定はトレーラを含む tar 全体で行い、`copy_from` と同じ定義に合わせる)
- テスト: `tests/container_linux.rs` に上限超過のテストを追加する (sparse ファイルで 64 MiB + 1 バイトのファイルを生成して投入し、エラーとパス含有を検証する。ディレクトリ配下のファイルが個別上限を超えるケースと、tar 全体の蓄積が上限を超えるケースも検証する)
