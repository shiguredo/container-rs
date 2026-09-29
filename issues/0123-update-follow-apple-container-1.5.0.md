# Apple container 1.5.0 のリリースに追従する

- Created: 2026-09-29
- Completed: {YYYY-MM-DD}
- Branch: feature/update-follow-apple-container-1.5.0
- Polished: 2026-09-29

## 目的

Apple container 1.5.0 (2026-09-29 タグ) がリリースされた。前回追従した 1.4.0 / 1.4.1 (issues/0122 で調査済み、open) からの差分が本クレートの macOS (XPC) 経路に影響しないことを確認・記録し、ドキュメントを実態に合わせる。

## 現状

- README.md の要件表は「Apple container 1.2.0 以上必須」で、macOS 26 (Apple Silicon) を要件としている。
- CI の test-apple-container ジョブは `brew upgrade container` により最新版を検証する。Homebrew は 1.5.0 を配布済みで、macOS 26.6.2 の手元環境は client / server とも 1.5.0 (build: release) で動作確認できる。
- 本クレートは Apple container の apiserver を XPC で直接呼び出しており、CLI には依存していない (唯一の CLI 依存は watchdog の `container rm --force`)。
- 前回の追従は 1.4.0 / 1.4.1 (issues/0122) まで。

## 設計方針

### 1.4.1..1.5.0

17 commits (27 ファイル) を調査した。本クレートが利用する XPC ルート (containerCreate / containerBootstrap / containerStartProcess / containerCreateProcess / containerWait / containerStop / containerDelete / containerLogs / containerCopyIn / containerCopyOut / containerList / volumeCreate / volumeInspect / imagePull / imageList / contentGet / getDefaultKernel) と JSON スキーマの変更は確認できない。影響し得る変更は以下に限られ、互換性のためのコード修正は不要である。

- Security: K8s plugin の kubeconfig sanitize (GHSA-44v5-vx46-ghv6)。本クレートは K8s 関連の機能を使わない。
- CLI: `container k8s start` の削除 (breaking)、`container k8s create --cni` 追加、unknown command のエラーメッセージ改善、PLUGINS 節の削除。いずれも CLI の話であり、本クレートは該当 CLI を使わない (watchdog の `container rm --force` は変更なし)。
- Core: イメージ entrypoint が `[""]` のときコマンドを実行できるよう CLI の Parser を修正 (#2296 / apple/container#2248)。XPC の `containerCreate` のスキーマは不変。ただし本クレートはイメージ config の Entrypoint / Cmd を自前解決するため、同じ意味論を別途実装する必要がある。1.5.0 起因の互換性問題ではなく既存の実装漏れであり、0124 で対応する。
- Network: localhost DNS 変更時の egress 喪失修正 (#2256)。pf アンカーを `com.apple.container` から `com.apple/container` へ移し、当該アンカーのみ再読込する `container system dns` (CLI) 側の修正であり、本クレートはこの CLI を使わないため影響しない。
- SocketForwarder: frontend 閉塞時に backend チャネルを即時クローズする修正 (#2260)。FD リークの修正であり、published port の大容量転送の挙動は変わらない (下記「実測」参照)。
- RuntimeLinux: VM リソースをコンテナの cgroup 制限から分離して確保する変更 (containerization #922 / 0.47.0 の `VMResources`)。本クレートが送る `resources` (`cpus` / `memoryInBytes` / `cpuOverhead`) はそのまま有効で、コンテナの制限値の意味は変わらない。
- Build: Globber の symlink 配下の扱い修正 (#2252)。本クレートは `container build` を使わない。

### containerization 0.45.0..0.47.0

11 commits を確認した。本クレートが使う XPC ルートの引数・戻り値スキーマへの影響はない。主な変更は、イメージ pull 時のサイズ検証強化 (過大 blob / zstd レイヤー / 破損レイヤー)、vminitd の runc 起動経路と seccomp 既定プロファイル、socket 転送のハング / I/O スタール修正、`ContainerStatistics` への filesystem 追加、`TokenResponse` の Codable 修正など。

したがってコード修正は不要と判断する。最小要件の 1.2.0 は 1.5.0 で廃止された機能が無いため維持する。

### 実測 (macOS 26.6.2 / Apple container 1.5.0)

- nginx: pull / create / published port / `WaitFor::http` / `with_copy_to` が成功した (`RUN_CONTAINER_TESTS=1 RUN_HOST_NETWORK_TESTS=1 cargo test --test nginx_http11 --features http_wait_plain`)。
- alpine: `exec` 大出力 / exec の stdout・stderr / `copy_file_from` / stdout follow が成功した (`RUN_CONTAINER_TESTS=1 cargo test --test container_macos`)。
- published port の大容量レスポンス切断は 1.5.0 でも再現した (10 MiB のレスポンスを遅い消費者で受信し 9,785,900 / 10,485,760 バイトで EOF。再現テストは `tests/nginx_http11.rs` の `published_port_large_response_truncates_for_slow_consumer`)。README.md / docs/TESTCONTAINERS.md の警告は維持する。

## 完了条件

- 1.4.1..1.5.0 と containerization 0.45.0..0.47.0 の差分を調査し、1.5.0 起因のコード修正が不要かどうかが確認できること。
- 調査結果がドキュメントの実態と整合し、追記が必要な修正が特定されていること (未検出の場合はその旨)。
- published port の大容量転送の既知の制約が 1.5.0 でも成立することが確認されていること。

## 解決方法

どのように対応するのかを明確にすること (例: どのようなコードを追加・修正するのか、どのようなテストを追加するのかなど)
