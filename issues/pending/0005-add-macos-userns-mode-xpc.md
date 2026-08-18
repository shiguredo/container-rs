# 保留: ImageExt::with_userns_mode は XPC に該当項目が無い

- Priority: Low
- Created: 2026-07-18
- Completed:
- Model: Grok 4.5
- Branch: feature/add-image-ext-with-userns-mode
- Polished:

## 目的

本家の `ImageExt::with_userns_mode` を追加したいが、XPC に該当項目が無い。仕様追加まで保留する。

## 優先度根拠

`docs/TESTCONTAINERS.md` で「XPC には該当項目なし」。user namespace は Linux Docker 寄りの設定で、Apple container に対応口が無い。Low。

## 現状

- `with_userns_mode` が本クレートに無い
- XPC `ContainerCfg` に userns 相当が無い

## 設計方針

- Apple が同等の設定口を公開したら本家互換で実装する

## 完了条件

- [ ] Apple 側に userns 相当が追加されたことを確認できること
- [ ] 実装方針を再 open し、実装 issue に切り出せること

## pending にする理由

XPC に userns 相当の設定が無く、実装不能なため。

## 解決方法

未着手 (pending)。
