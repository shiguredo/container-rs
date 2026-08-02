# バグ: Linux archive の 404 をコンテナ内パス不存在と区別せず ContainerNotFound と報告する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-linux-archive-404-diagnosis
- Polished: {YYYY-MM-DD}

## 目的

`copy_file_from` / `with_copy_to` で存在しないコンテナ内パスを指定したときに、コンテナが存在するのに「container not found」と誤報告する診断の誤誘導を解消する。

## 現状

- `src/core/client/docker_client.rs` の `DockerClient::copy_from` / `copy_to` は HTTP 404 を一律 `ClientError::ContainerNotFound` に変換する
- Docker Engine API の `/containers/{id}/archive` は「コンテナ不存在」と「コンテナ内パス不存在」の両方で 404 を返す。404 のレスポンスボディには daemon の詳細メッセージが含まれる
- `ContainerAsync::copy_file_from` (Linux 分岐) はパス不存在のケースでも「container not found: <id>」と報告し、利用者はコンテナ ID を疑う方向に誘導される
- macOS 経路 (XPC `containerCopyOut`) はパス不存在を別エラーで報告するため、バックエンド間でも非対称

## 設計方針

404 時にレスポンスボディを読み、daemon のメッセージを確認して「コンテナ不存在」と「パス不存在」を区別する。区別できない場合はボディのメッセージをエラーに含めて診断性を上げる。

## 完了条件

- 存在しないコンテナ内パスへの `copy_file_from` が `ContainerNotFound` ではなくパス不存在を表すエラーになる
- 存在しないコンテナへの操作は引き続き `ContainerNotFound` になる

## 解決方法

- `DockerClient::copy_from` / `copy_to` の 404 処理でボディを読み、メッセージ内容で区別する (またはメッセージを `ClientError::Other` に含める)
- `tests/container_linux.rs` に「パス不存在」のテストを追加する (既存テストはコンテナ不存在のみ)
