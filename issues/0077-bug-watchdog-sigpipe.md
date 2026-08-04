# バグ: watchdog の reaper への書き込みが SIGPIPE でテストプロセスごと死ぬ

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-watchdog-sigpipe
- Polished: {YYYY-MM-DD}

## 目的

watchdog feature 有効時、reaper プロセスが親プロセス生存中に終了した場合に、次のコンテナ登録が SIGPIPE で**テストプロセス全体を終了**させる問題を修正する。

## 現状

- `src/watchdog.rs` の `write_id` は `ChildStdin` への `writeln!` + `flush` の成否 (`is_ok()`) で pipe の死活を判定し、失敗時は respawn 経路 (`register`) で reaper を再起動する設計
- しかし Rust の std は SIGPIPE のデフォルト disposition (プロセス終了) を変更しないため、読み取り端が閉じた pipe への `write` は EPIPE が返る前に **SIGPIPE シグナルでプロセス全体が終了**する
- プロジェクト内に SIGPIPE を無視する設定は無い (`SIGPIPE` / `signal` はクレート内 0 件)
- 発火条件は「親プロセス生存中に reaper が死ぬ」こと (外部 kill・シェル起動の異常等)。このとき respawn ロジック (`watchdog.rs` の `register`) に到達せず、以後の登録が無効になるだけでなくテストプロセスが無言で死ぬ

## 設計方針

- Rust の `std::io::Write` は SIGPIPE を EPIPE に変換しないため、プロセス全体への SIGPIPE を無視 (SIG_IGN) する設定を入れる
- 設定箇所は `register` の初回呼び出し時 (または crate 初期化時) に 1 回だけ行う

## 完了条件

- reaper が親プロセス生存中に終了しても、次の `register` が SIGPIPE でプロセスを殺さず、respawn 経路 (`write_id` の `Err` → 再 spawn) に進むこと
- `container rm --force` が実行されない環境 (reaper 死亡後の書き込み) でもテストプロセスが生存すること

## 解決方法

- `src/watchdog.rs` の `register` 初回呼び出し時に `libc::signal(libc::SIGPIPE, libc::SIG_IGN)` を設定する (libc は既存依存)
- 設定後に pipe 死亡 → EPIPE → respawn の経路が成立することを確認するテストを `src/watchdog.rs` の単体テストに追加する (reaper を終了させてから `register` を呼び、プロセスが生存し respawn されることを検証する)
