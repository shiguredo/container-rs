//! Docker Engine API の archive エンドポイントで使う自前 POSIX ustar 実装。
//!
//! regular file と directory (typeflag `'5'`) の書き込み、複数エントリ、`prefix[155]` 分割に
//! 対応する。symlink・特殊ファイル・PAX / GNU 拡張は扱わない。tar / HTTP ともにメモリ完結 (MVP)。
//!
//! 本番の投入は `UstarBuilder` を使う。`build_single_file_ustar` は単一 file の薄いラッパ
//! (単体テスト互換)。`parse_first_regular_file_from_ustar` は先頭エントリが regular file なら
//! その内容を、directory なら `IsDirectory` を返す。

use std::io::{Error as IoError, ErrorKind};

use crate::core::client::docker_client::DOCKER_RESPONSE_BODY_LIMIT;
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
const PREFIX_OFF: usize = 345;
const PREFIX_LEN: usize = 155;

/// name フィールドに収める最大バイト長 (NUL 終端用に 1 バイト空ける)。
const NAME_MAX: usize = NAME_LEN - 1;

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

/// パス名エラーを生成する短縮形。
fn path_err(msg: impl Into<String>) -> CopyToContainerError {
    CopyToContainerError::PathNameError(msg.into())
}

/// `field` (長さ `digits + 1`) に `value` を `digits` 桁の 8 進 ASCII + NUL で書く。
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

/// 相対パスを ustar の `name` / `prefix` に分割する。
///
/// - 全体が `NAME_MAX` 以下なら prefix 空
/// - 超過時は要素境界での合法分割のうち **name が最長**のものを採る
/// - directory の末尾 `/` は常に name 側に残す
fn split_ustar_path(path: &str) -> Result<(String, String), CopyToContainerError> {
    let bytes = path.as_bytes();
    if bytes.is_empty() {
        return Err(path_err("tar entry path must not be empty"));
    }
    if bytes.contains(&0) {
        return Err(path_err("tar entry path must not contain NUL"));
    }
    if bytes.len() <= NAME_MAX {
        return Ok((path.to_string(), String::new()));
    }

    // directory 末尾 `/` は分割点にしないため、探索範囲から末尾の `/` を除く。
    let search_end = if bytes.last() == Some(&b'/') {
        bytes.len() - 1
    } else {
        bytes.len()
    };
    if search_end == 0 {
        return Err(path_err("tar entry path must not be empty"));
    }

    let mut best: Option<(usize, usize)> = None; // (name_len, prefix_len)
    for i in 0..search_end {
        if bytes[i] != b'/' {
            continue;
        }
        let prefix_len = i;
        let name_len = bytes.len() - (i + 1);
        if !(1..=NAME_MAX).contains(&name_len) {
            continue;
        }
        if !(1..=PREFIX_LEN).contains(&prefix_len) {
            continue;
        }
        match best {
            None => best = Some((name_len, prefix_len)),
            Some((best_name, _)) if name_len > best_name => best = Some((name_len, prefix_len)),
            _ => {}
        }
    }

    let Some((name_len, prefix_len)) = best else {
        return Err(path_err(format!(
            "tar entry path too long to fit ustar name/prefix: {} bytes",
            bytes.len()
        )));
    };
    let prefix = path[..prefix_len].to_string();
    let name = path[prefix_len + 1..].to_string();
    debug_assert_eq!(name.len(), name_len);
    Ok((name, prefix))
}

