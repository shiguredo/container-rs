# Apple container 1.6.0 / 1.6.1 / 1.7.1 のリリースに追従する

- Created: 2026-11-08
- Completed: {YYYY-MM-DD}
- Branch: feature/update-follow-apple-container-1.7.1
- Polished: {YYYY-MM-DD}

## 目的

Apple container 1.6.0 (2026-10-20)、1.6.1 (2026-10-23)、1.7.1 (2026-11-06) がリリースされた。前回追従した 1.5.0 からの差分が本クレートの macOS (XPC) 経路に影響しないことを確認・記録し、ドキュメントを実態に合わせる。1.7.0 はタグのみで GitHub Release は無く、1.7.1 に含まれる。

1.6.0 でネットワークエイリアス (`AttachmentOptions.aliases`) が追加された。これは本クレートが利用する `containerCreate` の設定に新しい任意フィールドを足すものであり、互換性のための修正は不要だが、新しい機能として別 issue (0127) で対応する。

## 現状

- README.md の要件表は「Apple container 1.2.0 以上必須」で、macOS 26 (Apple Silicon) を要件としている。
- CI の test-apple-container ジョブは `brew upgrade container` により最新版を検証する。手元の環境は client / server とも 1.7.1 (build: release) である。
- 本クレートは Apple container の apiserver を XPC で直接呼び出しており、CLI には依存していない (唯一の CLI 依存は watchdog の `container rm --force`)。
- 前回の追従は 1.5.0 (issues/closed/0123) までで、1.6.0 / 1.6.1 / 1.7.0 / 1.7.1 は未確認である。

## 設計方針

### 1.5.0..1.6.0 (30 commits / 147 files)

本クレートが利用する XPC ルートと JSON スキーマの破壊的変更は確認できない。影響し得る変更は以下に限られる。

- Core: `container commit` コマンドが追加された。CLI の `container export` 相当と image load を組み合わせる実装で、apiserver の XPC ルートは追加されていない。本クレートに commit 相当の公開 API は無く、testcontainers-rs 0.28 にも無いため対象外とする。
- Networking: コンテナのネットワークアタッチメントに `aliases` が追加された (`AttachmentConfiguration.swift` の `AttachmentOptions`)。
  - `aliases` は `[String]` で、カスタム Codable によりキーが無い既存設定は空配列としてデコードされ、空のときはエンコードされない。本クレートの `NetOpts` は `hostname` と `mtu` のみを送るため、既存の送信内容はそのまま有効である。
  - `ContainersService.create` のホスト名重複検証が `AttachmentNameValidator` に置き換わり、ホスト名とエイリアスの大文字小文字を無視した比較、末尾ドットの正規化、エイリアスの検証が加わった。本クレートが送るホスト名 (明示 hostname → container_name → コンテナ ID) は一意であり、エイリアスは送らないため影響しない。
  - CLI はエイリアスに既定 DNS ドメインを補完してから送るが、XPC クライアントは補完しない。完全修飾名でないエイリアスはサーバーの `AliasPolicy` が拒否する (1 アタッチメントあたり最大 32 個、重複拒否、hostname との一致拒否)。
- Build: `container build` / `container builder` が CLI プラグインに分離された。本クレートはビルド機能を使わない。
- Kubernetes: 複数ワーカーの `container k8s` クラスタが追加された。本クレートは K8s 関連の機能を使わない。
- Security: pull-by-digest の検証 (GHSA-vgrw-36j7-r93w) と build context の symlink 競合 (GHSA-fqxg-rrf3-358v) が修正された。本クレートは image 参照を `imagePull` に渡すだけで、検証は apiserver 側で完結する。
- その他: image list の読めないエントリのスキップ、`container system status` の Seatbelt サンドボックス対応、空ログの follow 修正、`container-builder-shim` の更新。いずれも本クレートの経路と無関係か、透明な改善である。

### 1.6.0..1.6.1 (5 commits / 57 files)

