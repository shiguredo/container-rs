# テスト: restart_rearms_log_stream が「再武装」を検証できていないのを修正する

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-restart-log-rearm-verification
- Polished: 2026-08-12

## 目的

`tests/container_linux.rs` の `restart_rearms_log_stream` が、テストコメントの主張 (「新規リーダーが新バッファに接続されることを検証する」) を実際には検証できていないため、再武装の有無で結果が**変わる**観測に修正する。

## 現状

- `tests/container_linux.rs` の `restart_rearms_log_stream` は、stop → start 後に `stdout_to_vec()` で marker A と異なる marker B が読めることを条件にする
- しかし `stdout_to_vec()` は `stdout(false)` → 1-shot リーダー (`follow=false`) で、`stdout()` の doc コメント (`async_container.rs`) のとおり**新規 HTTP セッションを張って全ログを取得する**。1-shot 経路はローカル共有バッファ (`log_source`) を一切経由せず、`socket_path` / `id` だけで毎回 daemon へ直接接続する
- 新規セッションはコンテナのログ全体を返すため、`refresh_log_streams` (再武装) が**実行されなくても** marker B は読める
- つまり再武装の有無でテスト結果が変わらず、回帰を検出できない

## 設計方針

- 再武装の有無で結果が変わる観測は「**再起動後に取得した新規 follow リーダーが新ログ (marker B) を読めること**」だけである。再武装が実行されない (旧実装相当) 場合、`log_source` が終端済み旧ハンドルのままのため、新規 follow リーダーは即 EOF になり marker B が読めず失敗する
- 「再起動前に取得した follow リーダー (`stdout(true)`) が stop で EOF になる」は再武装の有無と無関係に成立するため、判別観測にしない (stop は `stop_log_delivery` → `handle.stop()` で再武装と独立に必ずハンドルを終端する)。旧リーダーの EOF は再武装失敗実装でも成立し、検証力を持たない
- 観測はすべて効果の間接観測である (`log_source` の差し替えは private フィールド (型自体は `pub(crate)`) で統合テストからは読めない)。「直接観測」ではなく再武装の効果を観測する、という位置づけにする
- 既存の `stop_terminates_log_stream_for_new_reader` は「stop 後に**新規**リーダーを開くと EOF」を検証するテストであり、再武装の検証とは観測対象が異なるため、本 issue では組み合わせない (既存テストは変更せず併存させる)

## 完了条件

- 再武装 (`refresh_log_streams`) が実行されない (旧実装相当) 場合に失敗するテストになること (検出対象は「呼ばない」実装であり、「`Err` を返す」実装ではない。後者は start がエラー伝播で失敗するため、どのテストでも落ちてしまい判別力がない)
- 判別の確認: 実装時に、`refresh_log_streams` の本体から新規セッション起動と `log_source` の差し替え (再武装) を一時的に消し、コンテナ再起動だけを残して当該テストが失敗すること (新規 follow リーダーが EOF で即失敗する) を手動で確認し、確認後に元へ戻す (`refresh_log_streams` は Linux の start 経路でコンテナ再起動も担うため、呼び出し全体を no-op にすると再武装の欠落に隔離できない)
- 正常系では通ること
- `CHANGES.md` の `### misc` に `[UPDATE]` エントリ (担当者行つき) が追加されていること

## 解決方法

- `tests/container_linux.rs` の `restart_rearms_log_stream` を、1-shot 取得ではなく follow リーダー (`stdout(true)`) を使う観測に書き換える。フローは次のとおり:
  1. 再起動前に follow リーダーを開き、そのリーダー経由で marker A を取得する (ステップ 3 の比較材料の準備。tail=all の共有バッファ経由のため、接続の生死確認にはならない)
  2. 即時停止 (`stop_with_timeout(Some(0))`) → start を実行する (既存テストと同じ SIGKILL 即時停止を維持する)
  3. 再起動後に新規 follow リーダーを開き、marker A とは異なる marker B が読めることを確認する (再武装の有無で結果が変わる判別観測)
- 新規 follow リーダーのセッションは `tail=all` のため marker A も含まれる。判別は既存ヘルパー `last_restart_marker` (最後の marker を抽出) と `marker_b != marker_a` の比較で行う (既存の判定ロジックを再利用する)
- marker A / marker B の待ちは、コンテナ生存中 (cmd 末尾 `tail -f /dev/null`) は follow リーダーが EOF しないため、既存の `stdout_follow_stream_reads_marker` と同様に read + deadline ポーリング (10 秒程度) で行う
- テストコメントを新しい観測 (再起動後の新規 follow リーダーが新ログを読めることで再武装を検証する) に合わせて書き換える。テスト名 `restart_rearms_log_stream` は維持する
- `CHANGES.md` の `### misc` に `[UPDATE]` エントリを追加する
