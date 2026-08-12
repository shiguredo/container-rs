# バグ: parse_user が空 gid ("1000:") をエラーにする (Docker の仕様と非対称)

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-parse-user-empty-gid
- Polished: {YYYY-MM-DD}

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
- moby の実挙動 (`libcontainer/user` のパース処理) では `"1000:"` は uid のみ指定と同義で、既定 gid (0) が使われる。空 gid はエラーにならない
- 同一クレート内で非対称: Linux 経路 (docker_client.rs) は user 文字列をそのまま daemon へ渡すため `"1000:"` が通る。macOS のみエラーになる
- `"1000:0:0"` のように余剰成分がある場合も `splitn(2, ':')` の都合で gid 側に `"0:0"` が入りエラーになる (moby は余剰成分を無視する)

## 設計方針

- `Some("")` を `None` (gid = 0) と同じ扱いに倒す
- 余剰成分 (`"0:0"` 等) の扱いは moby に合わせて無視する (gid は最初の成分のみパース) か、現状維持にするかを判断して明示する

## 完了条件

- `"1000:"` が macOS でエラーにならず、uid 1000・既定 gid でコンテナが起動すること
- 不正な gid (`"1000:abc"` 等) は従来どおりエラーになること
- 既存の user 指定テストが従来どおり通ること
