# リファクタリング: 未使用のエラー型バリアントを削除する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
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

- `src/core/error.rs` から 5 個のバリアント・型・`From` 実装・Display を削除する
- 削除に伴う単体テスト (`error.rs` 内の roundtrip テスト) を整理する
- CHANGES.md に [CHANGE] を追記し、docs/TESTCONTAINERS.md 17 章を修正する
