# バグ: Linux の `with_copy_to` のエラーに失敗したホストパスが含まれず、どのソースが失敗したか特定できない

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-linux-copy-to-error-path
- Polished: 2026-08-12
- Updated: 2026-08-05

## 目的

Linux の `with_copy_to` (`src/runners/async_runner.rs` の `copy_to_sources_linux`) で、ホストパスが存在しない / シンボリックリンクの場合のエラーにパス情報が含まれず、「どのパスが失敗したか」が特定できないのを改善する。

## 現状

- ホストパスが存在しない場合のエラー (`CopyToContainerError::IoError`) にパス情報が含まれない (単一ファイルは `tokio::fs::symlink_metadata` と `read_file_limited_async` (内部の `tokio::fs::File::open` / `read_to_end`)、ディレクトリ walk は `std::fs::read_dir` (エントリ列挙の `collect` を含む) / `std::fs::symlink_metadata` / `read_file_limited` (内部の `std::fs::File::open` / `read_to_end`) の失敗で発生)
- シンボリックリンク拒否のエラー (`CopyToContainerError::PathNameError`、`name_err` 経由) もメッセージにパスを含まない (単一ファイルの `copy_to source is a symlink` とディレクトリ配下の `copy_to source contains a symlink` の 2 経路)
- per-file の上限超過エラーだけはホストパス含有が実装済み (0079 で追加した `SizeLimitExceeded` の `name` にホストパスが入る)。本 issue の対象 (パス不存在・symlink 拒否) は未対応のまま

## 設計方針

- 失敗したホストパスをエラーに含める。本 issue では公開 API の `CopyToContainerError` へのバリアント追加は行わない (パス不存在は `IoError` 側、シンボリックリンク拒否は `PathNameError` のメッセージにパスを埋め込む)。0079 で追加した `SizeLimitExceeded` バリアントは対象外で、新たなバリアント追加はしない
- `IoError` へのパス付与は `std::io::Error::new(e.kind(), ...)` で `ErrorKind` を維持したままメッセージにパスを埋め込む方式とする (source チェーンは途切れるが、メッセージに元エラーの表示を含めて情報を保つ)
- 対象は「パス不存在・シンボリックリンク拒否」の 2 クラスに限定する。同種のパス欠落を持つ他の `name_err` 経路 (non-regular file 拒否・非 UTF-8) は本 issue の対象外とし、対応しない
- macOS は対象外: macOS の `copy_to_sources` は XPC `copy_in` にパスを直渡しするだけで `symlink_metadata` / `read_dir` 等のメタデータ検査が存在しないため、本 issue の問題は構造的に存在しない

## 完了条件

- パス不存在 (単一ファイルソース・ディレクトリソース) とシンボリックリンク拒否 (単一ファイル・ディレクトリ配下) のエラーメッセージに該当ホストパスが含まれること
- `tests/container_linux.rs` の統合テストでパス含有が検証されること
- `CHANGES.md` に `[FIX]` エントリが追加されていること

## 解決方法

- `copy_to_sources_linux` の現状で挙げた IoError 系失敗経路すべて (単一ファイルの `symlink_metadata` / `read_file_limited_async`、walk の `read_dir` / `collect` / `symlink_metadata` / `read_file_limited`) とシンボリックリンク拒否の 2 経路で、ホストパスをエラーに付与する
- テスト: `tests/container_linux.rs` に次を追加する (既存の `copy_to_directory_with_symlink_is_rejected` はパス含有の断言を追加して強化する)
  - パス不存在: 単一ファイルソースとディレクトリソースの各 1 本で、`IoError` 種別の断言とパス含有を検証する (どちらもトップレベルの `symlink_metadata` 失敗が対象で失敗箇所は同一だが、ソース形式を変えてもパスが含まれることを意図的に 2 本で検証する)
  - シンボリックリンク拒否: 単一ファイルソース (新規追加) とディレクトリ配下 (既存テストの強化) の両経路でパス含有を検証する
- パス付与ロジックを private ヘルパーに切り出し、パス付与の変換自体 (`io::Error` + パス → パス付きエラー) をローカル (コンテナ不要) で検証できるようにする。ヘルパーは OS 非依存の位置 (例: `src/core/copy.rs` の private 関数) に置き、単体テストは同ファイルの `#[cfg(test)] mod tests` に置く (`copy_to_sources_linux` は `#[cfg(target_os = "linux")]` ゲート内のため、ヘルパーをそこに置くと macOS でコンパイル・テストできない。統合テストは Docker が必要なため macOS 開発環境では実行できない)。walk 内部の失敗経路は決定的に再現できないため統合テストでは検証せず、各失敗箇所がヘルパーを経由することは実装時のコードレビューで担保する
