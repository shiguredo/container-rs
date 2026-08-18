# リファクタリング: 未使用のエラー型バリアントを削除する

- Created: 2026-08-02
- Completed: 2026-08-18
- Branch: feature/refactor-remove-dead-error-variants
- Polished: {YYYY-MM-DD}

## 目的

コードベースのどこからも構築されないエラー型バリアントを削除し、公開 API 面を実態に合わせる (closed 0009 の「デッドエラーバリアント方針」の継続)。

## 現状

`src/core/error.rs` に、コードベース全体で構築箇所 0 件のバリアントが 5 個ある:

- `Error::MissingInfo` とその中身の型 `ContainerMissingInfo` (関連する `From` 実装と Display も含む)
- `WaitContainerError::StateUnavailable` (状態取得失敗はすべて `ClientError::ContainerNotFound` / `Other` に寄せられている)
- `ClientError::XpcNullReply` (XPC の null 応答は `XpcTimeout` に変換されるため未使用)
- `ExecError::WaitLog` と `From<WaitLogError> for ExecError` (exec の待機失敗は `Error::other` に寄せられている。単体テストでのみ構築)

いずれも `rg` で使用箇所 0 件を確認済み。`docs/TESTCONTAINERS.md` 17 章には「対応」と記載されており、実態 (構築されない) より広い記述になっている。

## 設計方針

バリアント・関連 impl・docs 記載をまとめて削除する。公開 API の削除のため、CHANGES.md に [CHANGE] として記載する。

## 完了条件

- 上記 5 個のバリアントと関連実装が削除され、ビルド・テスト・clippy が通る
- CHANGES.md の develop に [CHANGE] が記載される
- docs/TESTCONTAINERS.md 17 章の該当記載が削除される

## 解決方法

- `src/core/error.rs` から以下を削除した (いずれも構築箇所 0 件を再確認済み):
  - `Error::MissingInfo` バリアントと `ContainerMissingInfo` 型・`From<ContainerMissingInfo> for Error`・Display 実装
  - `WaitContainerError::StateUnavailable` バリアント
  - `ClientError::XpcNullReply` バリアント
  - `ExecError::WaitLog` バリアントと `From<WaitLogError> for ExecError`
- 削除に伴い `src/core/error.rs` 内の単体テスト (`exec_error_wait_log_roundtrip` / `container_missing_info_display_contains_id_and_path`) を削除した
- `docs/TESTCONTAINERS.md` 17 章の該当行とサマリ件数 (対応 287→283 / shiguredo 拡張 24→23) を修正した。17 章の節番号は繰り上げた (`ContainerMissingInfo` 節の削除)
- `skills/shiguredo-container/SKILL.md` のエラー型一覧も同様に修正した (docs と同じ情報のため)
- CHANGES.md への [CHANGE] 追記は行わなかった。2026.1.0 の正式リリース前で変更履歴が未作成のため、リリース時に一括で記載する方針 (メンテナ判断) による
