//! クレート内で共通に使うユーティリティ関数。

/// プロセスユニークなサフィックスを生成する。
///
/// ナノ秒タイムスタンプだけでは実解像度 (macOS はマイクロ秒程度) の範囲で
/// 並列 `start()` が同じ値になり、コンテナ ID が衝突する。
/// 同一プロセス内はアトミックカウンタで、プロセス間はプロセス ID で一意性を担保する。
pub(crate) fn unique_suffix() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{}-{}", std::process::id(), nanos, count)
}

/// コンテナ ID が Apple container 1.2.0 の `nameValid` 相当の制約を満たすか検証する。
///
/// Apple container 1.2.0 (`ContainersHarness`) は bootstrap / create / delete / diskUsage /
/// logs / export の ID に対し `ManagedContainer.nameValid` を enforce する。その規則:
///
/// - 長さが 63 以下
/// - 正規表現 `^[a-zA-Z0-9][a-zA-Z0-9_.-]+$` (先頭は英数字、以降は英数字 / `_` / `.` / `-`、
///   実質 2 文字以上)
///
/// 文字種が ASCII 限定のため、バイト列 (`len()` / バイト走査) で実装しても文字数判定と
/// 同値になる (非 ASCII は文字種チェックで落ちる)。watchdog の reaper への ID 登録もこの
/// 関数で検証し、空文字・空白・改行・glob 文字の拒否によるシェル安全性を維持する。
pub(crate) fn is_valid_container_id(id: &str) -> bool {
    if id.len() < 2 || id.len() > 63 {
        return false;
    }
    let bytes = id.as_bytes();
    if !bytes[0].is_ascii_alphanumeric() {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_id_accepts_valid_ids() {
        // デフォルト生成 ID 形式と一般的な名前が適合すること。
        assert!(
            is_valid_container_id("c-1-2-3"),
            "デフォルト形式の ID は適合するべき"
        );
        assert!(
            is_valid_container_id("my_container.test-01"),
            "許可文字のみの名前は適合するべき"
        );
        assert!(
            is_valid_container_id("A1"),
            "英数字 2 文字以上は適合するべき"
        );
        assert!(
            is_valid_container_id("1a"),
            "数字先頭の 2 文字 ID も適合するべき"
        );
    }

    #[test]
    fn container_id_rejects_invalid_ids() {
        // 空文字・1 文字・長さ超過・先頭文字種・非許可文字を拒否すること。
        assert!(!is_valid_container_id(""), "空文字は拒否するべき");
        assert!(!is_valid_container_id("A"), "1 文字の ID は拒否するべき");
        assert!(!is_valid_container_id("1"), "1 文字の ID は拒否するべき");
        assert!(
            !is_valid_container_id("-abc"),
            "先頭が英数字でない ID は拒否するべき"
        );
        assert!(
            !is_valid_container_id("_abc"),
            "先頭が英数字でない ID は拒否するべき"
        );
        assert!(
            !is_valid_container_id(".abc"),
            "先頭が英数字でない ID は拒否するべき"
        );
        assert!(
            !is_valid_container_id("has space"),
            "空白を含む ID は拒否するべき"
        );
        assert!(
            !is_valid_container_id("glob*"),
            "glob 文字を含む ID は拒否するべき"
        );
        assert!(
            !is_valid_container_id("line\nbreak"),
            "改行を含む ID は拒否するべき"
        );
        assert!(
            !is_valid_container_id("q?"),
            "クエスチョンマークを含む ID は拒否するべき"
        );
        assert!(
            !is_valid_container_id("a/b"),
            "スラッシュを含む ID は拒否するべき"
        );
        assert!(
            !is_valid_container_id("あいう"),
            "非 ASCII の ID は拒否するべき"
        );
    }

    #[test]
    fn container_id_boundary_length() {
        // ちょうど 63 文字は許可、64 文字は拒否されること。
        let exactly_63 = "a".repeat(63);
        assert!(
            is_valid_container_id(&exactly_63),
            "ちょうど 63 文字は許可されるべき"
        );
        let over_64 = "a".repeat(64);
        assert!(!is_valid_container_id(&over_64), "64 文字は拒否されるべき");
    }

    #[test]
    fn auto_generated_container_id_passes_validation() {
        // 自動生成 ID (c-{pid}-{nanos}-{count}) が nameValid の全条件 (長さ上限含む) を通ること。
        let id = format!("c-{}", unique_suffix());
        assert!(
            is_valid_container_id(&id),
            "自動生成 ID は適合するべき: {id}"
        );
        assert!(id.len() <= 63, "自動生成 ID は 63 文字以下であるべき: {id}");
    }
}
