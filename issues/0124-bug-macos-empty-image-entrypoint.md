# イメージの Entrypoint が [""] のときコンテナが起動できない問題を修正する

- Created: 2026-09-29
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-empty-image-entrypoint
- Polished: {YYYY-MM-DD}

## 目的

Apple container 1.5.0 は、イメージ config の `Entrypoint` が `[""]` のとき「entrypoint クリア」として扱い、指定コマンドまたはイメージ `Cmd` を executable にするよう CLI を修正した (#2296 / apple/container#2248)。Docker も同じ意味論で、マージ後の `Entrypoint` が `[""]` なら entrypoint を無しに戻す (moby `daemon/create.go` の "Reset the Entrypoint if it is [\"\"]")。

本クレートの macOS (XPC) 経路はイメージ config の Entrypoint / Cmd を自前で解決して `initProcess` を組み立てるため、CLI の修正の影響を受けず、同じ穴が残っている。互換性のため同じ意味論に揃える。

## 現状

- `src/core/client/image_config.rs` の `ImageConfig::effective_command` は、イメージ config の entrypoint が `[""]` でもそのまま executable の先頭にする。`Entrypoint: [""]` + `Cmd: ["sh"]` では `exe = ""` / `args = ["sh"]` を返す (一時テストで確認済み)。
- 空 executable は Apple 側で `failed to find target executable` となりコンテナは起動しない (apple/container#2248 と同じ症状)。
- ユーザー指定 `with_entrypoint("")` は `Some(ep) if !ep.is_empty()` の条件で無視され、イメージ entrypoint にフォールバックする。Docker では `Entrypoint: [""]` がクリア扱いになるため挙動が異なる。
- Docker (Linux) 経路は Docker Engine がイメージ既定値を解決するため影響しない。修正対象は macOS 経路。

## 設計方針

- Docker / Apple container と同じく、entrypoint が「単一要素の空文字列」`[""]` のときだけクリアとして扱う。複数要素 (`["", "/bin/sh"]`) は対象外とする (Docker もリセットしない)。
- イメージ config とユーザー指定 (`with_entrypoint("")`) の両方に適用する。
- クリア後の解決規則は既存の分岐のままとする:
  - ユーザー cmd があればその先頭が executable になる。
  - 無ければイメージ cmd の先頭が executable になる。
  - どちらも無ければ既存どおり「no command specified」エラーになる。
- 実装は `effective_command` 内の entrypoint 決定で行い、`parse_image_config` の解析結果は変更しない。

## 完了条件

- `Entrypoint: [""]` + `Cmd: [...]` のイメージで、cmd 無指定時はイメージ `Cmd` の先頭が executable、`with_cmd` 指定時はその先頭が executable になること (単体テスト)。
- `with_entrypoint("")` が entrypoint クリアとして扱われること (単体テスト)。
- `Entrypoint: ["", "/bin/sh"]` のような複数要素はクリア扱いしないこと (回帰テスト)。
- entrypoint / cmd が両方存在しない場合の既存エラーが維持されること (既存テスト)。
- macOS / Linux の既存テストに回帰がないこと。

## 解決方法

どのように対応するのかを明確にすること (例: どのようなコードを追加・修正するのか、どのようなテストを追加するのかなど)
