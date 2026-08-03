# ドキュメント: TESTCONTAINERS.md の誤情報と件数不一致を修正する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/update-testcontainers-md-inaccuracies
- Polished: 2026-08-02

## 目的

精度を標榜する比較ドキュメント `docs/TESTCONTAINERS.md` に、実装と食い違う誤情報と件数の自己矛盾があるのを修正する。

## 現状

- 22 章 (feature ゲート) の `ring` / `aws-lc-rs` / `ssl` の行に「該当なし (shiguredo では rustls を直接使用)」とある (docs 790-792 行) が、`Cargo.toml` の依存に rustls は含まれず、`http_wait_plain` は plain HTTP のみ (TLS 非対応)。`cargo tree` でも TLS 系クレート 0 件
- サマリの「shiguredo 拡張 (20)」は誤り。Apple 列の判定セルが「shiguredo 拡張」の表の行を数えると 21 行 (docs 156-159 / 202 / 206 / 247 / 253 / 295-296 / 508-509 / 512-513 / 695-701) あり、`rm_blocking` (6.1 節・7 章) が列挙 (docs 89 行) に無く、`ClientError::Xpc*` のグループ表記で件数が揺れる
- `ClientError::XpcTimeout` (src/core/error.rs、src/xpc/conn.rs で実際に構築) は 17.4 の ClientError 対応表に記載が無い

## 設計方針

実装 (Cargo.toml の依存・公開 API 面) と突合し、誤情報を削除・修正して件数を再集計する。件数の数え方は「Apple 列の判定セルが『shiguredo 拡張』の表の行を単位に数える」とする (docs 80 行の「feature ゲート表の『備考』列も集計外」と同じ流儀。8 章の `init/ssh` accessor と `masked_paths/readonly_paths` accessor はそれぞれ 1 行として数え、計 2 件)。修正後の期待値は「shiguredo 拡張 22 件・内訳合計 415」とする (内訳: 413 + 1 (数え方修正: 拡張 20 → 21) + 1 (XpcTimeout 追記) = 415。拡張以外の内訳 (対応 274 / 部分対応 22 / 未実装 0 / XPC 制約 3 / なし 94) は不変)。

- `Healthcheck::to_docker_json` は Apple 列が「なし」で元々集計外のため、16.3 節の備考 (「shiguredo 拡張」表記) を「内部関数 (集計外)」に修正する (表記の整合のみで件数は変わらない。「内部型/内部関数 (対象外)」の件数も変更しない)
- `ClientError::XpcTimeout` は 17.4 の対応表に追記し、集計に含める (確定事項。備考は「XPC 応答なし (タイムアウト) / Docker: XPC 固有」)
- 列挙 (docs 89 行) は `rm_blocking` を追加し、`ClientError::Xpc*` を個別バリアント (XpcConnect / Xpc / XpcNullReply / XpcTimeout) に展開し、accessor の表記を 2 行に合わせて、件数 (22) と一致させる (「何件と数えるか」の揺れを再発させない)
- 集計の基準時点は現状の docs とする。0072 (XpcNullReply 削除予定) は実施しない方針 (スキップ) のため、XpcNullReply は残ることを前提に集計する。0065 (open) が `ClientError::ContainerPathNotFound` を追加予定のため、0065 とマージ順に注意する (0065 が先にマージされた場合はその分を加算する)
- 「意図的に保持する shiguredo 拡張」節 (docs 855 行) の列挙をサマリ列挙の全項目に整合させる (with_ssh の備考「Docker: 未反映 (Apple 固有)」は、他章の「Docker: start 時に明示エラー」と不整合のため正確化する)

## 完了条件

- rustls に関する誤記載が無くなる (検証: `rg -i rustls docs/TESTCONTAINERS.md` で 0 件)
- 「shiguredo 拡張」の件数 (22) と列挙が一致し、「内訳合計: 413 API」の行が 415 に再計算される
- `ClientError::XpcTimeout` が 17.4 の対応表に記載される
- 16.3 節の `to_docker_json` の備考が「内部関数 (集計外)」に修正される
- 「意図的に保持する shiguredo 拡張」節の列挙がサマリと整合する

## 解決方法

- 22 章の備考を「該当なし (TLS バックエンドを使用しない。plain HTTP のみ)」に修正する (3 行すべて。他行の表記形式に揃える)
- サマリの列挙に `rm_blocking` (6.1 節・7 章の 2 行) を追加し、`ClientError::Xpc*` を個別バリアントに展開し、accessor の表記を 2 行に合わせて、件数 (22) と一致させる。「内訳合計: 413 API」の行も 415 に再計算する
- 17.4 の ClientError 対応表に `XpcTimeout` の行を追加する
- 16.3 節の `to_docker_json` の備考を「内部関数 (集計外)」に修正する
- 「意図的に保持する shiguredo 拡張」節の列挙をサマリ列挙の全項目に整合させる (with_ssh の備考も正確化する)
- 0065 (open) が同じ `docs/TESTCONTAINERS.md` のサマリ列挙を更新するため、マージ順に注意する
