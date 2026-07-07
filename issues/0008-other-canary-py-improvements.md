# その他: canary.py の使い勝手とテストを改善する

- Priority: Low
- Created: 2026-07-12
- Completed:
- Model: Kimi
- Branch: feature/update-canary-py
- Polished: 2026-07-21

## 目的

canary.py は canary リリース用にバージョンの bump、git commit、tag、push までを一括で行うスクリプトで、tag push を契機に `.github/workflows/release.yml` が GitHub Release 作成と crates.io への publish を行う。つまりこのスクリプトが生成したバージョン文字列や git 操作の結果がそのままリリースフローに直結する。コードレビューで確認した以下の問題をまとめて修正し、バージョン変換ロジックにテストを追加する。

## 優先度根拠

いずれも実害が出にくい問題のため Low。確認プロンプトの不一致は安全側 (キャンセル側) に倒れる、dry-run の対話確認は手動実行では実害がない、バージョン変換はリリース担当者が確認プロンプトで目視する機会がある。ただし変換ロジックにテストが無いため、正規表現を誤って変更した場合に壊れたバージョン文字列を commit / tag / push するところまで進むリスクがあり、テストの追加は意味がある。

## 現状

確認した問題点:

- プロンプトと挙動の不一致 (`canary.py:68-72`): プロンプトは `Do you want to update the version? (Y/n):` で、慣例どおりなら Enter (空入力) は Yes のはずだが、判定が `if confirmation != "y"` のため空入力はキャンセル扱いになる。安全側に倒れるため被害は小さいが、プロンプト表記に合わせて修正する
- `--dry-run` でも対話確認 (`input()`) が必須 (`canary.py:68-85`): `update_version` 内で確認 → dry-run 判定の順序になっているため、dry-run を CI や検証用途で非対話に実行できない
- バージョン書き換えロジック (`canary.py:32-49`) にテストが無い: `-canary.N` のインクリメントと、canary サフィックスが無い場合の次マイナーバージョン + `-canary.0` 付与の 2 経路がある。正規表現のグループ構成を誤ると壊れたバージョン文字列のまま commit / tag / push まで進む
- コメントの重複・不正確さ: `canary.py:97` のコメント `# git コミット、タグ、プッシュを実行` は `git_commit_version` 関数 (add と commit のみ) に対して不正確。`canary.py:111` も同一コメントだが、`git_operations_after_build` 関数は tag と push のみでコミットを行わない。また関数名 `git_operations_after_build` に対応する「build」はスクリプト内に存在しない

## 設計方針

- 確認プロンプトの挙動を表記どおりにする: 空入力を Yes として扱い、`y` / `yes` / 空入力を許可、`n` / `no` / その他をキャンセルとする
- dry-run 時は対話確認をスキップする (dry-run 判定を確認プロンプトより前に行い、dry-run なら確認を経ずに変換結果の表示まで進める)
- バージョン変換部を純粋な文字列変換の関数として抽出する。抽出する関数は `def next_canary_version(version: str) -> str` (バージョン文字列を受け取り次の canary バージョンを返す) とし、ファイル I/O や `input()` を含まない純粋関数にする。標準ライブラリ `unittest` によるテストを追加する。テストファイルは `test_canary.py` をリポジトリ直下に置いて `python3 -m unittest` で実行できる形にする。CI / prek への統合は今回は行わない (手動実行のみ)
- テストケースは最低限、canary バージョンのインクリメント (`1.2.3-canary.0` → `1.2.3-canary.1`)、通常バージョンからの変換 (`1.2.3` → `1.3.0-canary.0`) を含める
- テストファイルのコメント・ログメッセージは AGENTS.md の規約に従う (コメントは日本語、テストのログメッセージは日本語)
- `:97` のコメントを実態 (add とコミット) に合わせて修正する。`:111` のコメントも実態 (タグとプッシュ) に合わせて修正する。関数名 `git_operations_after_build` も実態に合った名前 (例: `git_tag_and_push`) に変更する

## 完了条件

- [ ] 確認プロンプトで Enter (空入力) が Yes として扱われること
- [ ] `--dry-run` 実行時に対話確認なしで変換結果が表示されること
- [ ] バージョン変換ロジックが純粋関数として抽出され、上記のテストケースを含む `unittest` テストが追加され、`python3 -m unittest` で pass すること
- [ ] `:97` と `:111` のコメントが関数の実態に合わせて修正され、関数名 `git_operations_after_build` が実態に合った名前に変更されること
