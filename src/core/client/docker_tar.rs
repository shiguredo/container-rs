//! Docker Engine API の archive エンドポイントで使う自前 POSIX ustar 実装。
//!
//! 対象は単一 regular file のみ。ディレクトリ・symlink・特殊ファイル・PAX / GNU 拡張は
//! 扱わない。tar / HTTP ともにメモリ完結 (MVP)。
//!
//! `build_single_file_ustar` は 1 エントリの ustar を構築し、`parse_first_regular_file_from_ustar`
//! は先頭エントリが regular file ならその内容を、directory なら `IsDirectory` を返す。

use std::io::{Error as IoError, ErrorKind};

use crate::core::copy::{CopyFromContainerError, CopyToContainerError};

/// tar ブロックサイズ (バイト)。
const BLOCK_SIZE: usize = 512;

/// ustar ヘッダ内のフィールドオフセットと長さ。
const NAME_OFF: usize = 0;
const NAME_LEN: usize = 100;
const MODE_OFF: usize = 100;
const UID_OFF: usize = 108;
const GID_OFF: usize = 116;
const SIZE_OFF: usize = 124;
const SIZE_LEN: usize = 12;
const MTIME_OFF: usize = 136;
const CHKSUM_OFF: usize = 148;
const CHKSUM_LEN: usize = 8;
const TYPEFLAG_OFF: usize = 156;
const MAGIC_OFF: usize = 257;
const VERSION_OFF: usize = 263;

/// typeflag: regular file (NUL または '0')。
const TYPEFLAG_REGULAR: u8 = b'0';
/// typeflag: directory。
const TYPEFLAG_DIRECTORY: u8 = b'5';

/// ustar の uid / gid フィールド上限 (8 進 7 桁)。
const OCTAL_7DIGIT_MAX: u64 = 0o7777777;
/// ustar の size フィールド上限 (8 進 11 桁)。
const OCTAL_11DIGIT_MAX: u64 = 0o77777777777;

/// 数値フィールドの桁溢れを InvalidData で生成する短縮形。
fn overflow_err(msg: String) -> CopyToContainerError {
    CopyToContainerError::IoError(IoError::new(ErrorKind::InvalidData, msg))
}

/// invalid data エラーを生成する短縮形。
fn invalid_data(msg: &'static str) -> CopyFromContainerError {
    CopyFromContainerError::Io(IoError::new(ErrorKind::InvalidData, msg))
}

/// `field` (長さ `digits + 1`) に `value` を `digits` 桁の 8 進 ASCII + NUL で書く。
///
/// `value` が `digits` 桁に収まらない場合は下位桁が優先され上位は切り詰められる
/// (mode / uid / gid / size はそれぞれ既定の桁数に収まる値を想定)。
fn write_octal(field: &mut [u8], value: u64, digits: usize) {
    let mut v = value;
    for i in (0..digits).rev() {
        field[i] = b'0' + (v & 0o7) as u8;
        v >>= 3;
    }
    field[digits] = 0;
}

/// `field` (8 バイト) にチェックサムを 6 桁 8 進 ASCII + NUL + 空白で書く。
fn write_chksum(field: &mut [u8], sum: u32) {
    let mut v = sum;
    for i in (0..6).rev() {
        field[i] = b'0' + (v & 0o7) as u8;
        v >>= 3;
    }
    field[6] = 0;
    field[7] = 0x20;
}

/// 8 進 ASCII フィールドを parse する。NUL または空白で停止する。
fn parse_octal(field: &[u8]) -> Result<u64, CopyFromContainerError> {
    let mut value: u64 = 0;
    for &b in field {
        if b == 0 || b == 0x20 {
            break;
        }
        if !(b'0'..=b'7').contains(&b) {
            return Err(invalid_data("invalid octal field"));
        }
        value = value
            .checked_mul(8)
            .and_then(|v| v.checked_add((b - b'0') as u64))
            .ok_or_else(|| invalid_data("octal field overflow"))?;
    }
    Ok(value)
}

/// チェックサムフィールド (8 バイト) を parse する。NUL または空白で停止する。
fn parse_chksum(field: &[u8]) -> Result<u32, CopyFromContainerError> {
    let mut value: u32 = 0;
    for &b in field {
        if b == 0 || b == 0x20 {
            break;
        }
        if !(b'0'..=b'7').contains(&b) {
            return Err(invalid_data("invalid checksum field"));
        }
        value = value
            .checked_mul(8)
            .and_then(|v| v.checked_add((b - b'0') as u32))
            .ok_or_else(|| invalid_data("checksum field overflow"))?;
    }
    Ok(value)
}

