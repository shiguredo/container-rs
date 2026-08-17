# バグ: イメージ pull 後の再 GET 失敗が ImageNotFound と誤分類される

- Created: 2026-08-12
- Completed: 2026-08-14
- Branch: feature/fix-image-not-found-misclassification
- Polished: 2026-08-12

## 目的

`resolve_image_descriptor` が pull 成功後の再 GET の失敗 (daemon 異常等の 404 以外の 4xx / 5xx) を `ImageNotFound` と誤分類し、存在するイメージの起動が「存在しない」と誤診断されるのをなくす。

## 現状

`src/core/client/docker_client.rs` の `resolve_image_descriptor` は、初回 GET が 404 のとき pull して再 GET するが、再 GET の結果が 200 以外なら**すべて** `ImageNotFound` を返す。

```rust
} else if response.status_code() == 404 {
    self.pull_image(descriptor, platform).await?;
    let response = self.request("GET", &path, None).await?;
    if response.status_code() == 200 {
        Ok(descriptor.to_string())
    } else {
        Err(ClientError::ImageNotFound(descriptor.to_string()).into())
    }
}
```

- 呼び出し側 (`async_runner.rs` の `resolve_or_pull_linux`) は `Err(_)` で**あらゆるエラー**に対して pull を再試行するため、誤分類された `ImageNotFound` が不要な pull を誘発し、daemon 異常が継続している場合は「存在するのに pull → 誤分類 → 再 pull → pull 失敗」の誤ったエラーで start が失敗する
- 初回 GET の非 404 エラーは `Other("failed to resolve image ...")` と正しく分類されており、再 GET だけが誤っている
- 注: pull 自体を 2 重に試みる構造 (二重 pull) は `issues/0096-refactor-merge-duplicate-implementations.md` で対応予定のため、本 issue は誤分類の修正のみを対象とする。0096 の「`ImageNotFound` のみ pull」化は本修正 (再 GET 500 → `Other`) を前提とするため、**0096 は本 issue の完了後に実装すること**

## 設計方針

- 再 GET の非 200 を `ImageNotFound` にせず、初回 GET と同じ**エラー分類** (404 → `ImageNotFound`・それ以外 → `Other("failed to resolve image {descriptor}: {status}")`) に揃える (再 GET は pull しない。pull は初回 GET の 404 分岐のみ。対象は Linux の `DockerClient` のみ。macOS 側の `XpcClient::resolve_image_descriptor` は pull 後の再 GET 構造を持たず誤分類が無い)

## 完了条件

- pull 成功後の再 GET が 404 の場合のみ `ImageNotFound` が返ること (0096 の macOS 同等化後は初回 GET の 404 のみが pull の合図になり、再 GET の 404 → `ImageNotFound` は pull を誘発せず最終エラーとして伝播する。検証はコードレビューで担保する)
- 再 GET が 500 等の場合は `Other` エラーが返ること (実 daemon で 500 を再現する手段がなくモック・スタブ禁止のため、分岐の検証はコードレビューで担保する)
- 既存のイメージ解決・pull テストが従来どおり通ること
- `CHANGES.md` に `[FIX]` エントリが記載されること

## 解決方法

`src/core/client/docker_client.rs` の `DockerClient::resolve_image_descriptor` を修正した。

- pull 成功後の再 GET の非 200 をすべて `ImageNotFound` にしていたのを修正し、初回 GET と同じ分類 (404 → `ImageNotFound`・それ以外 → `Other("failed to resolve image {descriptor}: {status}")`) に揃えた
- pull の実行は初回 GET の 404 分岐のみ (再 GET では pull しない)。pull 成功後に存在しない場合 (pull と GET の間で消えた等) の再 GET 404 は `ImageNotFound` として最終エラーになる (呼び出し元が高々 1 回の再試行を行うため無限ループにはならない)
- 分岐の検証は実 daemon で 500 を再現する手段がなくコードレビューで担保した (issue の指示どおり)。既存のイメージ解決・pull テストは従来どおり通過
- `CHANGES.md` の `## develop` に `[FIX]` エントリを追記した
