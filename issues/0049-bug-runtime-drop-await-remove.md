# バグ: Runtime 内 Drop でコンテナ削除完了を保証する

- Priority: High
- Created: 2026-07-29
- Completed: {YYYY-MM-DD}
- Model: Composer
- Branch: feature/fix-runtime-drop-await-remove
- Polished: {YYYY-MM-DD}
- Reporter: @voluntas

## 目的

tokio Runtime 内で `ContainerAsync` / `Container` を Drop したとき、削除スレッドの完了を待ち、プロセス終了直後でも孤立コンテナを残さないようにする。
利用側の `force_remove_container` や MinioGuard / RustfsGuard のような手動クリーンアップを不要にし、本家 testcontainers-rs と同様に「保持するだけで掃除される」体験に戻す。

## 優先度根拠

利用者フィードバック (mqtt-rs 移行)。Runtime 内 Drop が削除スレッドを join しないため、テストプロセス終了時に孤立コンテナが溜まる。利用側は CLI 強制削除と Guard で自衛せざるを得ない。`0035` で契約文書化、`0042` で明示 `rm_blocking` を追加予定だが、いずれも「Drop だけで確実削除」にはならない。ライブラリ側が解決すべき最大ギャップであり High。

## 現状

- Runtime 内 Drop (`src/core/containers/async_container.rs`): `std::thread::spawn` で `remove_blocking` を実行するが **join しない** (fire-and-forget)。コメントは「async Drop からの join は deadlock し得る」としている
- `remove_blocking` 自体は macOS / Linux とも tokio Runtime に依存しない同期実装
- Runtime 外 Drop は呼び出しスレッドで同期実行し試行終了まで待つ
- `0035` (closed): 上記を「完了非保証」契約として README / rustdoc / TESTCONTAINERS に固定。挙動変更はスコープ外だった
- `0042` (open): `rm_blocking()` 追加。Drop の fire-and-forget は維持し、timeout 付き join は「別 issue」としている

## 設計方針

1. Runtime 内 Drop で削除専用スレッドを **timeout 付き join** する。`remove_blocking` は Runtime 非依存のため、tokio ワーカー上の Drop から join しても Runtime 待ち合いの deadlock にはならない前提で進める（実装時に根拠を rustdoc に書く）
2. timeout 超過時は現行同様に諦めて `tracing::error` し、Drop 自体は panic しない（呼び出し側に Result は返せないため）
3. `0035` で固定した「Runtime 内 Drop は完了非保証」契約を撤回し、README / `docs/TESTCONTAINERS.md` / SKILL / rustdoc を「timeout 内で完了を待つ。超過時は best-effort」に更新する
4. `0042` の `rm_blocking` とは並立する。明示削除と成否 `Result` が必要な経路は `rm` / `rm_blocking`、何もしない Drop での掃除保証は本 issue
5. Linux 向け reaper / watchdog 拡張はスコープ外

## 完了条件

- Runtime 内で `ContainerAsync` を Drop した直後（timeout 内）に、1 ショットの不在確認が通る統合テストが pass する
- 利用側が CLI 強制削除や専用 Guard なしで Drop のみ掃除できることが文書上も明確である
- `0035` 由来の「完了非保証」記述が README / TESTCONTAINERS / SKILL / rustdoc から除去または書き換えられている
- `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass する

## 解決方法

`ContainerAsync::Drop` の Runtime 内分岐を、`spawn` 後に timeout 付き `join` する形へ変更する。timeout 値は既存のログ側 Drop polling（最大 1 秒）や CI 許容遅延を参考に決め、定数化する。契約文書と統合テスト（Runtime 内 Drop → 即不在 assert）を合わせて更新する。
