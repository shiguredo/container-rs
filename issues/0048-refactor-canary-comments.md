# リファクタリング: canary.py の不正確なコメントと関数名を修正する

- Priority: Low
- Created: 2026-07-29
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/refactor-canary-comments

## 目的

canary.py のコメントと関数名が実態と一致しておらず、誤読を招く。コメントを関数の実態に合わせて修正し、関数名を実態に合った名前に変更する。

## 優先度根拠

Low。コメントと関数名の不正確さは実害を生まないが、コードの可読性と保守性を損なう。

## 現状

- `canary.py:97` のコメント `# git コミット、タグ、プッシュを実行` は `git_commit_version` 関数 (add と commit のみ) に対して不正確
- `canary.py:111` も同一コメントだが、`git_operations_after_build` 関数は tag と push のみでコミットを行わない
- 関数名 `git_operations_after_build` に対応する「build」はスクリプト内に存在しない

## 設計方針

- `:97` のコメントを実態 (add とコミット) に合わせて修正する
- `:111` のコメントも実態 (タグとプッシュ) に合わせて修正する
- 関数名 `git_operations_after_build` を実態に合った名前 (例: `git_tag_and_push`) に変更する。呼び出し側 (`main()` 内 `canary.py:150`) も合わせて修正する

## 完了条件

- [ ] `:97` と `:111` のコメントが関数の実態に合わせて修正されること
- [ ] 関数名 `git_operations_after_build` が実態に合った名前に変更され、呼び出し側も更新されること
