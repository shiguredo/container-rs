# バグ: macOS の published port で大容量レスポンスが遅い消費者に対して途中で切断される

- Created: 2026-08-18
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-published-port-large-transfer-truncation
- Polished: 2026-08-18

## 目的

macOS (Apple Container) の published port 経由で、消費者 (HTTP クライアント) がサーバーの送信速度に追いつかない場合に、大容量レスポンスのボディが途中で切断され、クライアントが不完全なレスポンスを「正常な EOF」として受信してしまう問題を記録し、利用者向けの制約・回避策を整備する。

## 現状

shiguredo_container の macOS バックエンド (Apple Container / XPC) で published port (`with_exposed_port` / `with_mapped_port`) 経由の HTTP 転送を行うと、サーバー → クライアント方向の大容量レスポンスが、消費者が遅い場合に途中で切断される。切断は TCP の RST ではなくきれいな EOF (FIN) として観測される。切断のメカニズムはフォワーダーのバッファ溢れと推定される (「原因の切り分け」参照)。また、`docs/TESTCONTAINERS.md` の「macOS の制約 (Local Network Privacy)」節では published port 経由の接続ブロックは文書化済みだが、大容量転送の途中切断は未文書化である。

### 再現手順

http11-rs の nginx 統合テスト (`examples/http11_client/tests/nginx_upload.rs` の `put_10mb_binary_roundtrip`) を macOS 上で実行する。このテストは WebDAV PUT で 10 MiB のバイナリをアップロードし、GET で同一バイトを取得して完全一致を検証する (nginx コンテナは published port で起動される)。

1. 前提を満たす: macOS 側は `container system start` 済みかつ Local Network Privacy でコンテナへの接続が許可されていること (未許可だと接続がブロックされ、本バグは再現しない)。また、このテストのヘルパー (`ensure_docker`) が `docker version` の成功 (Docker daemon の応答) を要求するため、Docker daemon の起動も必要である
2. http11-rs の `examples/http11_client/` で `cargo test --test nginx_upload put_10mb_binary_roundtrip` を実行する
3. GET レスポンスのボディが Content-Length (10 MiB) に満たない位置 (例: 8.5〜9.9 MiB) で EOF になり、以降データが届かない
4. テストは GET の `Connection closed before response complete` エラーで panic し、失敗する (約 85% の確率で再現する)

### 観測結果

本 issue の観測記録 (「現状」の EOF (FIN) 観測を含む) は、2026-08-18 時点の macOS (Apple Container 1.2.2) 環境で観測したものである。切断の再現は確率的であり、環境により再現率は変わり得る。

- 高速消費者 (curl、読み間隔なし) は 10 MiB を 10/10 回完全受信する
- 生 TcpStream の 8 KiB 読み (間隔なし) も 20/20 回完全受信する
- 8 KiB 読み + 読み合間に 1 ms 待つ観測用プローブ (リポジトリに残していない一時コード) では、1 MiB でも途中切断が発生する (切断位置は毎回ばらつく)
- `http11_client` (8 KiB ずつ読み、読み合間にデコード処理を持つ) は 10 MiB GET が約 85% 失敗する (切断位置は 8.5〜9.9 MiB 付近でばらつく)
- Linux (Docker) では同一テストが 3/3 回成功する
- container CLI / ランタイム 1.2.2 でも再現する

### 原因の切り分け

- データ経路に shiguredo_container のコードは介在しない。published port の設定は `src/core/client/container_cfg.rs` の `build_config` が `publishedPorts` に反映するのみで、転送自体は Apple 側のポートフォワーダー (`container-runtime-linux`) が担う
- 消費者が速ければ問題が出ないこと、読みの間に意図的な待機を入れるだけで再現することから、フォワーダーのバッファ溢れ時に背圧 (backpressure) をかけずデータを切り捨てていると推定される

## 設計方針

- 原因は Apple 側 (ポートフォワーダー) と推定され、本クレートから直接修正できない可能性が高い。再現手順と切り分け結果を残し、Apple へのフィードバック (Feedback Assistant 等) の材料にする
- 本クレート側でできる対応として、利用者向けの制約・回避策を `docs/TESTCONTAINERS.md`・`README.md`・`skills/shiguredo-container/SKILL.md` に明記する
  - 回避策の候補: macOS で大容量レスポンスを扱う場合は published port ではなくコンテナ IP 直結を使う (`ContainerAsync::get_bridge_ip_address` で取得した IP へ直接接続する。直結も Local Network Privacy 未許可の環境では接続がブロックされ得る点に注意)
  - 回避策の有効性 (コンテナ IP 直結で遅い消費者でも大容量を完全受信できること) は未観測のため、文書化の前に「観測結果」のプローブと同様の遅い消費者 (8 KiB 読み + 待機) で macOS 上で 1 件観測して本 issue に記録する。直結でも切断が観測された場合は回避策として文書化せず、観測結果を本 issue に記録して方針を再検討する
  - 消費者側の読み間隔を詰めても安全なサイズ域は環境依存であり保証できないため、保証値は提示しない
- 再現テストは container-rs の `tests/nginx_http11.rs` に `RUN_HOST_NETWORK_TESTS=1` ゲート付きで追加し、環境で再現できる状態を維持する (http11-rs 側には同ゲートの仕組みが無いため container-rs 側に置く)。「観測結果」のプローブと同様に遅い消費者 (8 KiB 読み + 待機) を模擬し、途中切断が観測されることを期待値とする (現在の不具合挙動のスナップショット。Apple 側で修正された場合は期待値を更新する)。再現は確率的 (約 85%) なため、確認は数回の実行で行う
- 再現テスト追加はコード変更のため、`CHANGES.md` の misc にエントリを追記する (0056 で確立した解釈に従う。ドキュメント変更分は非対象)

## 完了条件

- 本 issue に、追試可能な再現手順と観測結果が記録されている
- `tests/nginx_http11.rs` に、`RUN_HOST_NETWORK_TESTS=1` ゲート付きの再現テストが追加され、実行時に途中切断が観測できること (再現が確認できること)
- 再現テスト追加に対応する `CHANGES.md` の misc エントリが追記されている
- コンテナ IP 直結で遅い消費者でも 10 MiB レスポンスを完全受信できることを確認した観測記録が本 issue に残されている
- `docs/TESTCONTAINERS.md`・`README.md`・`skills/shiguredo-container/SKILL.md` に、macOS の published port で大容量レスポンスが遅い消費者に対して途中切断され得る制約と回避策 (コンテナ IP 直結等) が記載されている
- Apple へのフィードバックの送信状況 (送信した / 送らない判断) が本 issue に記録されている
