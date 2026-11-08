# 機能追加: ネットワークエイリアスに対応する

- Created: 2026-11-08
- Completed: 2026-11-08
- Branch: feature/add-network-aliases
- Polished: {YYYY-MM-DD}

## 目的

Apple container 1.6.0 で、コンテナのネットワークアタッチメントに `aliases` が追加された。1 つのコンテナに複数の DNS 名 (完全修飾名と、先頭 `.` で始まるサブツリー名) を割り当てられる。Docker Engine API にも `NetworkingConfig.EndpointsConfig.<network>.Aliases` があり、user-defined network 上で同じ目的に使える。

本クレートは `with_hostname` で 1 つのホスト名を設定できるだけで、同じコンテナを複数の名前で解決する手段が無い。testcontainers では「同一ネットワーク上のサービスを別名で参照する」構成が頻出するため、shiguredo 拡張としてエイリアス設定 API を追加する。

## 現状

- `ImageExt::with_network` はネットワーク名だけを保持し、`ContainerRequest` にエイリアスを保持するフィールドは無い。
- macOS: `containerCreate` の `networks[0].options` は `hostname` と `mtu` のみを送る (`src/core/client/container_cfg.rs` の `NetOpts`)。
- Linux: `CreateContainerBody` の `NetworkingConfig` は `EndpointsConfig.<network>:{}` のみで、`Aliases` を送っていない (`src/core/client/docker_client.rs`)。
- Apple container 1.6.0 の `AttachmentOptions` は `aliases: [String]` を持つ。キーが無い既存設定は空配列としてデコードされ、空のときはエンコードされない。
- Apple container の API サーバーはエイリアスを CLI のようには正規化しない。CLI は既定 DNS ドメイン (`test`) を補完してから設定に載せるが、XPC クライアントは補完しない。サーバーの `AliasPolicy` は、完全修飾名 (2 ラベル以上) を要求し、1 アタッチメントあたり最大 32 個、重複、hostname との一致を検証する。
- Docker のエイリアスは user-defined network でのみ有効である。デフォルト bridge を指定すると `network-scoped aliases are only supported for user-defined networks` で失敗する。

## 設計方針

### 公開 API

`ImageExt` に次の 2 メソッドを追加する (shiguredo 拡張)。複数回呼び出した場合は追加する (上書きしない)。`with_network` と同じく、既存の設定を壊さない。

- `with_network_alias(self, alias: impl Into<String>) -> ContainerRequest<I>`
- `with_network_aliases(self, aliases: impl IntoIterator<Item = impl Into<String>>) -> ContainerRequest<I>`

`ContainerRequest` に `network_aliases: Vec<String>` と accessor `network_aliases(&self) -> &[String]` を追加する。

### 共通の検証

start 時に、pull / resolve より前に次を検証し、違反は `ClientError::Configuration` で明示エラーにする。

- 空文字列のエイリアスを拒否する。
- 同一エイリアスの重複を拒否する (Apple container は重複を拒否するため、Docker でも同じ規則に揃える)。

### macOS (Apple container)

- `NetOpts` に `aliases: Vec<String>` を追加し、`networks[0].options.aliases` に載せる。空のときはキー自体を出さない (Apple のエンコーダと同じ表現)。
- start 時に、先頭の `.` を除いた名前が `.` を 1 個以上含むこと (完全修飾名) と、32 個以下であることを検証する。CLI と違い既定 DNS ドメインを補完しないため、1 ラベルの名前は指定できない。この差は rustdoc と docs に明記する。
- エイリアスは Apple container 1.6.0 以上が必要である。1.2.0〜1.5.0 の `AttachmentOptions` は未知キーを無視するため、古いランタイムでは黙って無効になる。バージョン検出は行わず、サポート条件としてドキュメントに記載する。

### Linux (Docker Engine API)

- `ContainerConfig` と `CreateContainerBody` に `network_aliases: Vec<String>` を追加し、`NetworkingConfig.EndpointsConfig.<network>.Aliases` に載せる。エイリアスが空のときの JSON は従来と同一にする。
- 未指定 (デフォルト bridge) または組み込みの `bridge` / `host` / `none` を指定してエイリアスを指定した場合は start 時に明示エラーにする (これらでは解決できないため)。
- 1 ラベルのエイリアスは Docker が受け付けるため、Linux では許可する。macOS との差はドキュメントに明記する。
- ネットワークの自動作成は行わない (既存の `with_network` と同じ)。

### テスト

