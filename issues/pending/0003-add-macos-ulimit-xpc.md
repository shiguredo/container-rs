# 保留: ImageExt::with_ulimit は XPC にコンテナ全体 ulimit が無い

- Priority: Low
- Created: 2026-07-18
- Completed:
- Model: Grok 4.5
- Branch: feature/add-image-ext-with-ulimit
- Polished:

## 目的

本家の `ImageExt::with_ulimit` を追加したいが、XPC `ContainerCfg` にはプロセス単位の rlimits はある一方、コンテナ全体の ulimit API が無い。仕様追加まで保留する。

## 優先度根拠

`docs/TESTCONTAINERS.md` で「なし」「XPC には ulimit 全体は無い」。自前で擬似実装すると本家と意味がずれるため保留。Low。

## 現状

- `with_ulimit` は本クレートに無い
- プロセスごとの rlimits は別経路に存在するが、本家の HostConfig ulimit とは対応関係が不明確

## 設計方針

- Apple がコンテナ全体の ulimit 設定口を公開したら本家互換で実装する
- プロセス rlimits への勝手な読み替えはしない

## 完了条件

- [ ] Apple 側に ulimit 相当の設定口が追加されたことを確認できること
- [ ] 実装方針を再 open し、実装 issue に切り出せること

## pending にする理由

XPC に本家 `with_ulimit` 相当のコンテナ全体設定が無く、実装不能なため。

## 解決方法

未着手 (pending)。
