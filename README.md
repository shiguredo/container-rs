# shiguredo_container

[![crates.io](https://img.shields.io/crates/v/shiguredo_container.svg)](https://crates.io/crates/shiguredo_container)
[![docs.rs](https://docs.rs/shiguredo_container/badge.svg)](https://docs.rs/shiguredo_container)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![GitHub Actions](https://github.com/shiguredo/container-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/shiguredo/container-rs/actions/workflows/ci.yml)
[![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?logo=discord&logoColor=white)](https://discord.gg/shiguredo)

## About Shiguredo's open source software

We will not respond to PRs or issues that have not been discussed on Discord. Also, Discord is only available in Japanese.

Please read <https://github.com/shiguredo/oss> before use.

## 時雨堂のオープンソースソフトウェアについて

利用前に <https://github.com/shiguredo/oss> をお読みください。

## 概要

Apple の [container](https://github.com/apple/container) 対応をメインとする Rust 用テストコンテナライブラリです。

- **macOS**: container の XPC API を叩く実装を自前で持ちます。Docker Desktop は不要です。本ライブラリのメイン対象です。対応範囲は [testcontainers-rs / Apple Container / Docker Engine API の比較](docs/TESTCONTAINERS.md) を参照してください。
- **Linux**: Docker Engine API を利用します。

> [!WARNING]
> Linux ではライフサイクル (start / exec / stop / rm / Drop) に加えログ関連 (stdout / stderr / ログ待機 / LogConsumer) とファイルコピー (`copy_file_from` / `with_copy_to`) とヘルスチェック待機 (`with_health_check` / `WaitFor::healthcheck`) と exec の stdout / stderr 取得と bridge IP 取得も動くが、exec の env・ネットワーク系設定などは未対応のままである。対応範囲は [testcontainers-rs / Apple Container / Docker Engine API の比較](docs/TESTCONTAINERS.md) を参照してください。
>
> `with_copy_to` の起動前投入は Linux のみ。macOS は start 後コピーのため、初期プロセスが起動時に読むファイルには利用側の起動待ち等が別途必要になり得る。

## モチベーション

[testcontainers-rs](https://github.com/testcontainers/testcontainers-rs) はコンテナを利用したテストを簡単に書ける素晴らしいライブラリです。ただ、testcontainers は [Docker](https://www.docker.com/) 社が開発しており、同社の製品ではない Apple の [container](https://github.com/apple/container) に対応する可能性は低いと考えています。そこで、Apple の container を利用したテスト向けのコンテナライブラリとして本ライブラリを開発しています。

公開 API は、利用者の学習コストを減らすためできるだけ testcontainers-rs に寄せています。ただし、完全な互換は目指しません。

また、本ライブラリは依存クレートをできるだけ少なくする方針で実装しています。

## 要件

| OS | ランタイム | 備考 |
|:--|:--|:--|
| macOS 26 (Apple Silicon) | `container` (`brew install container`) | macOS 26 必須 |
| Linux | Docker Engine (Docker Engine API 互換) | Podman 等 API 互換ランタイムも可 |

## インストール

```toml
[dependencies]
shiguredo_container = { version = "2026", features = ["blocking", "http_wait_plain"] }
tokio = { version = "1.53", features = ["full"] }
```

## クイックスタート

`GenericImage` を `AsyncRunner::start` で起動し、`ContainerAsync` を取得します。nginx を `WaitFor::http` で待ち、公開ポートへ素の HTTP/1.1 で GET する例です。

```rust
use std::time::Duration;

use shiguredo_container::{
    AsyncRunner, GenericImage, ImageExt, WaitFor,
    core::{IntoContainerPort, wait::HttpWaitStrategy},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn test_with_nginx() {
    let container = GenericImage::new("nginx", "latest")
        .with_exposed_port(80.tcp())
        .with_wait_for(WaitFor::http(
            HttpWaitStrategy::new("/")
                .with_port(80.tcp())
                .with_expected_status_code(200_u16),
        ))
        .with_startup_timeout(Duration::from_secs(120))
        .start()
        .await
        .expect("failed to start nginx");

    let host_port = container
        .get_host_port_ipv4(80.tcp())
        .await
        .expect("failed to resolve published host port");

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", host_port))
        .await
        .expect("failed to connect");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .expect("failed to write request");
    let mut body = Vec::new();
    stream
        .read_to_end(&mut body)
        .await
        .expect("failed to read response");
    let text = String::from_utf8_lossy(&body);
    assert!(text.starts_with("HTTP/1.1 200"));
    assert!(text.to_ascii_lowercase().contains("nginx"));

    container.stop().await.expect("failed to stop");
    container.rm().await.expect("failed to remove");
}
```

macOS では `container system start` 済みであること、および Local Network Privacy でコンテナへの接続が許可されていることが必要です。

### ブロッキング API

`blocking` feature を有効にすると `SyncRunner` が使えます。`#[tokio::test]` ではなく通常の `#[test]` で書けます。

```rust
use shiguredo_container::{
    GenericImage, ImageExt, SyncRunner, WaitFor,
};

#[test]
fn test_with_nginx_blocking() {
    let container = GenericImage::new("nginx", "latest")
        .with_wait_for(WaitFor::message_on_either_std("start worker processes"))
        .with_startup_timeout(std::time::Duration::from_secs(180))
        .start()
        .expect("failed to start nginx");

    let id = container.id().to_string();
    println!("nginx container id: {id}");

    container.stop().expect("failed to stop");
    container.rm().expect("failed to remove");
}
```

#### 制約

`blocking` feature 使用時、`LogConsumer` コールバック内や既存の tokio ランタイムコンテキスト内から `SyncRunner::start` などの同期 API を呼び出すと、共有 Runtime への再入によって deadlock します。ライブラリ側は再入を検出して即座にエラーにしますが、コールバック内での同期 API 呼び出しは避けてください。

### コンテナの掃除契約

- 既定 (`TESTCONTAINERS_COMMAND` 未設定または `remove`) では `ContainerAsync` / `Container` の Drop 時にコンテナを削除する。削除経路は Drop が Runtime 内か外かで異なる
  - Runtime 内 Drop (`Handle::try_current()` が `Ok`): 削除を専用 std スレッドで実行し、5 秒 (`DROP_REMOVE_TIMEOUT`) を上限に完了を待つ。timeout 内に完了すれば `drop` 復帰時点で削除は終わっている。超過時は best-effort (削除スレッドは裏で走り続けるが、`drop` 直後にプロセスが終了すると中断され得る)
  - Runtime 外 Drop: 呼び出しスレッドで削除試行が終わるまで待つ (成功は保証しない。失敗は `tracing::error` に記録するのみで呼び出し側には届かない)
- 削除の完了待ち、または成否の `Result` が必要なら明示 `rm()` を使う (async は `rm().await`、sync は `rm()`)。同期コンテキスト (Runtime 内の Drop ガードや `spawn_blocking` 内) から削除完了を待ちたい場合は `rm_blocking()` を使う (`block_on` を使わないため Runtime 内から呼んでも deadlock しない)。明示 `rm()` / `rm_blocking()` は `TESTCONTAINERS_COMMAND=keep` でも削除する (Drop の `keep` ゲートとは非対称)
- `TESTCONTAINERS_COMMAND=keep` のときは Drop で削除しない (調査用に残す)
- `stop()` は LogConsumer 配信を止める。明示的な `rm()` を呼ばなくても Drop で削除される (`keep` 除く)
- 共有 Runtime のワーカースレッド上から最後の同期 `Container` を drop するとハングし得る既知の限界がある。LogConsumer コールバック内での `Container` drop は避けること

## サンプル

同じ流れの統合テストです。

```bash
RUN_HOST_NETWORK_TESTS=1 cargo test --test nginx_http11 --features http_wait_plain -- --nocapture
```

## ライセンス

Apache License 2.0

```text
Copyright 2026 Shiguredo Inc.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```