/// ヘッダ 1 ブロックを組み立ててバッファへ追記する。
fn append_header(
    out: &mut Vec<u8>,
    relative_path: &str,
    typeflag: u8,
    size: u64,
    mode: u32,
    uid: u32,
    gid: u32,
) -> Result<(), CopyToContainerError> {
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
    if size > OCTAL_11DIGIT_MAX {
        return Err(overflow_err(format!(
            "data size {size} exceeds ustar 11-octal-digit limit ({OCTAL_11DIGIT_MAX})"
        )));
    }

    let (name, prefix) = split_ustar_path(relative_path)?;
    let name_bytes = name.as_bytes();
    let prefix_bytes = prefix.as_bytes();

    let mut header = [0u8; BLOCK_SIZE];
    header[NAME_OFF..NAME_OFF + name_bytes.len()].copy_from_slice(name_bytes);
    if !prefix_bytes.is_empty() {
        header[PREFIX_OFF..PREFIX_OFF + prefix_bytes.len()].copy_from_slice(prefix_bytes);
    }
    write_octal(
        &mut header[MODE_OFF..MODE_OFF + 8],
        (mode & 0o7777) as u64,
        7,
    );
    write_octal(&mut header[UID_OFF..UID_OFF + 8], uid as u64, 7);
    write_octal(&mut header[GID_OFF..GID_OFF + 8], gid as u64, 7);
    write_octal(&mut header[SIZE_OFF..SIZE_OFF + SIZE_LEN], size, 11);
    write_octal(&mut header[MTIME_OFF..MTIME_OFF + SIZE_LEN], 0, 11);
    header[TYPEFLAG_OFF] = typeflag;
    header[MAGIC_OFF..MAGIC_OFF + 6].copy_from_slice(b"ustar\0");
    header[VERSION_OFF..VERSION_OFF + 2].copy_from_slice(b"00");

    header[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN].copy_from_slice(&[0x20; CHKSUM_LEN]);
    let sum: u32 = header.iter().map(|&b| b as u32).sum();
    write_chksum(&mut header[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN], sum);

    out.extend_from_slice(&header);
    Ok(())
}

/// 複数エントリの ustar archive を構築するビルダー。
///
/// `append_*` のたびにヘッダ (+ データ) をバッファへ書き、`finish` で終端 NUL ブロック 2 個を
/// 一度だけ付ける。複数回の `build_single_file_ustar` 連結はしてはならない。
///
/// archive 全体の蓄積は `DOCKER_RESPONSE_BODY_LIMIT` (64 MiB) を上限とし、超過時は
/// `SizeLimitExceeded` を返す (OOM 防止)。上限はトレーラを含む archive 全体で判定する
/// (`copy_from` の tar 全体 64 MiB 上限と同じ定義)。
pub(crate) struct UstarBuilder {
    buf: Vec<u8>,
}

impl UstarBuilder {
    /// 空のビルダーを返す。
    pub(crate) fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// 追記後の蓄積が 64 MiB 上限を超えないかを検証する。
    ///
    /// 上限を超える場合は `SizeLimitExceeded` を返す。`name` は上限超過を報告する対象名。
    fn check_limit(&self, name: &str) -> Result<(), CopyToContainerError> {
        if self.buf.len() > DOCKER_RESPONSE_BODY_LIMIT {
            return Err(CopyToContainerError::SizeLimitExceeded {
                limit: DOCKER_RESPONSE_BODY_LIMIT,
                name: name.to_string(),
            });
        }
        Ok(())
    }

