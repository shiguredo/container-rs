# テスト: restart_rearms_log_stream が「再武装」を検証できていないのを修正する

- Created: 2026-08-04
- Completed: 2026-08-12
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

- `tests/container_linux.rs` の `restart_rearms_log_stream` を書き換えた。再起動前に開いた follow リーダーで marker A を取得し、stop → start 後に開いた新規 follow リーダーで `last_restart_marker` が marker A と異なる marker B を返すことを確認する。再武装が実行されない場合、新規 follow リーダーは旧バッファを読み切った後 EOF になり marker B が読めず失敗する (判別観測)
- 新規 follow リーダーのセッションは `tail=all` のため marker A も含まれる。判別は `last_restart_marker` (最後の marker を抽出) と `marker_b != marker_a` の比較で行う (既存の判定ロジックを再利用する)
- marker A / marker B の待ちは、コンテナ生存中 (cmd 末尾 `tail -f /dev/null`) は follow リーダーが EOF しないため、既存の `stdout_follow_stream_reads_marker` と同様に read + deadline ポーリング (10 秒程度) で行う。read はデッドラインの残り時間でタイムアウトを張り、タイムアウト時は取得済みログを含むメッセージで失敗する
- `last_restart_marker` を `split_inclusive` + `strip_suffix` による完全行のみ抽出に修正した (読み込み境界で分割された途中行を marker と誤認しないため)
- テストコメントを新しい観測 (再起動後の新規 follow リーダーが新ログを読めることで再武装を検証する) に合わせて書き換えた。テスト名 `restart_rearms_log_stream` は維持する
- 判別確認: `refresh_log_streams` から新規セッション起動と `log_source` の差し替えを一時的に除去し (コンテナ再起動だけを残す)、当該テストが失敗することを Docker コンテナ内 (Linux) で確認した。確認後に元へ戻した
- 正常系: Docker コンテナ内 (Linux) で `cargo test --all-features --test container_linux` が 57 件全 pass することを確認した
- `CHANGES.md` の `### misc` に `[UPDATE]` エントリ (担当者行つき) を追加した
