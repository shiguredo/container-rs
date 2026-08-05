# バグ: Linux の `with_copy_to` のエラーに失敗したホストパスが含まれず、どのソースが失敗したか特定できない

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-copy-to-error-path
- Polished: {YYYY-MM-DD}
- Updated: 2026-08-05

## 目的

Linux の `with_copy_to` (`src/runners/async_runner.rs` の `copy_to_sources_linux`) で、ホストパスが存在しない / シンボリックリンクの場合のエラーにパス情報が含まれず、「どのパスが失敗したか」が特定できないのを改善する。

## 現状

- ホストパスが存在しない場合のエラー (`CopyToContainerError::IoError`) にパス情報が含まれない (単一ファイルは `tokio::fs::symlink_metadata` の失敗、ディレクトリ walk は `std::fs::read_dir` / `std::fs::symlink_metadata` / `read_file_limited` (内部の `std::fs::File::open` / `read_to_end`) の失敗で発生)
- シンボリックリンク拒否のエラー (`CopyToContainerError::PathNameError`、`name_err` 経由) もメッセージにパスを含まない
- 上限超過のエラーだけはホストパス含有が実装済み (0079 で追加した `SizeLimitExceeded` の `name` にホストパスが入る)。本 issue の対象 (パス不存在・symlink 拒否) は未対応のまま

## 設計方針

- 失敗したホストパスをエラーに含める。`std::io::Error` のメッセージにパスを付与して返す方式とし、本 issue では公開 API の `CopyToContainerError` へのバリアント追加は行わない (パス不存在は `IoError` 側、シンボリックリンク拒否は `PathNameError` のメッセージにパスを埋め込む)
- 0079 で `SizeLimitExceeded` バリアントが追加済みのため、「公開 enum へのバリアント追加は行われない」という前提は既に崩れている。本 issue でさらに追加する必要はないという意味に限定する

## 完了条件

- パス不存在・シンボリックリンク拒否のエラーメッセージに該当ホストパスが含まれること

## 解決方法

- `copy_to_sources_linux` の各失敗経路 (`symlink_metadata` / `read_dir` / `read_file_limited` の失敗、シンボリックリンク拒否) で、ホストパスをエラーに付与する
- テスト: `tests/container_linux.rs` にパス不存在・シンボリックリンク拒否でパス含有を検証するテストを追加する
