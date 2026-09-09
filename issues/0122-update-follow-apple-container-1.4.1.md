# Apple container 1.4.0 / 1.4.1 のリリースに追従する

- Created: 2026-09-09
- Completed: {YYYY-MM-DD}
- Branch: feature/update-follow-apple-container-1.4.1
- Polished: {YYYY-MM-DD}

## 目的

Apple container 1.4.0 (2026-09-08 タグ) と 1.4.1 (同日タグ) がリリースされた。前回追従した 1.3.0 からの差分が本クレートの macOS (XPC) 経路に影響しないことを確認・記録し、ドキュメントを実態に合わせる。

## 現状

- README.md の要件表は「Apple container 1.2.0 以上必須」で、macOS 26 (Apple Silicon) を要件としている。
- CI の test-apple-container ジョブは `brew upgrade container` により最新版を検証する。
- 本クレートは Apple container の apiserver を XPC で直接呼び出しており、CLI (`container run` / `container pull` のフラグ構文) には依存していない。
- 前回の追従は 1.3.0 (issues/closed/0121) までで、1.4.0 / 1.4.1 は未確認である。

## 設計方針

### 1.4.0..1.4.1

差分は 2 ファイルのみで、container 本体の Swift ソース変更は無い。

- `Package.swift`: `scVersion` を `0.43.0` から `0.45.0` へ更新
- `Package.resolved`: containerization を `0.43.0` から `0.45.0` へ更新

apiserver の XPC ルートと JSON スキーマは不変のため、本クレートへの影響は無い。

### 1.3.0..1.4.0

15 commits (42 ファイル) を調査した。本クレートが利用する XPC ルートと JSON スキーマの変更は確認できない。影響し得る変更は以下に限られ、いずれも本クレートの経路とは無関係である。

- 新規 XPC ルート `containerClean` (apiserver) と `com.apple.container.runtime/clean` (runtime) が追加された。`container clean` コマンド用であり additive。本クレートは未使用で、testcontainers-rs にも clean 相当の API は無い。
- tmpfs の `source` 修正 (`Sources/Services/ContainerAPIService/Client/Parser.swift` の `Parser`) は CLI パーサーの話。本クレートは `src/core/client/container_cfg.rs` の `MountCfg` 変換で tmpfs に既に `source: "tmpfs"` を送っているため影響しない。
- `HealthCheckHarness` の `apiServerVersion` 形式変更 (`ReleaseVersion.singleLine(appName:)` から `ReleaseVersion.version()` へ) は `container system status` / `container system version` の表示用。本クレートは health check ルートを呼ばないため無関係。
- `ImagesService` / `SnapshotStore` の digest 検証強化 (`validatedDigestEncoding()`) は disk usage 系の内部処理。本クレートは `imageList` / `imagePull` のみを使うため無関係。
- その他: `container clean` コマンド追加、`container system status` の表示拡充、JSON 出力のスラッシュ非エスケープ化、マシン一覧 JSON の出力統一、containerization 0.42.0 / 0.43.0 への更新。

### containerization 0.43.0..0.45.0

11 commits (40 ファイル) を確認した。cctl の NBD マウント対応、Swift Crypto v4 対応、CIDR 包含判定の修正、vsock の stdio ポートプール上限、`ContainerManager` への `Logger` 伝播、cctl の entrypoint / cmd 反映、sandboxy の Pi エージェント対応、oci-layout / index.json の正規ファイル化、vminitd の copy / stat パス解決の rootfs 内への制限、macOS での `sun_path` オーバーフロー修正など。本クレートが使う XPC ルートの引数・戻り値スキーマには影響しない。

したがってコード修正は不要と判断する。最小要件の 1.2.0 は 1.4.0 / 1.4.1 で廃止された機能が無いため維持する。

## 完了条件

- 1.3.0..1.4.0 と 1.4.0..1.4.1 の差分を調査し、1.4.0 / 1.4.1 起因のコード修正が不要かどうかが確認できること。
- 調査結果がドキュメントの実態と整合し、追記が必要な修正が特定されていること (未検出の場合はその旨)。