- Core: 転送プロセスのシグナル表現を名前 (`SIGTERM` など) に統一する修正 (#1778) と、`SIGWINCH` を kill 対象から外す修正 (#1854) が入った。本クレートは `containerKill` を呼ばず、`containerStop` の `StopOptions.signal` は 1.5.0 の時点で文字列を送っているため影響しない。
- Core: OCI image から platform と runtime を推論する変更 (#2422) が入り、image サービスの XPC ルートに `imagePlatforms`、キーに `ociPlatforms` が追加された。本クレートは `containerCreate` に platform と `runtimeHandler` を明示的に送るため、推論の有無に依存しない。`imagePlatforms` は追加のみで未使用。
- Kubernetes: `container k8s` から `container cluster` への改名。CLI のみの変更で、本クレートは該当 CLI を使わない。

### 1.6.1..1.7.1 (23 commits / 131 files)

- Core: `container stats` の機械可読出力に `cpuUtilization` が追加された。本クレートは stats を公開 API として持たないため対象外。
- Core: Time Machine のバックアップ対象からイメージスナップショットとコンテナファイルシステムを除外する変更が入った。データの再生成可能性に関する変更で、XPC のスキーマは変わらない。
- Core: 既定カーネルの推奨が Kata 4.3.1 (`vmlinux-6.18.56-208-debug`) に変わった。`getDefaultKernel` のルートと `Kernel` JSON の形は不変で、既定カーネルは自動更新されない。手元の既定カーネルは `vmlinux-6.18.15-186` のままである。
- Networking: `config.toml` で既定の DNS nameserver / search domain / options を設定できるようになった。グローバル設定であり、本クレートの per-container 設定 API には影響しない。
- Build: BuildKit の新しいバージョンとの互換修正、build context の修正。本クレートはビルド機能を使わない。
- Kubernetes: `container cluster` の `--shared-image-store` 追加、`ls` の出力変更、ライフサイクルコマンドがクラスタノードを操作しなくなった。CLI の破壊的変更だが、本クレートは該当 CLI を使わない。

### containerization 0.47.0..0.54.0 (25 commits / 36 files)

本クレートが使う XPC ルートの引数・戻り値スキーマへの影響はない。主な変更は、registry client の content hash 検証 (GHSA-vgrw-36j7-r93w)、ImageStore の platform introspection、`ContainerStats` の CPU / メモリ / ネットワークプロトコル統計、cgroup v2 の `unified` 対応、sparse OCI image の import、cloud-hypervisor の direct I/O と virtiofsd オプション、`FileDescriptorOps` の symlink 安全なパス操作、EXT4 journal の修正、Bearer token 認証チャレンジの修正、runc 起動経路の修正である。

### 破壊的変更の有無

- XPC のルート定義 (`Sources/Services/ContainerAPIService/Client/XPC+.swift`) は 1.5.0 と 1.7.1 で同一である。
- `ContainersHarness.swift`、`ContainerConfiguration.swift`、`Filesystem.swift`、`ProcessConfiguration.swift`、`ContainerStopOptions.swift`、`ContainerSnapshot.swift`、`PublishPort.swift` も 1.5.0 と 1.7.1 で同一である。
- 追加は image サービスの `imagePlatforms` ルートと `ociPlatforms` キー、および `AttachmentOptions.aliases` のみで、いずれも additive である。
- したがって、1.6.0 / 1.6.1 / 1.7.1 に起因するコード修正は不要と判断する。最小要件の 1.2.0 は、`maskedPaths` / `readonlyPaths` と `ManagedContainer.nameValid` による ID 検証がいずれも 1.7.1 に存在するため維持する。

## 完了条件

- 1.5.0..1.6.0、1.6.0..1.6.1、1.6.1..1.7.1 と containerization 0.47.0..0.54.0 の差分を調査し、1.6.0 / 1.6.1 / 1.7.1 起因のコード修正が不要かどうかが確認できること。
- 調査結果がドキュメントの実態と整合し、追記が必要な修正が特定されていること (未検出の場合はその旨)。
- 公開ポートの大容量転送の既知の制約が 1.7.1 でも成立することが確認されていること。
- ネットワークエイリアスなど、追従すべき新しい機能が issue として切り出されていること。