    /// directory エントリを追記する。`relative_path` は末尾 `/` 必須。
    pub(crate) fn append_directory(
        &mut self,
        relative_path_with_trailing_slash: &str,
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> Result<(), CopyToContainerError> {
        if !relative_path_with_trailing_slash.ends_with('/') {
            return Err(path_err(
                "directory tar entry path must end with a slash".to_string(),
            ));
        }
        append_header(
            &mut self.buf,
            relative_path_with_trailing_slash,
            TYPEFLAG_DIRECTORY,
            0,
            mode,
            uid,
            gid,
        )?;
        self.check_limit(relative_path_with_trailing_slash)
    }

    /// regular file エントリを追記する。
    ///
    /// 追記前に「現バッファ + ヘッダ + データ + pad」の合計で上限超過を判定する
    /// (fail-fast)。`CopyDataSource::Data` は読み込みを伴わずサイズが既知のため、
    /// 事前判定で過渡的に上限超のデータをコピーせずに拒否できる。
    pub(crate) fn append_file(
        &mut self,
        relative_path: &str,
        data: &[u8],
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> Result<(), CopyToContainerError> {
        if relative_path.ends_with('/') {
            return Err(path_err(
                "file tar entry path must not end with a slash".to_string(),
            ));
        }
        let pad = (BLOCK_SIZE - (data.len() % BLOCK_SIZE)) % BLOCK_SIZE;
        // ヘッダ 512 バイト + データ + pad の合計で事前判定する (OOM 防止)。
        let projected = self
            .buf
            .len()
            .checked_add(BLOCK_SIZE)
            .and_then(|n| n.checked_add(data.len()))
            .and_then(|n| n.checked_add(pad))
            .ok_or_else(|| overflow_err("tar size overflow".to_string()))?;
        if projected > DOCKER_RESPONSE_BODY_LIMIT {
            return Err(CopyToContainerError::SizeLimitExceeded {
                limit: DOCKER_RESPONSE_BODY_LIMIT,
                name: relative_path.to_string(),
            });
        }
        append_header(
            &mut self.buf,
            relative_path,
            TYPEFLAG_REGULAR,
            data.len() as u64,
            mode,
            uid,
            gid,
        )?;
        self.buf.extend_from_slice(data);
        let pad = (BLOCK_SIZE - (data.len() % BLOCK_SIZE)) % BLOCK_SIZE;
        self.buf.extend(std::iter::repeat_n(0u8, pad));
        Ok(())
    }

    /// archive 終端 (NUL 512×2) を付けてバイト列を返す。
    pub(crate) fn finish(mut self) -> Result<Vec<u8>, CopyToContainerError> {
        self.buf.extend(std::iter::repeat_n(0u8, BLOCK_SIZE * 2));
        self.check_limit("tar trailer")?;
        Ok(self.buf)
    }
}

/// 単一 regular file の ustar archive を構築する。
///
/// 内部は `UstarBuilder` に 1 file だけ積む薄いラッパ。中間 directory は付けない。
/// 本番の投入は `UstarBuilder` を直接使う。本関数は単体テスト互換用。
#[cfg(test)]
pub(crate) fn build_single_file_ustar(
    name: &str,
    data: &[u8],
    mode: u32,
    uid: u32,
    gid: u32,
) -> Result<Vec<u8>, CopyToContainerError> {
    let mut builder = UstarBuilder::new();
    builder.append_file(name, data, mode, uid, gid)?;
    builder.finish()
}

/// ustar archive の先頭エントリを parse し、regular file ならその内容を返す。
///
/// 先頭エントリが directory なら `IsDirectory`、空 archive なら `EmptyArchive`、未対応
/// typeflag なら `UnsupportedEntry`、チェックサム不整合 / ヘッダ破損なら `Io(InvalidData)`。
/// gzip 応答 (magic `0x1f 0x8b`) は `UnsupportedEntry("gzip")` で拒否する。
pub(crate) fn parse_first_regular_file_from_ustar(
    bytes: &[u8],
) -> Result<Vec<u8>, CopyFromContainerError> {
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        return Err(CopyFromContainerError::UnsupportedEntry("gzip"));
    }
    if bytes.len() < BLOCK_SIZE {
        return Err(invalid_data("truncated tar header"));
    }
    let header = &bytes[..BLOCK_SIZE];
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
        assert_eq!(tar.len(), 512 + 512 + 1024);
        let header = &tar[..512];
        assert_eq!(field_str(header, NAME_OFF, NAME_LEN), b"hello.txt");
        assert_eq!(field_str(header, MODE_OFF, 8), b"0000644");
        assert_eq!(field_str(header, UID_OFF, 8), b"0001750");
        assert_eq!(field_str(header, GID_OFF, 8), b"0001750");
        assert_eq!(field_str(header, SIZE_OFF, SIZE_LEN), b"00000000005");
        assert_eq!(field_str(header, MTIME_OFF, SIZE_LEN), b"00000000000");
        assert_eq!(header[TYPEFLAG_OFF], b'0');
        assert_eq!(&header[MAGIC_OFF..MAGIC_OFF + 6], b"ustar\0");
        assert_eq!(&header[VERSION_OFF..VERSION_OFF + 2], b"00");
        let stored = parse_chksum(&header[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN])
            .expect("chksum が parse できること");
        let mut sum_header = [0u8; 512];
        sum_header.copy_from_slice(header);
        sum_header[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN].copy_from_slice(&[0x20; 8]);
        let computed: u32 = sum_header.iter().map(|&b| b as u32).sum();
        assert_eq!(stored, computed, "チェックサムがヘッダ合計と一致すること");
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
        tar[CHKSUM_OFF] ^= 0xFF;
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
        let mut builder = UstarBuilder::new();
        builder
            .append_directory("dir/", 0o755, 0, 0)
            .expect("directory 追記が成功すること");
        let tar = builder.finish().expect("finish が成功すること");
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
        // 空名・NUL 混入は PathNameError であること。
        assert!(build_single_file_ustar("", b"x", 0o644, 0, 0).is_err());
        assert!(build_single_file_ustar("a\0b", b"x", 0o644, 0, 0).is_err());
        // 単一要素が 100 バイト以上で prefix 分割不能なら拒否。
        let long = "a".repeat(100);
        assert!(build_single_file_ustar(&long, b"x", 0o644, 0, 0).is_err());
    }

