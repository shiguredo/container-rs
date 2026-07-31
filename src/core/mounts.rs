//! マウント関連の型。元の testcontainers 0.27 のサブセット（差分の正は `docs/TESTCONTAINERS.md` の該当節 14）。

/// ファイルシステムマウント。
#[derive(Debug, Clone)]
pub struct Mount {
    access_mode: AccessMode,
    mount_type: MountType,
    source: Option<String>,
    target: Option<String>,
    tmpfs_options: Option<MountTmpfsOptions>,
}

/// tmpfs マウントのオプション。
#[derive(Debug, Clone, Default)]
pub struct MountTmpfsOptions {
    /// tmpfs のサイズ (バイト)。
    pub size_bytes: Option<i64>,
    /// tmpfs のパーミッション mode (整数)。
    pub mode: Option<i64>,
}

/// マウントの種類。
#[derive(Debug, Copy, Clone)]
pub enum MountType {
    Bind,
    Volume,
    Tmpfs,
}

// 本家 testcontainers-rs と同じ公開 API。内部では match で直接変換しており本 impl を経由しないが、
// 利用者が to_string() で使う可能性があるため意図的に保持する。
impl std::fmt::Display for MountType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MountType::Bind => write!(f, "bind"),
            MountType::Volume => write!(f, "volume"),
            MountType::Tmpfs => write!(f, "tmpfs"),
        }
    }
}

/// アクセスモード。
#[derive(Debug, Copy, Clone)]
pub enum AccessMode {
    ReadOnly,
    ReadWrite,
}

impl std::fmt::Display for AccessMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccessMode::ReadOnly => write!(f, "ro"),
            AccessMode::ReadWrite => write!(f, "rw"),
        }
    }
}

impl Mount {
    /// バインドマウントを作る。
    pub fn bind_mount(host_path: impl Into<String>, container_path: impl Into<String>) -> Self {
        Self {
            access_mode: AccessMode::ReadWrite,
            mount_type: MountType::Bind,
            source: Some(host_path.into()),
            target: Some(container_path.into()),
            tmpfs_options: None,
        }
    }

    /// 名前付きボリュームマウントを作る。
    pub fn volume_mount(name: impl Into<String>, container_path: impl Into<String>) -> Self {
        Self {
            access_mode: AccessMode::ReadWrite,
            mount_type: MountType::Volume,
            source: Some(name.into()),
            target: Some(container_path.into()),
            tmpfs_options: None,
        }
    }

    /// tmpfs マウントを作る。
    pub fn tmpfs_mount(container_path: impl Into<String>) -> Self {
        Self {
            access_mode: AccessMode::ReadWrite,
            mount_type: MountType::Tmpfs,
            source: None,
            target: Some(container_path.into()),
            tmpfs_options: None,
        }
    }

    /// アクセスモードを設定する。
    pub fn with_access_mode(mut self, access_mode: AccessMode) -> Self {
        self.access_mode = access_mode;
        self
    }

    pub fn access_mode(&self) -> AccessMode {
        self.access_mode
    }

