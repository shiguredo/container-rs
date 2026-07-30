# リファクタリング: canary.py の不正確なコメントと関数名を修正する

- Priority: Low
- Created: 2026-07-29
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/refactor-canary-comments
- Polished: 2026-07-30

## 目的

canary.py のコメントと関数名が実態と一致しておらず、誤読を招く。コメントを関数の実態に合わせて修正し、関数名を実態に合った名前に変更する。

## 現状

- `git_commit_version` 関数上方のコメント `# git コミット、タグ、プッシュを実行` は、同関数が add と commit のみを行うため不正確
- `git_operations_after_build` 関数上方のコメントも同一文面だが、同関数は tag と push のみでコミットを行わないため不正確
- 関数名 `git_operations_after_build` に対応する「build」はスクリプト内に存在しない

## 設計方針

- `git_commit_version` 関数上方のコメントを実態 (add とコミット) に合わせて修正する
- `git_operations_after_build` 関数上方のコメントも実態 (タグとプッシュ) に合わせて修正する
- 関数名 `git_operations_after_build` を `git_tag_and_push` に変更する。`main()` 内の呼び出し側も合わせて修正する
- `main()` 内のインラインコメント `# git タグ付け、プッシュ` は既に正確なため変更しない
- 0046 (`feature/fix-canary-prompt-confirm`) と 0047 (`feature/add-canary-version-test`) も同一ファイル `canary.py` を変更する。変更箇所は重複しないが、マージ順に注意する

## 完了条件

- [ ] `git_commit_version` 関数上方と `git_operations_after_build` 関数上方のコメントが関数の実態に合わせて修正されること
- [ ] 関数名 `git_operations_after_build` が `git_tag_and_push` に変更され、`main()` 内の呼び出し側も更新されること
