# バグ: canary.py の確認プロンプトが空入力をキャンセル扱いにし、dry-run が非対話で実行できない

- Priority: Low
- Created: 2026-07-29
- Completed: 2026-07-31
- Model: qwen3.8-max-preview
- Branch: feature/fix-canary-prompt-confirm
- Polished: 2026-07-30

## 目的

canary.py の確認プロンプトの表記と挙動を一致させ、dry-run を非対話化する。

## 現状

- プロンプトと挙動の不一致 (`update_version` 関数内の `confirmation` 判定): プロンプトは `Do you want to update the version? (Y/n):` で、慣例どおりなら Enter (空入力) は Yes のはずだが、判定が `if confirmation != "y"` のため空入力はキャンセル扱いになる
- `--dry-run` でも対話確認 (`input()`) が必須 (`update_version` 関数内の確認 → dry-run 判定の順序): dry-run を検証用途で非対話に実行できない

## 設計方針

- 確認プロンプトの挙動を表記どおりにする: 空入力を Yes として扱い、`y` / `yes` / 空入力を許可、`n` / `no` / その他をキャンセルとする。既存の `.strip().lower()` による入力正規化は維持する
- dry-run 時は対話確認をスキップする (dry-run 判定を確認プロンプトより前に行い、dry-run なら確認を経ずにファイル全体出力と `new_version` の返却まで進める。既存のファイル全体出力は維持する)
- dry-run 時も `new_version` を返し、`main()` の後続処理 (`run_cargo_update` 等の dry-run 表示) を継続する

## 完了条件

- [ ] 確認プロンプトで `y` / `yes` / Enter (空入力) が Yes として扱われること
- [ ] `n` / `no` / 上記以外の入力でキャンセルされること
- [ ] `--dry-run` 実行時に対話確認なしでファイル全体出力と新バージョン文字列が表示されること
- [ ] `python3 canary.py --dry-run < /dev/null` で確認プロンプトが出ずに完了することを確認できること (`input()` が残っていれば `EOFError` で失敗するため、非対話性を機械的に検証できる)

## 解決方法

`canary.py` の `update_version` 関数を修正した。

1. dry-run 分岐を確認プロンプトより前に移動し、dry-run 時は `input()` を呼ばずにファイル全体出力と `new_version` の返却まで進めるよう変更した。これにより `python3 canary.py --dry-run < /dev/null` で非対話に実行できる
2. 確認判定を `if confirmation != "y"` から `if confirmation not in ("", "y", "yes")` に変更し、`(Y/n)` 慣例どおり空入力 / y / yes を Yes として扱うよう修正した。既存の `.strip().lower()` による入力正規化は維持している
3. dry-run 時も `new_version` を返すため、`main()` の後続処理 (`run_cargo_update` / `git_commit_version` / `git_operations_after_build`) が dry-run ガード付きで正しく連鎖する

変更ファイル: `canary.py`、`CHANGES.md`（misc に `[FIX]` エントリ追加）
