# その他: 構築箇所のないエラー variant の方針を決定する

- Priority: Low
- Created: 2026-07-12
- Completed:
- Model: Kimi
- Branch: feature/update-dead-error-variants-policy
- Polished: 2026-07-21

## 目的

本家 0.27 互換のために定義した公開エラー型 / variant のうち、このクレートのどの経路からも構築されないものが複数存在する。これらは利用者が `match` で処理しても決して到達しない腕となり、公開 API が実際の挙動と一致していない。各型 / variant について「存置」「削除」「構築経路の追加」のいずれかの方針を決定し、方針に沿って整理する。

## 優先度根拠

到達不能な variant は panic や誤動作を起こさず、実行時の実害はない。一方で公開 API の正確さを損なう事案であり、整理は破壊的変更の判断を伴う。機能開発やバグ修正より優先度は低いため Low。

## 現状

以下の対象について、Grep で定義箇所と構築箇所を全照合し、Display / From impl / テスト以外に構築箇所がないことを確認した (実装時に改めて確認すること)。いずれも本家 0.27 では実際の構築経路を持つ公開 API であり、このクレートでは構造だけを移植した状態になっている。

- `CopyToContainerError` 全体 (`src/core/copy.rs`、`src/core.rs` で re-export): variant は `IoError` / `PathNameError`。macOS の copy 処理 (`src/runners/async_runner.rs` の `copy_to_sources`) は XPC `containerCopyIn` 経由で、一時ファイル書き出し失敗は `Error::other(...)` を返す。Linux 側は未実装エラー。どこからも構築されない
- `CopyFromContainerError::EmptyArchive` / `UnsupportedEntry` (`src/core/copy.rs`): `ContainerAsync::copy_file_from` (`src/core/containers/async_container.rs`) は XPC `copy_out` で単一ファイルを一時ファイルに取り出す方式のため、構築され得るのは `copy_from_reader` 内の `From<std::io::Error>` 経由の `Io` のみ。本家では tar エントリの検査から構築されるが、このクレートは tar を介さないため 2 variant は永遠に構築されない。なお `IsDirectory` は `copy_file_from` 内で実際に構築されているため対象外
- `ClientError::XpcNullReply` (`src/core/error.rs`): 定義と Display のみで構築箇所がない。なお `ClientError::Configuration` は Linux (Docker) 経路の `CreateContainerBody::from_config` (`src/core/client/docker_client.rs`) で実際に構築されているため対象外
- `WaitContainerError::StateUnavailable` / `Unhealthy` (`src/core/error.rs`): `HealthWaitStrategy::wait_until_ready` (`src/core/wait/health_strategy.rs`) は `HealthCheckNotConfigured` のみを返すため構築箇所がない
- `ExecError::WaitLog` (`src/core/error.rs`): `From<WaitLogError> for ExecError` は単体テスト以外から使われない。exec 経路 (`src/core/containers/async_container.rs`) が構築するのは `ExitCodeMismatch` のみ。公開 trait impl の削除は variant 削除より影響が大きいため、存置なら「本家互換のため保持」とコメントし、削除なら `From` impl とテストの両方を削除する

公開 API の削除は破壊的変更になるため、対応にはリリースタイミングの考慮が必要。

## 設計方針

対象の各型 / variant ごとに、以下のいずれかの方針を決定する。

- 存置: 本家互換のために意図的に保持する。存置理由をコメントで明記する
- 削除: 型 / variant 本体に加え、Display / From impl と該当テストもあわせて削除する
- 構築経路の追加: 該当経路を検証するテストの追加

判断基準は本家との名前・型の互換性をどこまで維持するか。過去に本家互換のために意図的に名前・型を揃えた経緯があるため安易な削除は避け、将来 Linux (Docker) 対応が進んだ際にこのクレートの Linux 経路で構築される見込みがある variant は存置を優先する。存置と構築経路の追加のみで決着し、削除が一つもない結論もあり得る。

なお issue 0010 (未使用の型・メソッド整理) が `HealthWaitStrategy` の `poll_interval` / `with_poll_interval` を扱っており、本 issue の `WaitContainerError::StateUnavailable` / `Unhealthy` と同じファイル (`src/core/wait/health_strategy.rs`) を触る。0010 が先に実装された場合、本 issue の行番号参照や設計方針の前提がずれるため、実装順に注意する。

## 完了条件

- [ ] 対象の各型 / variant について「存置」「削除」「構築経路の追加」のいずれかの方針が決定され、その結果と理由が本 issue に記録されること
- [ ] 方針に沿ってコードが整理されること (存置なら存置理由のコメント明記、削除なら型 / variant / Display / From impl / 関連テストの削除、構築経路の追加なら当該経路を検証するテストの追加)
- [ ] `cargo test` (統合テストを追加した場合は `cargo test --all-features`) と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
