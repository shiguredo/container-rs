# バグ: canary.py の確認プロンプトが空入力をキャンセル扱いにする

- Priority: Low
- Created: 2026-07-29
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/fix-canary-prompt-confirm

## 目的

canary.py の確認プロンプトは `Do you want to update the version? (Y/n):` と表示するが、判定が `if confirmation != "y"` のため、慣例上 Yes を意味する Enter (空入力) がキャンセル扱いになる。また `--dry-run` 時も対話確認 (`input()`) が必須で、CI や検証用途で非対話に実行できない。プロンプト表記と挙動を一致させ、dry-run を非対話化する。

## 優先度根拠

Low。空入力は安全側 (キャンセル側) に倒れるため実害は小さい。dry-run の対話確認も手動実行では実害がない。ただしプロンプト表記と挙動の不一致はリリース担当者の誤操作を招く可能性がある。

## 現状

- プロンプトと挙動の不一致 (`canary.py:68-72`): プロンプトは `Do you want to update the version? (Y/n):` で、慣例どおりなら Enter (空入力) は Yes のはずだが、判定が `if confirmation != "y"` のため空入力はキャンセル扱いになる
- `--dry-run` でも対話確認 (`input()`) が必須 (`canary.py:68-85`): `update_version` 内で確認 → dry-run 判定の順序になっているため、dry-run を CI や検証用途で非対話に実行できない

## 設計方針

- 確認プロンプトの挙動を表記どおりにする: 空入力を Yes として扱い、`y` / `yes` / 空入力を許可、`n` / `no` / その他をキャンセルとする
- dry-run 時は対話確認をスキップする (dry-run 判定を確認プロンプトより前に行い、dry-run なら確認を経ずに新バージョン文字列の表示まで進める。既存のファイル全体出力は維持する)

## 完了条件

- [ ] 確認プロンプトで `y` / `yes` / Enter (空入力) が Yes として扱われること
- [ ] `--dry-run` 実行時に対話確認なしで新バージョン文字列が表示されること
