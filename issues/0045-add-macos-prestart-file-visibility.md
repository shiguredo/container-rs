# 機能追加: macOS で初期プロセス起動前にファイルを見せられるようにする

- Priority: Medium
- Created: 2026-07-23
- Completed: {YYYY-MM-DD}
- Model: Cursor Grok 4.5
- Branch: feature/add-macos-prestart-file-visibility
- Polished: {YYYY-MM-DD}
- Reporter: @voluntas

## 目的

macOS (Apple container) でも、初期プロセスが起動時に読む設定・証明書などを、利用側の起動待ちシェル無しで投入できるようにする。

Linux は `with_copy_to` を create 後・start 前に完了させる公開契約になった。macOS は `containerCopyIn` が running 必須のため、同契約に揃えられなかった。OS 非対称を残したまま、macOS 側の痛点（mqtt-rs 移行時の起動待ちシェル・レース依存）を解消する手段を提供する。

## 優先度根拠

利用者フィードバック (mqtt-rs の移行)。Linux だけ起動前投入が使えると、同一テストコードを両 OS で回せない。Medium（Linux は既に解消済みのため High にはしない）。

## 現状

- macOS の `AsyncRunner::start` は `start_process` 後に `copy_to_sources`（XPC `containerCopyIn`）を呼ぶ
- bootstrap 後・`start_process` 前に `containerCopyIn` を呼ぶと
  `XPC error invalidState: container ... is not running` で失敗する（実測済み）
- そのため macOS は「start 後コピー・レースあり」のまま文書化されている
- 手段（マウント・別 API・起動待ちヘルパー等）は未確定。本 issue で実測と設計を行い、公開契約に載せる

## 設計方針

- 手段は本 issue の調査・実測で決める。候補を事前に固定しない
- Linux の create → copy → start 契約を壊さない
- OS 非対称を隠した単一契約は謳わない。macOS 向けに使える経路ができたら、その範囲を文書と rustdoc に明示する
- 公開 API の追加が必要ならシグネチャと完了条件を設計方針確定後に具体化する

## 完了条件

- macOS で、初期プロセスが起動時に読むファイルを、利用側の起動待ちシェル無しで投入できる経路がある（公開 API または文書化された利用手順）
- 経路の制約・非対応ケースが `docs/TESTCONTAINERS.md` / rustdoc / README に書かれている
- 回帰を防ぐ統合テストが `tests/container_macos.rs` にある（CI skip 慣例に従う）
- `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass する

## 解決方法

（実装時に過去形で書き直す）