    pub fn mount_type(&self) -> MountType {
        self.mount_type
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// tmpfs のサイズをバイトで設定する。
    ///
    /// tmpfs 以外のマウント種別では XPC 反映時に無視する。
    pub fn with_size_bytes(mut self, size: i64) -> Self {
        self.tmpfs_options
            .get_or_insert_with(MountTmpfsOptions::default)
            .size_bytes = Some(size);
        self
    }

    /// tmpfs のサイズを人間可読形式で設定する。
    ///
    /// 対応形式: `"100k"` / `"100K"` (1000 バイト単位)、`"100m"` / `"100M"`、`"100g"` / `"100G"`。
    /// 単位なしはバイト。パース失敗時は panic する。
    pub fn with_size(self, size: &str) -> Self {
        let bytes = parse_size(size).expect("Invalid size format");
        self.with_size_bytes(bytes)
    }

    /// tmpfs のパーミッション mode を設定する。
    ///
    /// tmpfs 以外のマウント種別では XPC 反映時に無視する。
    pub fn with_mode(mut self, mode: i64) -> Self {
        self.tmpfs_options
            .get_or_insert_with(MountTmpfsOptions::default)
            .mode = Some(mode);
        self
    }

    /// 設定済みの tmpfs オプションを返す。
    pub fn tmpfs_options(&self) -> Option<&MountTmpfsOptions> {
        self.tmpfs_options.as_ref()
    }
}

/// 人間可読なサイズ文字列をバイト数へ変換する。
///
/// 対応: k/K (1000)、m/M (10^6)、g/G (10^9)。単位なしはバイト。
fn parse_size(size: &str) -> Result<i64, String> {
    let size = size.trim();
    if size.is_empty() {
        return Err("Size string is empty".to_string());
    }

    let (number_part, unit) = if let Some(stripped) = size.strip_suffix(['k', 'K']) {
        (stripped, 1_000)
    } else if let Some(stripped) = size.strip_suffix(['m', 'M']) {
        (stripped, 1_000_000)
    } else if let Some(stripped) = size.strip_suffix(['g', 'G']) {
        (stripped, 1_000_000_000)
    } else {
        (size, 1)
    };

    let number: i64 = number_part
        .trim()
        .parse()
        .map_err(|e| format!("Failed to parse number '{number_part}': {e}"))?;

    if number < 0 {
        return Err("Size cannot be negative".to_string());
    }

    number
        .checked_mul(unit)
        .ok_or_else(|| "Size value overflows i64".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_and_accessors_preserve_mount_configuration() {
        // 各コンストラクタがマウント種別・入出力パス・既定の読み書きモードを保つこと。
        let bind = Mount::bind_mount("/host/bind", "/container/bind");
        assert!(matches!(bind.mount_type(), MountType::Bind));
        assert!(matches!(bind.access_mode(), AccessMode::ReadWrite));
        assert_eq!(bind.source(), Some("/host/bind"));
        assert_eq!(bind.target(), Some("/container/bind"));
        assert!(bind.tmpfs_options().is_none());

        let volume = Mount::volume_mount("data", "/container/volume");
        assert!(matches!(volume.mount_type(), MountType::Volume));
        assert!(matches!(volume.access_mode(), AccessMode::ReadWrite));
        assert_eq!(volume.source(), Some("data"));
        assert_eq!(volume.target(), Some("/container/volume"));

        let tmpfs = Mount::tmpfs_mount("/container/tmpfs");
        assert!(matches!(tmpfs.mount_type(), MountType::Tmpfs));
        assert!(matches!(tmpfs.access_mode(), AccessMode::ReadWrite));
        assert_eq!(tmpfs.source(), None);
        assert_eq!(tmpfs.target(), Some("/container/tmpfs"));
        assert!(tmpfs.tmpfs_options().is_none());
    }

    #[test]
    fn access_mode_and_display_are_reflected() {
        // 読み取り専用指定と、公開型が表す文字列表現を確認する。
        let mount = Mount::bind_mount("/host/read-only", "/container/read-only")
            .with_access_mode(AccessMode::ReadOnly);
        assert!(matches!(mount.access_mode(), AccessMode::ReadOnly));
        assert_eq!(MountType::Bind.to_string(), "bind");
        assert_eq!(MountType::Volume.to_string(), "volume");
        assert_eq!(MountType::Tmpfs.to_string(), "tmpfs");
        assert_eq!(AccessMode::ReadOnly.to_string(), "ro");
        assert_eq!(AccessMode::ReadWrite.to_string(), "rw");
    }

    #[test]
    fn tmpfs_options_setters_store_size_and_mode() {
        // with_size_bytes / with_size / with_mode が MountTmpfsOptions に残ること。
        let mount = Mount::tmpfs_mount("/tmp")
            .with_size_bytes(1_000_000_000)
            .with_mode(0o1777);
        let opts = mount.tmpfs_options().expect("tmpfs オプションがあること");
        assert_eq!(opts.size_bytes, Some(1_000_000_000));
        assert_eq!(opts.mode, Some(0o1777));

        let by_string = Mount::tmpfs_mount("/tmp").with_size("20g");
        assert_eq!(
            by_string.tmpfs_options().and_then(|o| o.size_bytes),
            Some(20_000_000_000)
        );
    }

    #[test]
    fn parse_size_accepts_units_and_rejects_invalid() {
        // 本家と同じ単位換算と、不正入力の拒否を確認する。
        assert_eq!(parse_size("100k").expect("パースできること"), 100_000);
        assert_eq!(parse_size("100M").expect("パースできること"), 100_000_000);
        assert_eq!(parse_size("1g").expect("パースできること"), 1_000_000_000);
        assert_eq!(parse_size("12345").expect("パースできること"), 12_345);
        assert!(parse_size("").is_err());
        assert!(parse_size("abc").is_err());
        assert!(parse_size("-1m").is_err());
    }
}
