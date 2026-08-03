# バグ: Linux with_copy_to のターゲットパスが中間 .. を許容する

- Created: 2026-08-02
- Completed: 2026-08-03
- Branch: feature/fix-linux-copy-to-path-validation
- Polished: 2026-08-02

## 目的

`with_copy_to` (Linux 分岐) のターゲットパス検証が中間 `..` を通すため、意図した「明示エラー」ではなく daemon の挙動依存になる問題を解消する。

## 現状

- `src/runners/async_runner.rs` の `copy_to_sources_linux` のパス検証は、絶対パス・末尾スラッシュ・`file_name()` が `None` になるケース (末尾が `..` のパス) のみを検査する
- `/tmp/../etc/passwd` のような**中間** `..` は検証を通り、`make_path_relative` で `tmp/../etc/passwd` となり、`append_ancestor_directories` が `tmp/` や `tmp/../` のディレクトリエントリを tar に載せる
- 最終的に Docker daemon の挙動に依存する。daemon が `..` を含む tar エントリを拒否する場合は cryptic なエラー、`..` を解決する場合はコンテナ内の任意ファイル (`/etc/passwd` 等) をサイレント上書きし得る。どちらになるかは実測で確定する (検証の意図である「ライブラリ側で明確なエラー」を満たしていない)

## 設計方針

パスをコンポーネント単位で検査し、`..` (ParentDir) を含むターゲットパスを拒否する。エラー文言は既存パターンに合わせて `copy_to target path must not contain '..'` とし、`PathNameError` で明示エラーを返す。`.` (CurDir) と空コンポーネント (`//`) は `Path::components()` が正規化で消すため、ParentDir 検査の対象外 (許容される)。ただし `make_path_relative` 後の tar エントリ名には `.` が残り得るため、daemon が CurDir 入りエントリ名を受理するかは実装の最初に実測し、結果を現状セクションに記録する (拒否される場合は CurDir も拒否対象に含めることを検討する)。

macOS 経路 (`copy_to_sources`) は検証なし・素通しのまま (現状維持。本 issue の対象外。macOS は tar を自前構築せず XPC `containerCopyIn` にホストパスを直接渡すため、本 issue の対象機構である「tar エントリに `..` が載る」経路自体が存在しない)。

従来は `..` 入りパスが daemon 次第で成功 / 失敗に分かれていたが、修正後は常にライブラリ側で拒否する (利用者に見える挙動変化として許容する。末尾 `..` のエラー文言は既存の `copy_to target path must have a file name` のまま)。

## 完了条件

- `..` (ParentDir) を含むターゲットパス (中間 `/tmp/../etc/passwd`・先頭 `/../etc/passwd` 等) が `copy_to` の検証で拒否される
- 通常パスは従来どおり動作する (CurDir 入りパスは現状の実測結果に従う)
- `CHANGES.md` に `[FIX]` エントリが追加されている

## 解決方法

- `src/runners/async_runner.rs` の `copy_to_sources_linux` のパス検証に、`std::path::Component::ParentDir` (中間・先頭の `..`) を含むターゲットパスの拒否チェックを追加する (挿入位置は既存の `file_name()` チェックの後。末尾 `..` は既存チェックが先に捕捉し、エラー文言も既存のまま維持)。エラー文言は `copy_to target path must not contain '..'` で `PathNameError`
- 実測の結果、daemon は中間 CurDir (`/tmp/./x`) は受理するが、終端 CurDir (`/tmp/.`) と空コンポーネント (`/tmp//x`) は正しく展開しないことを確認したため、両者も明示的に拒否する (`ends_with("/.")` と `contains("//")` の文字列チェック。`Path::components()` は正規化で消すため捕捉できない)
- `with_copy_to` の rustdoc (`src/core/image/image_ext.rs`) に「ターゲットパスに `..` ・終端の `.`・空コンポーネント (`//`) は使えない」旨を追記する
- 統合テスト: `tests/container_linux.rs` に 6 本追加する (中間 `..` 拒否・先頭 `..` 拒否・末尾 `..` 既存文言回帰・終端 CurDir 拒否・空コンポーネント拒否・中間 CurDir 許容 (ファイル到達まで検証))。エラー型は `CopyToContainerError::PathNameError` を `downcast_ref` + `matches!` で検証するヘルパー `assert_path_name_error` に集約する
- 実測は Docker Desktop (docker 29.6.2) で実施し、全 51 統合テスト + 158 単体テストの通過を確認済み
- `CHANGES.md` に `[FIX]` エントリを追加する
