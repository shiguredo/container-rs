# バグ: イメージ pull 後の再 GET 失敗が ImageNotFound と誤分類される

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-image-not-found-misclassification
- Polished: {YYYY-MM-DD}

## 目的

`resolve_image_descriptor` が pull 成功後の再 GET の失敗 (daemon 異常等の 4xx / 5xx) を `ImageNotFound` と誤分類し、存在するイメージの起動が「存在しない」と誤診断されるのをなくす。

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

- pull 成功直後の再 GET が 500 (daemon 異常) でも `ImageNotFound` になる
- 呼び出し側 (`async_runner.rs` の `resolve_or_pull_linux`) は `ImageNotFound` に対してさらに pull を試みるため、「存在するのに pull → 誤分類 → 再 pull → registry に無いので pull 失敗」の誤ったエラーで start が失敗する
- 初回 GET の非 404 エラーは `Other("failed to resolve image ...")` と正しく分類されており、再 GET だけが誤っている
- 注: pull 自体を 2 重に試みる構造 (二重 pull) は `issues/0096-refactor-merge-duplicate-implementations.md` で対応予定のため、本 issue は誤分類の修正のみを対象とする

## 設計方針

- 再 GET の非 200 を `ImageNotFound` にせず、初回 GET と同じ分類 (404 → `ImageNotFound`・それ以外 → `Other`) に揃える
- この修正により 0096 の「404 限定 pull」化が正しく機能するようになる

## 完了条件

- pull 成功後の再 GET が 404 の場合のみ `ImageNotFound` が返ること
- 再 GET が 500 等の場合は `Other` エラーが返ること
- 既存のイメージ解決・pull テストが従来どおり通ること
