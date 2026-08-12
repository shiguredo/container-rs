# バグ: parse_user が空 gid ("1000:") をエラーにする (Docker の仕様と非対称)

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-parse-user-empty-gid
- Polished: 2026-08-12

## 目的

`with_user("1000:")` のような空 gid 付き user 指定が macOS でエラーになるのを、Docker (moby) の挙動に合わせて受理するようにする。

## 現状

`src/core/client/container_cfg.rs` の `parse_user` は `"uid:gid"` を `splitn(2, ':')` で分割し、gid 側のパースに失敗するとエラーを返す。

```rust
let gid = match gid_str {
    Some(s) => s.parse::<u32>().map_err(|_| ...invalid gid in user string...)?,
    None => 0,
};
```

- `"1000:"` は `gid_str = Some("")` となり `parse::<u32>()` が失敗して `invalid gid in user string` エラーになる
- moby の実挙動 (`moby/sys/user` の `GetExecUser`。runc の `libcontainer/user` からフォーク) では `"1000:"` は uid のみ指定と同義で、既定 gid (0) が使われる。空 gid はエラーにならない
- 同一クレート内で非対称: Linux 経路 (docker_client.rs) は user 文字列をそのまま daemon へ渡すため `"1000:"` が通る。macOS のみエラーになる
- `"1000:0:0"` のように余剰成分がある場合も `splitn(2, ':')` の都合で gid 側に `"0:0"` が入りエラーになる (余剰成分の扱いは moby のバージョンによって異なる: 旧実装は無視、最新の `moby/sys/user` はグループ名として解決してエラー。本 issue では余剰成分は現状維持とする。詳細は設計方針)

## 設計方針

- `Some("")` を `None` (gid = 0) と同じ扱いに倒す (moby が空 gid を指定なしとして扱い既定 gid を使う挙動と一致)。実装形は任意 (例: `Some(s) if !s.is_empty()` ガード)
- 余剰成分 (`"1000:0:0"` 等) は現状維持 (エラー) とする (moby の実挙動はバージョンによって異なり、macOS 側でコンテナ内 `/etc/group` のグループ名解決を実装する手段も無いため)
- グループ名 gid (`"1000:abc"` 等) の解決はスコープ外 (moby はコンテナ内 `/etc/group` から解決するが、macOS 側では参照できない)
- 空 uid (`":1000"` 等) は対象外 (従来どおり `ProcessUser::Raw` へフォールバックする)

## 完了条件

- `"1000:"` が macOS でエラーにならず、`ProcessUser::Id { uid: 1000, gid: 0 }` になること (既存の `user_numeric_is_reflected_in_container_cfg_json` と同型の JSON 反映テストを新規追加して検証する。非 root UID での実起動は Apple container の環境制約で検証できない)
- 不正な gid (`"1000:abc"` 等) は従来どおりエラーになること (エラーケースの検証テストを新規追加する)
- 余剰成分 (`"1000:0:0"` 等) は従来どおりエラーになること (エラーケースの検証テストを新規追加する)
- 既存の user 指定テストが従来どおり通ること
- 修正で陳腐化する `parse_user` の doc コメントが更新されること
- `CHANGES.md` に `[FIX]` エントリが記載されること