- 単体: `network_aliases()` accessor、追加と複数指定、空文字列・重複・macOS の非完全修飾名・32 個超過の検証、macOS / Linux の config JSON (aliases の有無)。
- macOS 統合: エイリアス付きでコンテナを起動し、ホスト側の container DNS サーバー (`127.0.0.1:2053` UDP) に A クエリを送ってコンテナ IP に解決されることを確認する。複数エイリアスも確認する。
- Linux 統合: `docker network create` で user-defined network を作り、同じネットワークの別コンテナから `getent hosts` でエイリアスが解決されることを確認する。テスト後にコンテナとネットワークを削除する。
- `docs/TESTCONTAINERS.md`、`skills/shiguredo-container/SKILL.md`、`CHANGES.md` を更新する。

## 完了条件

- [ ] `with_network_alias` / `with_network_aliases` と `network_aliases()` accessor が追加されていること
- [ ] macOS でエイリアス付きコンテナが起動し、ホスト側 DNS サーバーでエイリアスがコンテナ IP に解決されること (統合テスト)
- [ ] Linux で user-defined network 上の別コンテナからエイリアスが解決されること (統合テスト)
- [ ] 空文字列・重複・macOS の非完全修飾名・32 個超過が start 前に明示エラーになること (単体テスト)
- [ ] Linux で未指定・`bridge` / `host` / `none` のエイリアスが start 時に明示エラーになること (単体テスト)
- [ ] エイリアス未指定時の macOS / Linux の config JSON が従来と同一であること (単体テスト)
- [ ] `docs/TESTCONTAINERS.md` (ImageExt 対応表・accessor 一覧・shiguredo 拡張の件数と一覧) が更新されていること
- [ ] `skills/shiguredo-container/SKILL.md` の ImageExt 表と既知の制限事項が更新されていること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されていること
- [ ] `cargo test --all-features`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check` が成功すること

## 解決方法

`ImageExt` に `with_network_alias` / `with_network_aliases` を追加し、`ContainerRequest` に `network_aliases: Vec<String>` と `network_aliases() -> &[String]` accessor を追加した。複数回呼び出すと追加され、`with_network_aliases` は複数件をまとめて追加する。

- macOS: `src/core/client/container_cfg.rs` の `NetOpts` に `aliases` を追加し、空でないときだけ `networks[0].options.aliases` に出す。`validate_macos_network_aliases` で空文字列・重複・完全修飾名 (先頭 `.` を除いて `.` を含む)・32 個以下を検証し、`AsyncRunner::start` の pull 前と `build_config` の両方で呼ぶ。
- Linux: `ContainerConfig` / `CreateContainerBody` に `network_aliases` を追加し、`NetworkingConfig.EndpointsConfig.<network>.Aliases` に出す。`validate_linux_network_aliases` で空文字列・重複と、未指定・`bridge` / `host` / `none` を拒否する。`AsyncRunner::start` の pull 前と `build_container_config` で値を渡す。
- 単体テスト: accessor と追加順、空文字列・重複、macOS の完全修飾名・32 個超過・`build_config` 経由の拒否、Linux の user-defined network 必須、macOS / Docker の config JSON (aliases の有無)。
- 統合テスト: macOS はエイリアス付きコンテナを起動し、ホスト側 container DNS サーバー (`127.0.0.1:2053`) の A レコードがコンテナ IP に一致すること、サブツリーエイリアスが配下の名前を解決すること、削除後にエイリアスが解放されることを確認した。Linux は `docker network create` で作った user-defined network 上で、別コンテナから `getent hosts` により 2 つのエイリアスが server の IP に解決されることを確認する。
- ドキュメント: `docs/TESTCONTAINERS.md` (ImageExt 対応表・accessor 一覧・サマリの shiguredo 拡張 25 → 28 と内訳合計 396 → 399・「意図的に保持する shiguredo 拡張」)、`skills/shiguredo-container/SKILL.md` (ImageExt 表・既知の制限事項・参考資料の件数)、`CHANGES.md` の `[ADD]` を更新した。

検証:

- `RUN_CONTAINER_TESTS=1 cargo test --all-features`: macOS 統合テスト 96 件を含めすべて成功した。単体テストは追加分を含めて 256 件成功した。
- `cargo clippy --all-targets --all-features -- -D warnings` と `cargo clippy --target x86_64-unknown-linux-gnu --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check` が成功した。
- Linux の統合テストは macOS 上では実行できないため、Linux ターゲットの `cargo check` / clippy と、Docker Engine API へ同じ JSON (`NetworkingConfig` の `Aliases`) を直接送る手動検証で確認した。手動検証では user-defined network 上の 2 つのエイリアスが別コンテナから解決できた。
