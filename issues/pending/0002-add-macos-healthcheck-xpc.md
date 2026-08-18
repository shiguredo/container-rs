# 保留: HealthWaitStrategy / Healthcheck / with_health_check は Apple container に口が無い

- Priority: Low
- Created: 2026-07-18
- Completed:
- Model: Grok 4.5
- Branch: feature/add-macos-healthcheck
- Polished:

## 目的

本家の `HealthWaitStrategy`、`Healthcheck` 型、`ImageExt::with_health_check` を macOS で提供したいが、Apple container は Docker HEALTHCHECK 相当を実行・公開しない。仕様追加まで保留する。

## 優先度根拠

本家互換の欠落だが XPC / ContainerConfiguration に healthcheck の口が無い。`HealthWaitStrategy` は常に `HealthCheckNotConfigured` で、過去に差分明記方針を採っている。Low。

## 現状

- `HealthWaitStrategy::wait_until_ready` は OS 非依存で常にエラー
- `Healthcheck` 型と `with_health_check` は本クレートに無い
- Apple `ContainerConfiguration` に healthcheck フィールドが無い

## 設計方針

- Apple が HEALTHCHECK 相当の実行結果や設定口を公開したら、Wait と ImageExt を本家互換で実装する
- それまでは偽の healthy を返さない

## 完了条件

- [ ] Apple 側に healthcheck 相当が追加されたことを確認できること
- [ ] 実装方針を再 open し、実装 issue に切り出せること

## pending にする理由

Apple container が Docker HEALTHCHECK 相当を持たず、XPC からも観測・設定できないため。

## 解決方法

未着手 (pending)。