    #[test]
    fn build_rejects_uid_gid_overflow() {
        // uid / gid が 8 進 7 桁上限を超えると fail-fast すること。
        let over = (OCTAL_7DIGIT_MAX + 1) as u32;
        assert!(
            build_single_file_ustar("a", b"x", 0o644, over, 0).is_err(),
            "uid 上限超過は拒否されること"
        );
        assert!(
            build_single_file_ustar("a", b"x", 0o644, 0, over).is_err(),
            "gid 上限超過は拒否されること"
        );
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
        tar.truncate(512 + 4);
        let err = parse_first_regular_file_from_ustar(&tar).expect_err("データ切断は失敗すること");
        assert!(
            matches!(err, CopyFromContainerError::Io(_)),
            "truncated data は Io エラーであること: {err}"
        );
    }

    #[test]
    fn parse_invalid_octal_field_is_error() {
        // size フィールドに不正な 8 進文字が入ると Io(InvalidData) であること。
        let mut tar =
            build_single_file_ustar("f", b"x", 0o644, 0, 0).expect("build が成功すること");
        tar[SIZE_OFF] = b'9';
        tar[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN].copy_from_slice(&[0x20; CHKSUM_LEN]);
        let sum: u32 = tar[..512].iter().map(|&b| b as u32).sum();
        write_chksum(&mut tar[CHKSUM_OFF..CHKSUM_OFF + CHKSUM_LEN], sum);
        let err = parse_first_regular_file_from_ustar(&tar).expect_err("不正 8 進は失敗すること");
        assert!(
            matches!(err, CopyFromContainerError::Io(_)),
            "invalid octal は Io エラーであること: {err}"
        );
    }

    #[test]
    fn builder_directory_and_file_layout() {
        // directory → file の順で書き、typeflag / size / 終端が仕様どおりであること。
        let mut builder = UstarBuilder::new();
        builder
            .append_directory("var/", 0o755, 1000, 1000)
            .expect("directory 追記が成功すること");
        builder
            .append_file("var/a.txt", b"hi", 0o644, 1000, 1000)
            .expect("file 追記が成功すること");
        let tar = builder.finish().expect("finish が成功すること");
        // dir ヘッダ 512 + file ヘッダ 512 + data 512 + 終端 1024。
        assert_eq!(tar.len(), 512 + 512 + 512 + 1024);
        assert_eq!(tar[TYPEFLAG_OFF], TYPEFLAG_DIRECTORY);
        assert_eq!(field_str(&tar[..512], NAME_OFF, NAME_LEN), b"var/");
        assert_eq!(field_str(&tar[..512], SIZE_OFF, SIZE_LEN), b"00000000000");
        let file_header = &tar[512..1024];
        assert_eq!(file_header[TYPEFLAG_OFF], TYPEFLAG_REGULAR);
        assert_eq!(field_str(file_header, NAME_OFF, NAME_LEN), b"var/a.txt");
        assert_eq!(&tar[1024..1026], b"hi");
        assert!(
            tar[tar.len() - 1024..].iter().all(|&b| b == 0),
            "終端が NUL 512×2 であること"
        );
    }

    #[test]
    fn split_prefers_longest_name() {
        // 100 バイト超のパスで、最長 name 分割になること。
        let prefix_part = "p".repeat(20);
        let name_part = "n".repeat(90);
        let path = format!("{prefix_part}/{name_part}");
        assert!(path.len() > NAME_MAX);
        let (name, prefix) = split_ustar_path(&path).expect("分割できること");
        assert_eq!(name, name_part);
        assert_eq!(prefix, prefix_part);
        assert!(name.len() <= NAME_MAX);
        assert!(prefix.len() <= PREFIX_LEN);
    }

