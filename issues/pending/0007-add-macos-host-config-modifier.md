# 保留: ImageExt::with_host_config_modifier は bollard 型依存で XPC に同等が無い

- Priority: Low
- Created: 2026-07-18
- Completed:
- Model: Grok 4.5
- Branch: feature/add-image-ext-with-host-config-modifier
- Polished:

## 目的

本家の `with_host_config_modifier` は bollard の HostConfig を直接いじる API である。本クレートは bollard を依存に持たず、XPC にも同等の包括的 modifier が無いため保留する。

## 優先度根拠

`docs/TESTCONTAINERS.md` で「bollard の型を引数に取るため未対応」。自前の部分的な ImageExt で足りる範囲は個別 issue で追加する方針であり、包括 modifier 自体は導入しない。Apple が汎用 config hook を出さない限り Low のまま保留。

## 現状

- `with_host_config_modifier` が無い
- bollard を依存に含めない方針
- 個別設定は `with_cap_add` 等の ImageExt で部分的にカバー

## 設計方針

- bollard 型を公開 API に持ち込まない
- 不足する個別設定は、XPC に口があるものだけ別の add issue で追加する
- 包括 modifier が必要になったら、自前の型で再設計する (そのときは本 pending を再 open して設計 issue に切り出す)

## 完了条件

- [ ] 自前の包括 modifier が必要だと判断できること、または Apple 側に同等口が追加されたこと
- [ ] 設計を再 open し、実装 / 設計 issue に切り出せること

## pending にする理由

本家 API が bollard HostConfig 前提であり、本クレートの XPC 自前クライアント方針と両立しないため。

## 解決方法

未着手 (pending)。
