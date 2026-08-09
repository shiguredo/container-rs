# ドキュメント: TESTCONTAINERS.md の誤情報と件数不一致を修正する

- Created: 2026-08-02
- Completed: 2026-08-09
- Branch: feature/update-testcontainers-md
- Polished: 2026-08-02
- Updated: 2026-08-07

## 目的

精度を標榜する比較ドキュメント `docs/TESTCONTAINERS.md` に、実装と食い違う誤情報と件数の自己矛盾があるのを修正する。

## 現状

- 22 章 (feature ゲート) の `ring` / `aws-lc-rs` / `ssl` の行の「該当なし (shiguredo では rustls を直接使用)」誤記載は、5b1ba69 (2026-08-04) で該当行ごと削除され解消済み (`rg -i rustls docs/TESTCONTAINERS.md` は 0 件)
- `ClientError::XpcTimeout` (src/core/error.rs、src/xpc/conn.rs で実際に構築) は 17.4 の ClientError 対応表に記載済み
- サマリの件数は 5b1ba69 (2026-08-04) で「件数は参考値で、対応表の行数を機械集計したもの」という注記付きに更新され、現在は「shiguredo 拡張 23・内訳合計 393」になっている。列挙は `Xpc*` (ClientError の Xpc 系バリアント 4 個) の集約表記のままで、「何件と数えるか」の揺れは解消されていない
- 残っている誤りは以下:
  - 16.3 節の `Healthcheck::to_docker_json` の備考が「shiguredo 拡張」のまま (集計対象外のはずの内部関数)
  - 「意図的に保持する shiguredo 拡張」節の列挙がサマリの「shiguredo 拡張」列挙と不整合 (with_masked_paths / with_readonly_paths / rm_blocking / ClientError 系 / ContainerRequest accessor / CopyTargetOptions 系が欠落)

## 設計方針

実装 (Cargo.toml の依存・公開 API 面) と突合し、誤情報を削除・修正する。件数は 5b1ba69 の「参考値」方針に従い、固定値への厳密再集計は行わず、「列挙と件数の整合」と「数え方の揺れの解消」のみを確認する。

- `Healthcheck::to_docker_json` は Apple 列が「なし」で元々集計外のため、16.3 節の備考 (「shiguredo 拡張」表記) を「内部関数 (集計外)」に修正する (表記の整合のみで件数は変わらない。「内部型/内部関数 (対象外)」の件数も変更しない)
- `ClientError::Xpc*` の集約表記は件数の数え方を揺れさせるため、個別バリアント (XpcConnect / Xpc / XpcNullReply / XpcTimeout) への展開を検討する。0072 (XpcNullReply 削除) は open のまま未実施のため、現時点では XpcNullReply が残ることを前提にする (0072 実施時は再調整)
- 0065 は完了済み (`ClientError::ContainerPathNotFound` 追加済み。CHANGES.md develop に記載) のため、追記分の加算の考慮は不要
- 「意図的に保持する shiguredo 拡張」節の列挙をサマリ列挙の全項目に整合させる (with_ssh の備考「Docker: 未反映 (Apple 固有)」は、他章の「Docker: start 時に明示エラー」と不整合のため正確化する)

## 完了条件

- 16.3 節の `to_docker_json` の備考が「内部関数 (集計外)」に修正される
- サマリの「shiguredo 拡張」列挙と件数が整合し、「件数は参考値」の注記と矛盾しないこと
- 「意図的に保持する shiguredo 拡張」節の列挙がサマリと整合する

## 解決方法

- 16.3 節の `to_docker_json` の備考を「内部関数 (集計外)」に修正する
- サマリの「shiguredo 拡張」列挙を `ClientError::Xpc*` の集約表記から個別バリアント (XpcConnect / Xpc / XpcNullReply / XpcTimeout) への展開に修正し、件数 (23) と列挙の整合を取る (対応表で 1 行にまとめている accessor は (1 行) と明記する)
- 「意図的に保持する shiguredo 拡張」節の列挙をサマリ列挙の全項目 (with_masked_paths / with_readonly_paths / rm_blocking / ClientError 系 / ContainerRequest accessor / CopyTargetOptions 系) に整合させる。with_ssh の備考を「Docker: 未反映 (Apple 固有)」から「Docker: start 時に明示エラー (設定構築に未配線)」に正確化する