    #[test]
    fn split_directory_keeps_trailing_slash_in_name() {
        // directory の末尾 `/` が name 側に残ること。
        let prefix_part = "d".repeat(50);
        let mid = "m".repeat(50);
        let path = format!("{prefix_part}/{mid}/");
        assert!(path.len() > NAME_MAX);
        let (name, prefix) = split_ustar_path(&path).expect("分割できること");
        assert!(name.ends_with('/'), "name が末尾 / を持つこと: {name}");
        assert!(
            !prefix.ends_with('/'),
            "prefix に末尾 / を残さないこと: {prefix}"
        );
        assert_eq!(format!("{prefix}/{name}"), path);
    }

    #[test]
    fn builder_rejects_directory_without_slash() {
        // directory パスに末尾 `/` が無いと拒否すること。
        let mut builder = UstarBuilder::new();
        let err = builder
            .append_directory("var", 0o755, 0, 0)
            .expect_err("末尾スラッシュ無しは失敗すること");
        assert!(
            matches!(err, CopyToContainerError::PathNameError(_)),
            "PathNameError であること: {err}"
        );
    }

    #[test]
    fn builder_uses_prefix_for_long_path() {
        // 長い相対パスが prefix フィールドに載ること。
        let prefix_part = "a".repeat(40);
        let name_part = "b".repeat(80);
        let path = format!("{prefix_part}/{name_part}");
        let mut builder = UstarBuilder::new();
        builder
            .append_file(&path, b"x", 0o644, 0, 0)
            .expect("長いパスの追記が成功すること");
        let tar = builder.finish().expect("finish が成功すること");
        let header = &tar[..512];
        assert_eq!(field_str(header, NAME_OFF, NAME_LEN), name_part.as_bytes());
        assert_eq!(
            field_str(header, PREFIX_OFF, PREFIX_LEN),
            prefix_part.as_bytes()
        );
    }

    #[test]
    fn builder_rejects_accumulated_size_over_limit() {
        // 蓄積が 64 MiB を超える追記は SizeLimitExceeded になること。
        // ちょうど 64 MiB のデータは per-file 読み込み上限では成功するが、
        // ヘッダ (512) 分で蓄積が 64 MiB を超えるため追記で失敗する。
        let mut builder = UstarBuilder::new();
        let data = vec![0u8; DOCKER_RESPONSE_BODY_LIMIT];
        let err = builder
            .append_file("big.bin", &data, 0o644, 0, 0)
            .expect_err("蓄積 64 MiB 超の追記は失敗すること");
        assert!(
            matches!(err, CopyToContainerError::SizeLimitExceeded { limit, ref name }
                if limit == DOCKER_RESPONSE_BODY_LIMIT && name == "big.bin"),
            "SizeLimitExceeded で tar エントリ名を持つこと: {err}"
        );
    }

    #[test]
    fn builder_trailer_can_trip_limit() {
        // finish のトレーラ (1024) が加わった時点で 64 MiB を超える場合は、
        // finish で SizeLimitExceeded になること。
        // append 直後の蓄積がちょうど 64 MiB に収まるように、データ量を調整する。
        // ヘッダ (512) + データ + pad = 64 MiB ちょうどになるようにする。
        let mut builder = UstarBuilder::new();
        // 祖先ディレクトリヘッダ (512) + ファイルヘッダ (512) + データ + pad が 64 MiB ちょうど
        // になるようにデータ量を調整する。append は 64 MiB ちょうどで成功し、
        // finish のトレーラで初めて上限を超える。
        let data_len = DOCKER_RESPONSE_BODY_LIMIT - 1024;
        assert_eq!(data_len % 512, 0, "データ長が 512 の倍数であること");
        let data = vec![0u8; data_len];
        builder
            .append_directory("tmp/", 0o755, 0, 0)
            .expect("directory 追記が成功すること");
        builder
            .append_file("tmp/big.bin", &data, 0o644, 0, 0)
            .expect("蓄積が 64 MiB ちょうどの追記が成功すること");
        // ここで蓄積はちょうど 64 MiB のため append は成功する。
        let err = builder
            .finish()
            .expect_err("トレーラで 64 MiB を超える finish は失敗すること");
        assert!(
            matches!(err, CopyToContainerError::SizeLimitExceeded { .. }),
            "SizeLimitExceeded であること: {err}"
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
