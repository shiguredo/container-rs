# バグ: Linux archive の 404 をコンテナ内パス不存在と区別せず ContainerNotFound と報告する

- Created: 2026-08-02
- Completed: 2026-08-03
- Branch: feature/fix-linux-archive-404-diagnosis
- Polished: 2026-08-02

## 目的

`copy_file_from` で存在しないコンテナ内パスを指定したときに、コンテナが存在するのに「container not found」と誤報告する診断の誤誘導を解消する。

## 現状

- `src/core/client/docker_client.rs` の `DockerClient::copy_from` は HTTP 404 を一律 `ClientError::ContainerNotFound` に変換する (`copy_to` も同型だが、Linux の `with_copy_to` は投入先が常に `/` のため、パス不存在の 404 は public API 経由で到達不能)
- Docker Engine API の `GET /containers/{id}/archive` は「コンテナ不存在」と「コンテナ内パス不存在」の両方で 404 を返す。moby の実装では、コンテナ不存在は `No such container: <id>`、パス不存在は `Could not find the file <path> in container <id>` という daemon メッセージをボディに含む
- `ContainerAsync::copy_file_from` (Linux 分岐) はパス不存在のケースでも「container not found: <id>」と報告し、利用者はコンテナ ID を疑う方向に誘導される
- macOS 経路 (XPC `containerCopyOut`) は `ContainerNotFound` に変換せず XPC エラーを素通しするため、バックエンド間でも非対称 (パス不存在の具体的なエラーコードは Apple 側の実装依存。本 issue では解消しない)

## 設計方針

404 時にボディを読み、daemon のメッセージで「コンテナ不存在」と「パス不存在」を区別する。404 のボディは `{"message": "..."}` 形式の JSON のため、JSON をパースして `message` フィールドの値を取り出してから判定する。マッチはメッセージの先頭 (`starts_with`) で行う (`contains` はパス名に `No such container:` 等を含む場合に誤分類するため)。区別できない場合 (ボディが空・非 JSON・未知文言) は `ContainerNotFound(id)` のまま返す (パス不存在の誤診断を増やさない安全側の設計)。

パス不存在を表すエラー型として `ClientError` に新 variant `ContainerPathNotFound` を追加する。パスを保持し、Display は `container path not found: {path}` 形式にする (`ContainerNotFound(String)` が ID を保持するのと同じ流儀)。Display への arm 追加と `docs/TESTCONTAINERS.md` の ClientError 対応表更新を伴う。

## 完了条件

- 存在しないコンテナ内パスへの `copy_file_from` が `ContainerNotFound` ではなく `ClientError::ContainerPathNotFound` になる
- 存在しないコンテナへの操作は引き続き `ContainerNotFound` になる (区別できない場合も `ContainerNotFound` のまま)
- `docs/TESTCONTAINERS.md` の ClientError 対応表と「shiguredo 拡張」列挙が更新されている
- `CHANGES.md` に `[FIX]` エントリ (公開 variant 追加は `[ADD]` として併記) が追加されている

## 解決方法

- `src/core/error.rs` の `ClientError` に `ContainerPathNotFound(String)` variant を追加する (パスを保持し、Display は `container path not found: {path}` 形式)
- `src/core/client/docker_client.rs` の `DockerClient::copy_from` の 404 処理を、既に蓄積済みのレスポンスボディを nojson で JSON パースして `message` フィールドの値で区別するように変更する。`Could not find the file ` で始まる場合は `ContainerPathNotFound` (リクエスト引数の `path` を保持)、それ以外 (コンテナ不存在・未知文言・空ボディ・非 JSON) は `ContainerNotFound` のまま (安全側の設計)
- 判定ロジックは純粋関数 `classify_archive_404` / `parse_daemon_error_message` に分離し、単体テスト 6 本で検証する (パス不存在・コンテナ不存在・フォールバック 4 種・prefix 一致・非文字列/非オブジェクト message・正常 message 抽出)
- `copy_to` の 404 処理は変更しない (Linux の `with_copy_to` は投入先が常に `/` のため、パス不存在 404 は到達不能)。他の 404 → `ContainerNotFound` 変換 (inspect 系) もコンテナ不存在のみが原因のため変更しない
- `tests/container_linux.rs` に `copy_file_from_nonexistent_path_is_path_not_found` 統合テストを追加する (実 Docker でパス不存在 404 が `ContainerPathNotFound` になることを検証)
- `ContainerAsync::copy_file_from` の rustdoc に 404 の区別 (Linux) を明記する
- `docs/TESTCONTAINERS.md` の ClientError 対応表 (`ContainerPathNotFound` 行・`XpcTimeout` 行追加)・shiguredo 拡張の件数 (21)・copy_file_from の説明を更新する
- `CHANGES.md` に `[CHANGE]` (公開 variant 追加) と `[FIX]` (誤報告の修正) の 2 エントリを追加する (rustdoc 更新分は misc `[UPDATE]` として併記)
