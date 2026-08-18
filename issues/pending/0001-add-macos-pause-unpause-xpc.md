# 保留: ContainerAsync / Container の pause / unpause は XPC に route が無い

- Priority: Low
- Created: 2026-07-18
- Completed:
- Model: Grok 4.5
- Branch: feature/add-macos-pause-unpause
- Polished:

## 目的

本家にある `pause` / `unpause` を macOS で提供したいが、Apple container の XPCRoute に pause 系が無く実装できない。Apple 側に route が追加されるまで保留する。

## 優先度根拠

本家互換の欠落だが、Apple 側仕様待ちであり自前では解けない。過去に stub せず差分明記する方針を採っている。再検討は XPC 追加後のため Low。

## 現状

- XPCRoute に `containerPause` / `containerUnpause` 相当が無い
- 本クレートに `pause` / `unpause` メソッドは無い (`docs/TESTCONTAINERS.md` で未実装 (XPC 制約))

## 設計方針

- Apple container が pause 系 XPC を公開したら、本家互換シグネチャで実装する
- それまではシグネチャを追加して常にエラーを返す stub にはしない (差分明記方針を維持)

## 完了条件

- [ ] Apple 側に pause 系 API が追加されたことを確認できること
- [ ] 実装方針を再 open し、実装 issue に切り出せること

## pending にする理由

XPCRoute に pause 系が存在せず、本クレート単体では実装不能なため。Apple container の仕様追加を待つ。

## 解決方法

未着手 (pending)。
