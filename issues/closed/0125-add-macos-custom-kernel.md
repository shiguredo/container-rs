# 機能追加: macOS でカスタムカーネルを指定できるようにする

- Created: 2026-10-07
- Completed: 2026-10-07
- Branch: feature/add-macos-custom-kernel
- Polished: 2026-10-07

## 目的

Apple container の既定カーネルは tc (netem / htb / u32) 非搭載で、tc を使うネットワーク劣化テストにはカスタムカーネルが必須になる。CLI には `container run -k, --kernel <path>` がありコンテナ単位でカスタムカーネルを指定できるが、本クレートは既定カーネル固定で対応していない。macOS (XPC) 経路でカーネル指定に対応する。

## 現状

- `src/runners/async_runner.rs` の macOS 経路は start 時に常に `client.get_default_kernel()` を呼び、その結果を `client.create_container(&cfg, kernel)` に渡す。ユーザーがカーネルを指定する API は無い
- `src/core/client/xpc_client.rs` の `get_default_kernel` は XPC `getDefaultKernel` の応答 (`kernel`) をそのまま返し、`create_container` はそれを `containerCreate` の `kernel` 引数にそのまま載せる
- この `kernel` のデータは生の vmlinux ではなく、containerization の `Kernel` 構造体を JSON エンコードしたもの。形式は `{"path":"file:///...","platform":{"os":"linux","architecture":"arm64"},"commandLine":{"kernelArgs":[...],"initArgs":[...]}}`。`path` は Swift の `URL` Codable により file URL 文字列 (`file:///` + パーセントエンコード) になる (実測確認済み)
- サーバー側 (`ContainersHarness.create`) は `kernel` を `Kernel.self` として JSON デコードして使う
- Apple container CLI 1.5.0 には `-k, --kernel <path>` がある。CLI は指定時にファイル存在を確認して `Kernel(path:, platform: .current)` を組み立て、未指定時のみ `getDefaultKernel` を呼ぶ。カーネルはホストアーキテクチャに一致させる必要がある (amd64/Rosetta ゲストでもカーネルはホスト側の arm64)
- `ImageExt` (`src/core/image/image_ext.rs`) と `ContainerRequest` (`src/core/containers/request.rs`) にカーネル指定の API は無い
- Linux (Docker Engine API) はホストとカーネルを共有するため、コンテナ単位のカーネル指定は原理的に不可

## 設計方針

- shiguredo 拡張として `ImageExt` に setter を追加する (本家 testcontainers-rs に同名 API は無い)
  - 例: `with_kernel(self, kernel_path: impl AsRef<Path>)`。パス引数は `AsRef<Path>` にする規約に合わせる。内部では `PathBuf` として保持し、`kernel()` は `Option<&Path>` を返す。rustdoc に使用例を書く
  - 複数回呼び出しは上書き (`with_ready_conditions` と同じパターン)
  - `ContainerRequest` に `kernel() -> Option<&Path>` accessor を追加する (setter と accessor を対にする既存方針)
- macOS の start 経路 (`src/runners/async_runner.rs`)
  - `Kernel` JSON の組み立ては、XPC ペイロードの型 (`Platform` 等) がある `src/core/client/xpc_client.rs` に `pub(crate)` の関数として置く (`oci_platform_json` が同ファイルの private 関数のため)。単体テストも同ファイルの `mod tests` に追加する
  - `with_kernel` 指定時は `get_default_kernel` を呼ばず、`Kernel` JSON を自前で組み立てて `create_container` に渡す (既定カーネル未インストール環境でも動く。CLI と同じ挙動)
  - `path` は file URL 文字列 (`file:///...`、パーセントエンコード) にする。`URL(filePath:).absoluteString` 相当
  - `platform` は既存 `oci_platform_json("arm64")` の `{"os":"linux","architecture":"arm64"}` を使う (既存方針・CLI と同方針)
  - `commandLine` は `Kernel(path:platform:)` の既定 (`kernelArgs: ["console=hvc0","tsc=reliable","panic=0"]`、`initArgs: []`) を使う。古いランタイムでは未知キーとして無視されるため、常に全フィールドを出す
  - 検証: pull より前に絶対パス・実ファイルであることを確認し、不正なら `ClientError::Configuration` (既存 variant) を返す (fail-fast)。CLI は相対パスを cwd 基準で絶対化するが、本クレートは解釈せず絶対パス必須として明示エラーにする (apiserver は別プロセスで cwd が異なるため)
  - カーネル実体は XPC メッセージに載せず `path` で渡す (既存の `getDefaultKernel` 応答と同じ扱い)
- Linux: `linux_unsupported_request_reason` に追加し、`with_ssh` と同じ扱いで start 時に明示エラーを返す (`with_kernel() is not implemented on Linux`)
- `--kernel-arg` 相当 (カーネル引数の追加) は本 issue の対象外とする
- 新規 API 追加のみで既存呼び出しは壊れない。CHANGES.md は `[ADD]` とする
- 最小サポートバージョン: 検証は Apple container 1.5.0 で行う。`Kernel` JSON の形は containerization 側の変更に依存するため、検証の結果 1.5 未満で動作しないと判明した場合は最小サポートバージョン (現在 1.2.0) を 1.5 以上に引き上げてよい

