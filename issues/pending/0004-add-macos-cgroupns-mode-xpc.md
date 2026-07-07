# 保留: ImageExt::with_cgroupns_mode は XPC に該当項目が無い

- Priority: Low
- Created: 2026-07-18
- Completed:
- Model: Grok 4.5
- Branch: feature/add-image-ext-with-cgroupns-mode
- Polished:

## 目的

本家の `ImageExt::with_cgroupns_mode` を追加したいが、XPC に該当項目が無い。仕様追加まで保留する。

## 優先度根拠

`docs/TESTCONTAINERS.md` で「XPC には該当項目なし」。Linux 固有の cgroup namespace 操作であり Apple container VM モデルとは噛み合わない。Low。

## 現状

- `with_cgroupns_mode` / `CgroupnsMode` accessor が無い (または型だけ孤立している場合は削除方針の別 issue と隣接)
- XPC `ContainerCfg` に cgroupns 相当が無い

## 設計方針

- Apple が同等の設定口を公開したら本家互換で実装する
- 無い間はシグネチャだけ追加して無視しない (誤解を招く)

## 完了条件

- [ ] Apple 側に cgroupns 相当が追加されたことを確認できること
- [ ] 実装方針を再 open し、実装 issue に切り出せること

## pending にする理由

XPC に cgroupns 相当の設定が無く、実装不能なため。

## 解決方法

未着手 (pending)。