/// 単一 regular file の ustar archive を構築する。
///
/// `name` は basename (Docker の path クエリで dirname は別送するため 99 バイト以下)。
/// `mtime` は再現性のため常に 0 (エポック)。archive 終端は 512 バイト NUL ブロック 2 個。
pub(crate) fn build_single_file_ustar(
    name: &str,
    data: &[u8],
    mode: u32,
    uid: u32,
    gid: u32,
) -> Result<Vec<u8>, CopyToContainerError> {
    let name_bytes = name.as_bytes();
    if name_bytes.is_empty() {
        return Err(CopyToContainerError::PathNameError(
            "tar entry name must not be empty".to_string(),
        ));
    }
    // 99 バイト以下を許容、100 以上は拒否 (NUL 終端を強く要求する実装との相互運用性優先)。
    if name_bytes.len() >= NAME_LEN {
        return Err(CopyToContainerError::PathNameError(format!(
            "tar entry name too long: {} bytes (max 99)",
            name_bytes.len()
        )));
    }
    if name_bytes.contains(&0) {
        return Err(CopyToContainerError::PathNameError(
            "tar entry name must not contain NUL".to_string(),
        ));
    }
    // ustar の uid / gid は 8 進 7 桁、size は 8 進 11 桁が上限。超過はヘッダの暗黙桁詰め
    // (メタデータ / データ破損) になるため、公開入力の境界で fail-fast する。
    if (uid as u64) > OCTAL_7DIGIT_MAX {
        return Err(overflow_err(format!(
            "uid {uid} exceeds ustar 7-octal-digit limit ({OCTAL_7DIGIT_MAX})"
        )));
    }
    if (gid as u64) > OCTAL_7DIGIT_MAX {
        return Err(overflow_err(format!(
            "gid {gid} exceeds ustar 7-octal-digit limit ({OCTAL_7DIGIT_MAX})"
        )));
    }
    if (data.len() as u64) > OCTAL_11DIGIT_MAX {
        return Err(overflow_err(format!(
            "data size {} exceeds ustar 11-octal-digit limit ({OCTAL_11DIGIT_MAX})",
            data.len()
        )));
    }

    let mut header = [0u8; BLOCK_SIZE];
    header[NAME_OFF..NAME_OFF + name_bytes.len()].copy_from_slice(name_bytes);
    // mode は下位 12 ビットのみ。
    write_octal(
        &mut header[MODE_OFF..MODE_OFF + 8],
        (mode & 0o7777) as u64,
        7,
    );
    write_octal(&mut header[UID_OFF..UID_OFF + 8], uid as u64, 7);
    write_octal(&mut header[GID_OFF..GID_OFF + 8], gid as u64, 7);
    write_octal(
        &mut header[SIZE_OFF..SIZE_OFF + SIZE_LEN],
        data.len() as u64,
        11,
    );
    write_octal(&mut header[MTIME_OFF..MTIME_OFF + SIZE_LEN], 0, 11);
    header[TYPEFLAG_OFF] = TYPEFLAG_REGULAR;
    header[MAGIC_OFF..MAGIC_OFF + 6].copy_from_slice(b"ustar\0");
    header[VERSION_OFF..VERSION_OFF + 2].copy_from_slice(b"00");
    // uname / gname は空 (uid / gid を優先)。devmajor / devminor / prefix は未使用 (0 埋め)。

    // チェックサム: chksum フィールドを空白で埋めた状態の 512 バイト全体の符号なし合計。
    header[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN].copy_from_slice(&[0x20; CHKSUM_LEN]);
    let sum: u32 = header.iter().map(|&b| b as u32).sum();
    write_chksum(&mut header[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN], sum);

    let mut out = Vec::new();
    out.extend_from_slice(&header);
    out.extend_from_slice(data);
    // データ本体を 512 バイト境界まで NUL パディング。
    let pad = (BLOCK_SIZE - (data.len() % BLOCK_SIZE)) % BLOCK_SIZE;
    out.extend(std::iter::repeat_n(0u8, pad));
    // archive 終端: 512 バイト NUL ブロック 2 個。
    out.extend(std::iter::repeat_n(0u8, BLOCK_SIZE * 2));
    Ok(out)
}

