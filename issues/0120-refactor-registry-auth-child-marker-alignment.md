# リファクタリング: registry_auth の子プロセステスト全体に silent-pass 防止 marker 方式を揃える

- Created: 2026-08-18
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-registry-auth-child-marker-alignment
- Polished: {YYYY-MM-DD}

## 目的

`src/core/client/registry_auth.rs` の既存の子プロセス分離テスト (`x_registry_auth_picks_docker_hub_entry_for_docker_io_reference` / `x_registry_auth_docker_hub_child`) にも、直近の変更 (`load_config_json` 系テストで導入した子側 stdout marker + `--nocapture` + `.output()` パターン) を水平展開する。両系統でテスト起動と結果検証の書き方を揃え、libtest の `--exact` 0 マッチ silent-pass への耐性を高める。

## 現状

- `load_config_json` 系テストは、子側で `println!("{LOAD_CONFIG_CHILD_MARKER}");` を出力し、親側で `.output()` の stdout に `LOAD_CONFIG_CHILD_MARKER` が含まれることを assert している (子起動 args に `--nocapture` を必ず付ける)。これにより、子テスト名にタイポがあって libtest が 0 件マッチで exit 0 を返してもすぐに検知できる
- 既存の `x_registry_auth_docker_hub_child` (child) と `x_registry_auth_picks_docker_hub_entry_for_docker_io_reference` (parent) は `.status()` のみで exit code を確認しており、marker 検証や `--nocapture` を持たない。子テスト名を将来リファクタリングでずらすと silent-pass する
- 定数 `DOCKER_HUB_TEST_CHILD_ENV` (`SHIGUREDO_CONTAINER_REGISTRY_AUTH_TEST_CHILD`) と `LOAD_CONFIG_TEST_CHILD_ENV` (`SHIGUREDO_CONTAINER_LOAD_CONFIG_TEST_CHILD`) は「子プロセスとして起動されたこと」を示す用途で本質的に同じだが、名前と値が別々に定義されている

## 設計方針

- 既存 `x_registry_auth_picks_docker_hub_entry_for_docker_io_reference` (parent) を `.output()` に変更し、子側 stdout に marker が出力されていることを親側で assert する。子起動 args に `--nocapture` を追加する
- 既存 `x_registry_auth_docker_hub_child` (child) の入口に marker `println!` を追加する (marker 定数は既存の `LOAD_CONFIG_CHILD_MARKER` を再利用する。もしくは両系統共通の `REGISTRY_AUTH_CHILD_MARKER` に改名する。命名は実装時に検討)
- `DOCKER_HUB_TEST_CHILD_ENV` と `LOAD_CONFIG_TEST_CHILD_ENV` を統合できるか検討する (2 系統のテストが同時に走ることは想定していないため、実質「子プロセスとして起動されたことを示す共通フラグ」に集約できる。ただし既存 `x_registry_auth_docker_hub_child` は `DOCKER_AUTH_CONFIG` を積極的に子側にセットする使い方で、`load_config_json` 系は `env_remove` で排除する使い方であり、混同事故を防ぐには別 env のままにする選択肢もある。統合の是非は実装時に判断)
- 挙動 (テストの成功条件・検証対象) は変えない。silent-pass 検知の強化のみが目的
- テストヘルパの共通化は必須ではないが、`spawn_load_config_child` を `spawn_registry_auth_child` 相当にリネームして 2 系統で共有できるなら共有する

## 完了条件

- `x_registry_auth_picks_docker_hub_entry_for_docker_io_reference` が `.output()` + marker 検証に切り替わり、子テスト名にタイポを入れて手動で走らせると失敗すること
- 既存の Docker Hub 認証エントリ取得の挙動が変わらないこと (assertion 内容は同一で通ること)
- `load_config_json` 系テスト 4 本の挙動が変わらないこと
- `DOCKER_HUB_TEST_CHILD_ENV` / `LOAD_CONFIG_TEST_CHILD_ENV` を統合するか、そのままにするかを決めて実装する (統合しない場合は理由をコメントに残す)
- `CHANGES.md` の `## develop` の `### misc` サブセクションに `[UPDATE]` エントリを追加する (機能に直接影響しないリファクタリングのため)
