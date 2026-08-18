# 保留: ImageExt::with_security_opt は XPC に該当項目が無い

- Priority: Low
- Created: 2026-07-18
- Completed:
- Model: Grok 4.5
- Branch: feature/add-image-ext-with-security-opt
- Polished:

## 目的

本家の `ImageExt::with_security_opt` を追加したいが、XPC に該当項目が無い。仕様追加まで保留する。

## 優先度根拠

`docs/TESTCONTAINERS.md` で「XPC には該当項目なし」。seccomp / apparmor 系の Docker オプションであり Apple container に対応口が無い。Low。

## 現状

- `with_security_opt` / `security_opts` accessor が無い
- XPC `ContainerCfg` に security opt 相当が無い

## 設計方針

- Apple が同等の設定口を公開したら本家互換で実装する

## 完了条件

- [ ] Apple 側に security opt 相当が追加されたことを確認できること
- [ ] 実装方針を再 open し、実装 issue に切り出せること

## pending にする理由

XPC に security opt 相当の設定が無く、実装不能なため。

## 解決方法

未着手 (pending)。