/// ustar archive の先頭エントリを parse し、regular file ならその内容を返す。
///
/// 先頭エントリが directory なら `IsDirectory`、空 archive なら `EmptyArchive`、未対応
/// typeflag なら `UnsupportedEntry`、チェックサム不整合 / ヘッダ破損なら `Io(InvalidData)`。
/// gzip 応答 (magic `0x1f 0x8b`) は `UnsupportedEntry("gzip")` で拒否する。
pub(crate) fn parse_first_regular_file_from_ustar(
    bytes: &[u8],
) -> Result<Vec<u8>, CopyFromContainerError> {
    // gzip magic の検出 (Content-Encoding ヘッダを見なくても拒否できるようにする)。
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        return Err(CopyFromContainerError::UnsupportedEntry("gzip"));
    }
    if bytes.len() < BLOCK_SIZE {
        return Err(invalid_data("truncated tar header"));
    }
    let header = &bytes[..BLOCK_SIZE];
    // 先頭ブロックが全 NUL ならエントリ無し (空 archive)。
    if header.iter().all(|&b| b == 0) {
        return Err(CopyFromContainerError::EmptyArchive);
    }

    match header[TYPEFLAG_OFF] {
        TYPEFLAG_DIRECTORY => return Err(CopyFromContainerError::IsDirectory),
        b'\0' | TYPEFLAG_REGULAR => {}
        b'1' => return Err(CopyFromContainerError::UnsupportedEntry("hardlink")),
        b'2' => return Err(CopyFromContainerError::UnsupportedEntry("symlink")),
        b'3' => return Err(CopyFromContainerError::UnsupportedEntry("char device")),
        b'4' => return Err(CopyFromContainerError::UnsupportedEntry("block device")),
        b'6' => return Err(CopyFromContainerError::UnsupportedEntry("fifo")),
        _ => return Err(CopyFromContainerError::UnsupportedEntry("unknown typeflag")),
    }

    // チェックサム検証: chksum フィールドを空白で埋め直して合計を再計算し、数値比較する。
    let stored = parse_chksum(&header[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN])?;
    let mut sum_header = [0u8; BLOCK_SIZE];
    sum_header.copy_from_slice(header);
    sum_header[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN].copy_from_slice(&[0x20; CHKSUM_LEN]);
    let computed: u32 = sum_header.iter().map(|&b| b as u32).sum();
    if stored != computed {
        return Err(invalid_data("tar header checksum mismatch"));
    }

    let size = usize::try_from(parse_octal(&header[SIZE_OFF..SIZE_OFF + SIZE_LEN])?)
        .map_err(|_| invalid_data("tar size does not fit in usize"))?;
    let data_start = BLOCK_SIZE;
    let data_end = data_start
        .checked_add(size)
        .ok_or_else(|| invalid_data("tar size overflow"))?;
    if bytes.len() < data_end {
        return Err(invalid_data("truncated tar data"));
    }
    // Vec::with_capacity は使わない (破損入力の size による OOM 回避。shiguredo-rust 規約)。
    let mut out = Vec::new();
    out.extend_from_slice(&bytes[data_start..data_end]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ヘッダの指定オフセットのフィールドを NUL 終端まで取り出す。
    fn field_str(header: &[u8], off: usize, len: usize) -> &[u8] {
        let f = &header[off..off + len];
        let end = f.iter().position(|&b| b == 0).unwrap_or(len);
        &f[..end]
    }

    #[test]
    fn build_matches_known_field_layout() {
        // 既知の入力についてヘッダ各フィールドが仕様どおりのバイト列になること (ゴールデン)。
        let tar = build_single_file_ustar("hello.txt", b"world", 0o644, 1000, 1000)
            .expect("build が成功すること");
        // archive = ヘッダ 512 + データ 512 (パディング込み) + 終端 1024。
        assert_eq!(tar.len(), 512 + 512 + 1024);
        let header = &tar[..512];
        assert_eq!(field_str(header, NAME_OFF, NAME_LEN), b"hello.txt");
        assert_eq!(field_str(header, MODE_OFF, 8), b"0000644");
        assert_eq!(field_str(header, UID_OFF, 8), b"0001750"); // 1000 = 0o1750
        assert_eq!(field_str(header, GID_OFF, 8), b"0001750");
        assert_eq!(field_str(header, SIZE_OFF, SIZE_LEN), b"00000000005"); // 5 バイト
        assert_eq!(field_str(header, MTIME_OFF, SIZE_LEN), b"00000000000");
        assert_eq!(header[TYPEFLAG_OFF], b'0');
        assert_eq!(&header[MAGIC_OFF..MAGIC_OFF + 6], b"ustar\0");
        assert_eq!(&header[VERSION_OFF..VERSION_OFF + 2], b"00");
        // チェックサムがヘッダ合計と一致すること。
        let stored = parse_chksum(&header[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN])
            .expect("chksum が parse できること");
        let mut sum_header = [0u8; 512];
        sum_header.copy_from_slice(header);
        sum_header[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN].copy_from_slice(&[0x20; 8]);
        let computed: u32 = sum_header.iter().map(|&b| b as u32).sum();
        assert_eq!(stored, computed, "チェックサムがヘッダ合計と一致すること");
        // データ本体。
        assert_eq!(&tar[512..517], b"world");
    }

    #[test]
    fn round_trip_simple() {
        // build → parse で元データが復元されること。
        let tar = build_single_file_ustar("a.bin", b"\x00\x01\x02binary", 0o600, 0, 0)
            .expect("build が成功すること");
        let parsed = parse_first_regular_file_from_ustar(&tar).expect("parse が成功すること");
        assert_eq!(parsed, b"\x00\x01\x02binary");
    }

    #[test]
    fn parse_rejects_corrupted_checksum() {
        // チェックサム破損入力は InvalidData で失敗すること。
        let mut tar =
            build_single_file_ustar("a", b"data", 0o644, 0, 0).expect("build が成功すること");
        tar[CHKSUM_OFF] ^= 0xFF; // チェックサムを破壊
        let err =
            parse_first_regular_file_from_ustar(&tar).expect_err("破損チェックサムは失敗すること");
        assert!(
            matches!(err, CopyFromContainerError::Io(_)),
            "チェックサム不整合は Io エラーであること: {err}"
        );
    }

    #[test]
    fn parse_empty_archive_is_empty_archive_error() {
        // 全 NUL の先頭ブロックは EmptyArchive であること。
        let empty = vec![0u8; 1024];
        let err =
            parse_first_regular_file_from_ustar(&empty).expect_err("空 archive は失敗すること");
        assert!(
            matches!(err, CopyFromContainerError::EmptyArchive),
            "空 archive は EmptyArchive であること: {err}"
        );
    }

    #[test]
    fn parse_directory_entry_is_is_directory() {
        // typeflag '5' の先頭エントリは IsDirectory であること。
        let mut tar =
            build_single_file_ustar("dir", b"", 0o755, 0, 0).expect("build が成功すること");
        tar[TYPEFLAG_OFF] = TYPEFLAG_DIRECTORY;
        // typeflag 変更でチェックサムがずれるため再計算する。
        tar[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN].copy_from_slice(&[0x20; CHKSUM_LEN]);
        let sum: u32 = tar[..512].iter().map(|&b| b as u32).sum();
        write_chksum(&mut tar[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN], sum);
        let err = parse_first_regular_file_from_ustar(&tar).expect_err("directory は失敗すること");
        assert!(
            matches!(err, CopyFromContainerError::IsDirectory),
            "directory エントリは IsDirectory であること: {err}"
        );
    }

    #[test]
    fn parse_gzip_is_unsupported() {
        // gzip magic で始まる応答は UnsupportedEntry("gzip") であること。
        let gzip = vec![0x1f, 0x8b, 0x08, 0x00];
        let err = parse_first_regular_file_from_ustar(&gzip).expect_err("gzip は失敗すること");
        assert!(
            matches!(err, CopyFromContainerError::UnsupportedEntry("gzip")),
            "gzip は UnsupportedEntry(gzip) であること: {err}"
        );
    }

    #[test]
    fn build_rejects_invalid_name() {
        // 空名・100 バイト以上・NUL 混入は PathNameError であること。
        assert!(build_single_file_ustar("", b"x", 0o644, 0, 0).is_err());
        let long = "a".repeat(100);
        assert!(build_single_file_ustar(&long, b"x", 0o644, 0, 0).is_err());
        assert!(build_single_file_ustar("a\0b", b"x", 0o644, 0, 0).is_err());
    }

    #[test]
    fn build_rejects_uid_gid_overflow() {
        // uid / gid が 8 進 7 桁上限 (0o7777777) を超えると fail-fast すること (F1 の回帰)。
        let over = (OCTAL_7DIGIT_MAX + 1) as u32;
        assert!(
            build_single_file_ustar("a", b"x", 0o644, over, 0).is_err(),
            "uid 上限超過は拒否されること"
        );
        assert!(
            build_single_file_ustar("a", b"x", 0o644, 0, over).is_err(),
            "gid 上限超過は拒否されること"
        );
        // 上限ちょうどは許容されること。
        let max = OCTAL_7DIGIT_MAX as u32;
        assert!(build_single_file_ustar("a", b"x", 0o644, max, max).is_ok());
    }

    /// typeflag を書き換え、チェックサムを再計算する (特殊 typeflag テスト用ヘルパ)。
    fn set_typeflag(tar: &mut [u8], flag: u8) {
        tar[TYPEFLAG_OFF] = flag;
        tar[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN].copy_from_slice(&[0x20; CHKSUM_LEN]);
        let sum: u32 = tar[..512].iter().map(|&b| b as u32).sum();
        write_chksum(&mut tar[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN], sum);
    }

    #[test]
    fn parse_special_typeflags_are_unsupported() {
        // hardlink / symlink / device / fifo は UnsupportedEntry であること。
        for (flag, expected) in [
            (b'1', "hardlink"),
            (b'2', "symlink"),
            (b'3', "char device"),
            (b'4', "block device"),
            (b'6', "fifo"),
        ] {
            let mut tar =
                build_single_file_ustar("f", b"x", 0o644, 0, 0).expect("build が成功すること");
            set_typeflag(&mut tar, flag);
            let err = parse_first_regular_file_from_ustar(&tar)
                .expect_err("特殊 typeflag は失敗すること");
            assert!(
                matches!(err, CopyFromContainerError::UnsupportedEntry(name) if name == expected),
                "typeflag {flag} は UnsupportedEntry({expected}) であること: {err}"
            );
        }
    }

    #[test]
    fn parse_truncated_header_is_error() {
        // 512 バイト未満の入力は Io(InvalidData) であること。
        let short = vec![0u8; 100];
        let err = parse_first_regular_file_from_ustar(&short).expect_err("短い入力は失敗すること");
        assert!(
            matches!(err, CopyFromContainerError::Io(_)),
            "truncated header は Io エラーであること: {err}"
        );
    }

    #[test]
    fn parse_truncated_data_is_error() {
        // ヘッダの size より実データが短い場合は Io(InvalidData) であること。
        let mut tar =
            build_single_file_ustar("f", b"0123456789", 0o644, 0, 0).expect("build が成功すること");
        tar.truncate(512 + 4); // データ本体を途中で切断
        let err = parse_first_regular_file_from_ustar(&tar).expect_err("データ切断は失敗すること");
        assert!(
            matches!(err, CopyFromContainerError::Io(_)),
            "truncated data は Io エラーであること: {err}"
        );
    }

    #[test]
    fn parse_invalid_octal_field_is_error() {
        // size フィールドに不正な 8 進文字 (例: '9') が入ると Io(InvalidData) であること。
        let mut tar =
            build_single_file_ustar("f", b"x", 0o644, 0, 0).expect("build が成功すること");
        tar[SIZE_OFF] = b'9'; // 不正な 8 進数字
        // チェックサムを再計算してチェックサム検証は通過させる。
        tar[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN].copy_from_slice(&[0x20; CHKSUM_LEN]);
        let sum: u32 = tar[..512].iter().map(|&b| b as u32).sum();
        write_chksum(&mut tar[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN], sum);
        let err = parse_first_regular_file_from_ustar(&tar).expect_err("不正 8 進は失敗すること");
        assert!(
            matches!(err, CopyFromContainerError::Io(_)),
            "invalid octal は Io エラーであること: {err}"
        );
    }

    mod pbt {
        use super::super::*;
        use proptest::prelude::*;

        proptest! {
            /// 任意の (name, data, mode, uid, gid) について encode → decode で元データが復元されること。
            #[test]
            fn round_trip_single_file(
                name in "[a-zA-Z0-9_.-]{1,99}",
                data in proptest::collection::vec(any::<u8>(), 0..2048),
                mode in 0u32..0o7777,
                uid in 0u32..0o7777777,
                gid in 0u32..0o7777777,
            ) {
                let tar = build_single_file_ustar(&name, &data, mode, uid, gid)
                    .expect("build が成功すること");
                let parsed = parse_first_regular_file_from_ustar(&tar)
                    .expect("parse が成功すること");
                prop_assert_eq!(parsed, data);
            }
        }
    }
}