## 完了条件

- [ ] macOS で `with_kernel` にカスタムカーネルを指定してコンテナが起動できること (統合テスト)。カーネルパスは環境変数 `CONTAINER_TEST_KERNEL_PATH` で指定し、未設定ならスキップする (値は絶対パスで、`1` かどうかは見ない。既存の `WATCHDOG_VICTIM` と同じ方式)。既定カーネルを指すだけの空テストにしないこと。指定したカーネルが使われたことは既定カーネルとの差が観測できることで確認する (例: `/proc/config.gz` の `CONFIG_NET_SCH_NETEM` が、netem 入りカーネルでは `=y` (または `=m`)、既定カーネルでは `# CONFIG_NET_SCH_NETEM is not set` であること。既定カーネルでも文字列自体は現れるため、有無では判定できない)。`tc` の実行で確認する場合は、イメージに iproute2 を入れ、`with_cap_add("NET_ADMIN")` を指定する (既定の capability セットに NET_ADMIN は入っていない)
- [ ] `with_kernel` 未指定時は従来どおり既定カーネルで起動すること (既存テストが通ること)
- [ ] 不正な指定 (相対パス / 存在しないパス / ディレクトリ) で start 時に明示エラーになること (テストで確認)
- [ ] `Kernel` JSON 生成の単体テストがあること (`path` の file URL エンコード・`platform`・`commandLine`。`oci_platform_json_contains_os_and_architecture` と同じ方式)
- [ ] Linux で `with_kernel` 使用時に start 時明示エラーになること (単体テスト。既存の `with_masked_paths` の Linux テストと同じ方式)
- [ ] `kernel()` accessor の追加とテストが完了していること
- [ ] `docs/TESTCONTAINERS.md` (2 章 `ImageExt` 対応表・8 章 `ContainerRequest<I>` の accessor 一覧・Docker 節の Linux fail-fast 記述と「未実装 (start 時 fail-fast)」の列挙・Apple Container サマリの shiguredo 拡張件数と内訳合計・「意図的に保持する shiguredo 拡張」の列挙)、`skills/shiguredo-container/SKILL.md` (`ImageExt` の主要メソッド表・既知の制限事項の Linux の残ギャップ・環境変数表)、`README.md` (Linux の注意書きにある未対応 API の列挙) が更新されていること。最小サポートバージョンを引き上げた場合は `README.md` の要件表に加え、`docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` のランタイム要件も更新されていること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

`ImageExt` に shiguredo 拡張として `with_kernel(self, kernel_path: impl AsRef<Path>)` を追加し、`ContainerRequest` に `kernel` (`Option<PathBuf>`) と `kernel() -> Option<&Path>` accessor を追加した (`src/core/image/image_ext.rs` / `src/core/containers/request.rs`)。複数回呼び出しは後から指定したパスで上書きする。

macOS の `AsyncRunner::start` は `with_kernel` 指定時に `getDefaultKernel` を呼ばず、`src/core/client/xpc_client.rs` の `kernel_json` で `containerCreate` へ載せる containerization の `Kernel` JSON を組み立てる。`path` は Swift の `URL(filePath:).absoluteString` と同じ規則でパーセントエンコードし (`file_url_string`)、`platform` は既存の `oci_platform("arm64")` を `oci_platform_json` と共有、`commandLine` は `Kernel(path:platform:)` の既定 (`console=hvc0` / `tsc=reliable` / `panic=0`、`initArgs` は空) を常に出す。

`validate_kernel_path` は pull / resolve より前に絶対パス・UTF-8・実ファイルであることを検証し、違反時は `ClientError::Configuration` を返す。Linux は `linux_unsupported_request_reason` に追加し、`with_ssh` と同じく start 時に `with_kernel() is not implemented on Linux` の明示エラーになる。

テストは次を追加した: `kernel_json` の完全一致と `file_url_string` の Swift 一致 (単体)、`kernel()` accessor と上書き (単体)、`validate_kernel_path` の正常系 (単体)、Linux の fail-fast (Linux 単体)、不正パス 4 種 (macOS 統合)、カスタムカーネルでの起動と特殊文字を含むパスでの起動 (macOS 統合、`CONTAINER_TEST_KERNEL_PATH` 指定時のみ実行)。

検証は Apple container 1.5.0 で行った。カスタムカーネルは containerization の `kernel/Makefile` でビルドした netem 入りのものを使い、既定カーネルを指定した場合は netem 検証が失敗すること (空テストでないこと) も確認した。

ドキュメントは `docs/TESTCONTAINERS.md` (2 章 `ImageExt` 対応表・8 章 accessor 一覧・Docker 節の Linux fail-fast 記述・サマリの shiguredo 拡張 23 → 25 と内訳合計 394 → 396・「意図的に保持する shiguredo 拡張」)、`skills/shiguredo-container/SKILL.md` (ImageExt 表・既知の制限事項・環境変数表)、`README.md` と `src/lib.rs` (Linux の未対応 API 列挙) を更新し、`CHANGES.md` に `[ADD]` を追記した。
