# ドキュメント: TESTCONTAINERS.md の誤情報と件数不一致を修正する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-testcontainers-md-inaccuracies
- Polished: {YYYY-MM-DD}

## 目的

精度を標榜する比較ドキュメント `docs/TESTCONTAINERS.md` に、実装と食い違う誤情報と件数の自己矛盾があるのを修正する。

## 現状

- 22 章 (feature ゲート) の `ring` / `aws-lc-rs` / `ssl` の行に「該当なし (shiguredo では rustls を直接使用)」とあるが、`Cargo.toml` の依存 (base64ct / libc / nojson / shiguredo_http11 / tokio / tracing) に rustls は含まれない。`http_wait_plain` は plain HTTP のみ (TLS 非対応) で、TLS バックエンドは一切使わない (`cargo tree` で TLS 系クレート 0 件)
- サマリの「shiguredo 拡張 (20)」と列挙が一致しない。列挙 (with_init / with_ssh / with_masked_paths / with_readonly_paths / container_state ×2 / ClientError 系 / ContainerRequest accessor ×4 / CopyTargetOptions ×4) は数え方で 19〜21 件に揺れ、`rm_blocking` は別節 (8 章・7 章) で shiguredo 拡張と明記されているのに列挙に無い

## 設計方針

実装 (Cargo.toml の依存・公開 API 面) と突合し、誤情報を削除・修正して件数を再集計する。

## 完了条件

- rustls に関する誤記載が無くなる
- 「shiguredo 拡張」の件数と列挙が一致する

## 解決方法

- 22 章の備考を「TLS バックエンドを使用しないため該当なし」に修正する
- サマリの列挙に `rm_blocking` を追加し (必要なら `Healthcheck::to_docker_json` / `ClientError::XpcTimeout` も確認)、件数を再集計する
