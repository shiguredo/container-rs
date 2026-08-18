# テスト: ContainerRequest / ImageExt の公開アクセサに単体テストを追加する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/add-container-request-unit-tests
- Polished: 2026-08-02
- Updated: 2026-08-07

## 目的

公開 API の純ロジック (コンテナ不要で検証できるアクセサ群) のうち、PBT では実現できないケース (境界値・意図的な優先順位) に単体テストが無いのを解消する。

## 現状

- `src/core/containers/request.rs` の `ContainerRequest` の公開アクセサ 31 個 (`image` / `network` / `labels` / `container_name` / `hostname` / `env_vars` / `hosts` / `mounts` / `health_check` / `copy_to_sources` / `ports` / `privileged` / `readonly_rootfs` / `cap_add` / `cap_drop` / `shm_size` / `entrypoint` / `cmd` / `descriptor` / `ready_conditions` / `expose_ports` / `exec_after_start` / `startup_timeout` / `working_dir` / `user` / `open_stdin` / `init` / `platform` / `ssh` / `masked_paths` / `readonly_paths`) に公開アクセサの単体テストが無い (PortMapping の 2 アクセサを含めると 33 個)。`#[cfg(test)]` モジュール自体は `reject_duplicate_mapped_ports` (0091 で追加) のテスト用に存在するが、公開アクセサのテストは含まれない
- `src/core/image/image_ext.rs` の `ImageExt` の `with_*` メソッド 30 個も同様に `#[cfg(test)]` モジュールが無い (一部は `container_cfg.rs` の macOS ゲート単体テストや `async_runner.rs` の `linux_tests` で間接検証されているが、アクセサ自体の単体テストではない)
- `env_vars` の連結順序 (image 側が先・リクエスト側が後)・`descriptor` の name/tag 合成とフォールバック・`cmd` のフォールバック (空で上書きできない境界)・`ready_conditions` のオーバーライド優先は、どこからも検証されていない (ready_conditions のオーバーライドのみ統合テストで検証されているが、CI では実行されない)
- 一方で `ContainerState` (`src/core/image.rs`) や `ExecCommand` (`src/core/image/exec.rs`) には単体テストが存在し、非対称
- 設定→取得の往復などのラウンドトリップ性質の検証は、`pbt/tests/` の既存 PBT (`prop_host.rs` / `prop_ports.rs`) ではカバーされておらず、別 issue での PBT 追加が必要 (本 issue では対象外)

## 設計方針

公開 API の単体テストは shiguredo-rust 規約に従い `tests/test_request.rs` / `tests/test_image_ext.rs` に配置する (既存の `src/` 内 `#[cfg(test)]` は private を対象とする場合の配置であり、本 issue の対象は公開アクセサのため `tests/` が正しい)。

shiguredo-rust 規約「unittest は pbt で実現できないものだけを書くこと」に従い、本 issue の単体テストは PBT で実現できないケース (意図的な優先順位・フォールバック境界・同一キー重複時の連結) に限定する。設定→取得の往復などのラウンドトリップ性質は PBT の対象であり、本 issue では対象外とする (PBT での検証は別 issue として起票する。`pbt/tests/` には既に `prop_host.rs` / `prop_ports.rs` が存在するが、これは `Host` / `ContainerPort` の PBT であり `ContainerRequest` のアクセサは対象外)。

構築は `GenericImage` + `with_*` を基本とするが、`env_vars` の連結順序・`cmd` のフォールバック・`mounts` / `copy_to_sources` の連結の検証には、非空の `env_vars()` / `cmd()` / `mounts()` / `copy_to_sources()` を返すテスト専用の `Image` 実装が必要 (実データを返すだけの fixtures であり、モック・スタブには該当しない。`GenericImage` の `Image` 実装はこれらを空既定のままのため検証できない)。テスト専用の `Image` 実装は `tests/helpers/` に配置する (shiguredo-rust 規約「テスト間で共有するヘルパーは `tests/helpers/` に置くこと」に従う)。

検証対象外とするアクセサを明記する: `with_log_consumer` (取得アクセサが存在しないため往復検証不可)、`exec_after_start` (引数の `ContainerState::new` が `pub(crate)` のため `tests/` から構築不能)、`ssh` / `masked_paths` / `readonly_paths` (設定→取得の往復のみで PBT の対象のため)。

## 完了条件

- 以下の単体テストが追加され、検証される:
  - `env_vars`: image 側が先・リクエスト側が後の連結順序 (同一キーは両方 emit される。勝者までは契約外)
  - `descriptor`: name / tag の合成と、未指定時の image 側へのフォールバック
  - `ready_conditions`: リクエスト側のオーバーライド優先 (0058 が委譲済み)
  - `cmd`: リクエスト側が空の場合の `Image::cmd` へのフォールバック (`with_cmd([])` ではクリアできない境界も固定する)
  - `mounts` / `copy_to_sources`: image 側とリクエスト側の連結 (`CopyToContainer` に内容検証手段が無いため件数のみ検証)
- 新規テストがコンテナを必要としないこと (`cargo test --test test_request --test test_image_ext` がコンテナ無しで通過する)
- ラウンドトリップ性質 (往復) の検証を PBT で行う別 issue が起票されていること

## 解決方法

- `tests/test_request.rs` を新規作成し、`GenericImage` + `with_*` で構築した `ContainerRequest` のアクセサのうち、上記の検証対象をテストする (テスト名は既存の命名に合わせて `env_vars_image_before_request` / `descriptor_name_tag_composition` / `ready_conditions_request_overrides` / `cmd_falls_back_to_image` / `mounts_image_before_request` / `copy_to_sources_image_before_request` 等)
- `tests/test_image_ext.rs` を新規作成し、`ImageExt` の `with_*` のうち、`with_cmd([])` の境界等をテストする
- 非空の `env_vars()` / `cmd()` / `mounts()` / `copy_to_sources()` を持つテスト専用 `Image` 実装を `tests/helpers/` に定義し、連結順序・フォールバックを検証する
- `WaitFor` / `Mount` / `ExtraHost` は `PartialEq` を持たないため、`assert_eq!` ではなく match やアクセサ経由の検査で検証する (`CopyToContainer` は match でもアクセサでも検査不能なため件数のみ)
- `image_ext.rs` の rustdoc を変更する 0066 とマージ順に注意する (衝突リスクは低い)
- `CHANGES.md` の `### misc` に `[UPDATE]` エントリを追加する
- テストで発見された既存実装のバグは、本 issue では修正せず bug カテゴリの別 issue を起票する (失敗するテストは、別 issue の起票を確認した上で本 issue から外す) (0058 と同じ方針)
