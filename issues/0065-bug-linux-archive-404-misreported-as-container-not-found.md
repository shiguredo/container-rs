# バグ: Linux archive の 404 をコンテナ内パス不存在と区別せず ContainerNotFound と報告する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
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

- `DockerClient::copy_from` の 404 処理で、既に蓄積済みのレスポンスボディ (`body_bytes()`) を nojson で JSON パースして `message` フィールドを取り出し、その値で区別する (`read_http11_response` は全ステータスでボディを蓄積済みのため、追加の I/O は不要)
- `message` が `Could not find the file ` で始まる場合は `ClientError::ContainerPathNotFound` (リクエスト引数の `path` をそのまま保持する。daemon メッセージからの切り出しは文言変更で壊れるため行わない) を返す。`No such container: ` で始まる場合は `ContainerNotFound` のまま。区別できない場合は `ContainerNotFound(id)` のまま返す
- 判定ロジック (JSON の `message` 抽出 + `starts_with` 分類 + フォールバック) は純粋関数に分離し、単体テストで検証する (統合テストでは未知文言・空ボディのフォールバックを再現できないため)
- `copy_to` の 404 処理は変更しない (Linux の `with_copy_to` は投入先が常に `/` のため、パス不存在 404 は到達不能。`copy_to` の 404 は実質コンテナ不存在のみ)
- 他の 404 → `ContainerNotFound` 変換 (inspect 系) はコンテナ不存在のみが原因のため変更しない。既存の「404 は `ContainerNotFound` に寄せる」コメントは本変更で古くなるため更新する
- `tests/container_linux.rs` に「パス不存在」の統合テストを追加する (既存の `copy_file_from_nonexistent_container_is_not_found` はコンテナ不存在のみ)。パス不存在のエラー型 (`ContainerPathNotFound`) を assert する
- `docker_client.rs` と `src/core/error.rs`・`docs/TESTCONTAINERS.md` を変更するため、同じ `DockerClient::copy_from` を変更する 0064、同じ `error.rs` を変更する 0072、同じ `docs/TESTCONTAINERS.md` を変更する 0073 とマージ順に注意する
- `CHANGES.md` に `[FIX]` エントリ (公開 variant 追加は `[ADD]` として併記) を追加する
