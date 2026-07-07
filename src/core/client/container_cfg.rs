//! XPC `containerCreate` 向けの JSON ペイロードモデルと `build_config`。
//!
//! `AsyncRunner` が `ContainerRequest` から構築し、`XpcClient::create_container` に渡す。

use std::collections::HashMap;

use nojson::DisplayJson;

use crate::{
    ContainerRequest, Image,
    core::{error::Result, mounts::AccessMode, ports::ContainerPort},
};

/// platform 文字列を `(architecture, rosetta, resolve_platform)` に正規化する。
///
/// - `"linux/amd64"` / `"amd64"` → amd64 + Rosetta + resolve 用 `"linux/amd64"`
/// - `"linux/arm64"` / `"arm64"` → arm64 + Rosetta 無効 + resolve 用 `"linux/arm64"`
/// - 未指定・許可外 → arm64 + Rosetta 無効 + resolve 未指定 (`None`)
///
/// 許可外を無視するのは、manifest 選択だけが寄って pull / create と食い違うのを防ぐため。
pub(crate) fn normalize_platform(
    platform: Option<&str>,
) -> (&'static str, bool, Option<&'static str>) {
    match platform {
        Some("linux/amd64" | "amd64") => ("amd64", true, Some("linux/amd64")),
        Some("linux/arm64" | "arm64") => ("arm64", false, Some("linux/arm64")),
        _ => ("arm64", false, None),
    }
}

/// 空いているホストポートを OS に割り当ててもらう。
///
/// bind(port 0) で得たポートを即座に手放すため、実際の公開までに他プロセスに
/// 奪われる可能性は理論上あるが、テスト用途では一般的な手法。
/// SCTP は `reject_sctp_ports` で事前拒否するため、ここには到達しない。
fn allocate_free_host_port(port: ContainerPort) -> Result<u16> {
    let allocated = match port {
        ContainerPort::Tcp(_) | ContainerPort::Sctp(_) => {
            std::net::TcpListener::bind(("127.0.0.1", 0))
                .and_then(|l| l.local_addr())
                .map(|a| a.port())
        }
        ContainerPort::Udp(_) => std::net::UdpSocket::bind(("127.0.0.1", 0))
            .and_then(|s| s.local_addr())
            .map(|a| a.port()),
    };
    allocated.map_err(|e| crate::Error::other(format!("failed to allocate free host port: {e}")))
}

/// Apple container は SCTP ポート公開に未対応のため、検出時は明示エラーにする。
///
/// `with_mapped_port(..., *.sctp())` と `with_exposed_port(*.sctp())` の両方を対象にする。
/// XPC の分かりにくいエラーを待たず、`build_config` 時点で落とす。
fn reject_sctp_ports<I: Image>(req: &ContainerRequest<I>) -> Result<()> {
    let mapped_has_sctp = req.ports().is_some_and(|ps| {
        ps.iter()
            .any(|p| matches!(p.container_port(), ContainerPort::Sctp(_)))
    });
    let exposed_has_sctp = req
        .expose_ports()
        .iter()
        .any(|p| matches!(p, ContainerPort::Sctp(_)));
    if mapped_has_sctp || exposed_has_sctp {
        return Err(crate::Error::other(
            "SCTP port publishing is not supported on Apple container",
        ));
    }
    Ok(())
}

/// Swift の Date 参照日 (2001-01-01T00:00:00Z) の Unix epoch 秒。
const SWIFT_REFERENCE_DATE_UNIX_SECS: f64 = 978_307_200.0;

/// Unix epoch 秒を Swift の `timeIntervalSinceReferenceDate` (2001-01-01 基準) に変換する。
///
/// Apple container は `creationDate` を Swift の `Date` として JSON デコードするため、
/// Unix epoch 秒のまま渡すと作成日時が約 31 年未来として表示される。
fn unix_secs_to_swift_reference_date(unix_secs: f64) -> f64 {
    unix_secs - SWIFT_REFERENCE_DATE_UNIX_SECS
}

/// `ContainerRequest<I>` から XPC の `ContainerCfg` を構築する。
pub(crate) fn build_config<I: Image>(
    req: &ContainerRequest<I>,
    id: &str,
    desc_raw: &str,
    image_config: &crate::core::client::image_config::ImageConfig,
) -> Result<ContainerCfg> {
    // ENTRYPOINT / CMD から init プロセスの executable と arguments を決める。
    // Docker の意味論に合わせ、ユーザー指定が無ければ image config の既定値を使う。
    let (init_exe, arguments) = image_config.effective_command(req.entrypoint(), req.cmd())?;

    // env を KEY=VALUE のリストに。
    let env: Vec<String> = req.env_vars().map(|(k, v)| format!("{k}={v}")).collect();

    // image_ref の正規化。pull / resolve と同じ規則で完全修飾形式にする。
    let image_ref = crate::core::client::xpc_client::normalize_image_reference(&req.descriptor());

    // cpus。Apple container のデフォルト (ContainerSystemConfig の defaultCPUs = 4) と同値。
    // 本家 testcontainers 0.27 に CPU / メモリ指定の API は無いため固定値でよい。
    let cpus = 4i32;

    // working directory。
    let wd = req
        .working_dir()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "/".into());

    // mounts。
    let mounts: Vec<MountCfg> = req.mounts().map(mount_cfg).collect();

    // Apple container は SCTP 未対応。XPC に渡す前に明示エラーで落とす。
    reject_sctp_ports(req)?;

    // ports。明示的なマッピング (with_mapped_port) に加え、`Image::expose_ports` /
    // `with_exposed_port` で宣言されたポートには空きホストポートを自動で割り当てる
    // (本家のランダムポート公開に相当。以前は expose_ports が黙って無視されていた)。
    let mut ports: Vec<PortCfg> = req
        .ports()
        .map(|ps| {
            ps.iter()
                .map(|p| PortCfg {
                    host_address: "0.0.0.0".into(),
                    host_port: p.host_port(),
                    container_port: p.container_port().as_u16(),
                    proto: p.container_port().as_str().into(),
                })
                .collect()
        })
        .unwrap_or_default();
    for exposed in req.expose_ports() {
        let proto = exposed.as_str();
        if ports
            .iter()
            .any(|p| p.container_port == exposed.as_u16() && p.proto == proto)
        {
            continue;
        }
        ports.push(PortCfg {
            host_address: "0.0.0.0".into(),
            host_port: allocate_free_host_port(*exposed)?,
            container_port: exposed.as_u16(),
            proto: proto.into(),
        });
    }

    // platform。許可値だけ architecture / rosetta に反映する。
    let (architecture, rosetta, _) = normalize_platform(req.platform().as_deref());

    // creation date。Apple container は Swift の Date として JSON デコードするため、
    // Unix epoch 秒ではなく Swift 参照日基準の秒数を渡す。
    let creation_date = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| unix_secs_to_swift_reference_date(d.as_secs_f64()))
        .unwrap_or(0.0);

    // privileged が指定されていれば全 capability を付与。
    // Apple container には Docker の privileged 相当のフラグは無いが、
    // capAdd: ["ALL"] が能力面での最も近い挙動。
    let cap_add = if req.privileged() {
        vec!["ALL".to_string()]
    } else {
        req.cap_add().cloned().unwrap_or_default()
    };

    // network。未指定なら Apple container 標準の "default" ネットワークに接続する。
    // 指定されたネットワークが存在しない場合は containerCreate がエラーを返す。
    let network = req
        .network()
        .clone()
        .unwrap_or_else(|| "default".to_string());

    Ok(ContainerCfg {
        id: id.to_string(),
        // 明示 hostname → container_name → コンテナ ID の優先順位。
        hostname: req
            .hostname()
            .map(str::to_owned)
            .or_else(|| req.container_name().clone())
            .unwrap_or_else(|| id.to_string()),
        image_ref,
        image_descriptor: desc_raw.to_string(),
        mounts,
        ports,
        init_exe,
        init_args: arguments,
        init_env: env,
        working_directory: wd,
        use_init: req.init(),
        cap_add,
        cap_drop: req.cap_drop().cloned().unwrap_or_default(),
        shm_size: req.shm_size(),
        user: parse_user(req.user())?,
        network,
        architecture: architecture.to_string(),
        rosetta,
        ssh: req.ssh(),
        read_only: req.readonly_rootfs(),
        labels: req.labels().clone(),
        cpus,
        creation_date,
        terminal: req.open_stdin().unwrap_or(false),
    })
}

/// `Mount` から `MountCfg` を作る。
fn mount_cfg(m: &crate::core::mounts::Mount) -> MountCfg {
    let (source, fs_type, volume_name) = match m.mount_type() {
        crate::core::mounts::MountType::Bind => (
            m.source().map(|s| s.to_string()).unwrap_or_default(),
            "virtiofs",
            None,
        ),
        crate::core::mounts::MountType::Volume => (
            m.source().map(|s| s.to_string()).unwrap_or_default(),
            "volume",
            m.source().map(|s| s.to_string()),
        ),
        crate::core::mounts::MountType::Tmpfs => ("tmpfs".into(), "tmpfs", None),
    };
    let destination = m.target().map(|s| s.to_string()).unwrap_or_default();
    let readonly = matches!(m.access_mode(), AccessMode::ReadOnly);

    // Apple container の Filesystem.options は ro/rw に加え、tmpfs なら size= / mode= を載せる。
    let mut options = vec![if readonly {
        "ro".to_string()
    } else {
        "rw".to_string()
    }];
    if matches!(m.mount_type(), crate::core::mounts::MountType::Tmpfs)
        && let Some(tmpfs) = m.tmpfs_options()
    {
        if let Some(size) = tmpfs.size_bytes {
            options.push(format!("size={size}"));
        }
        if let Some(mode) = tmpfs.mode {
            // Linux mount の mode= は 8 進数字列 (例: 1777)。整数値を 8 進で書く。
            options.push(format!("mode={mode:o}"));
        }
    }

    MountCfg {
        source,
        destination,
        fs_type: fs_type.into(),
        volume_name,
        options,
    }
}

// ── DisplayJson 型 ──

/// XPC `containerCreate` に渡すコンテナ設定ペイロード。
pub(crate) struct ContainerCfg {
    id: String,
    hostname: String,
    image_ref: String,
    image_descriptor: String,
    mounts: Vec<MountCfg>,
    ports: Vec<PortCfg>,
    init_exe: String,
    init_args: Vec<String>,
    init_env: Vec<String>,
    working_directory: String,
    use_init: bool,
    cap_add: Vec<String>,
    cap_drop: Vec<String>,
    shm_size: Option<u64>,
    user: ProcessUser,
    network: String,
    architecture: String,
    rosetta: bool,
    ssh: bool,
    read_only: bool,
    labels: std::collections::BTreeMap<String, String>,
    cpus: i32,
    creation_date: f64,
    terminal: bool,
}

impl DisplayJson for ContainerCfg {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("id", &self.id)?;
            f.member(
                "image",
                &ImageCfg {
                    reference: self.image_ref.clone(),
                    descriptor_raw: self.image_descriptor.clone(),
                },
            )?;
            f.member("mounts", &self.mounts)?;
            f.member("publishedPorts", &self.ports)?;
            f.member("publishedSockets", Vec::<String>::new())?;
            f.member("labels", &self.labels)?;
            f.member("sysctls", HashMap::<String, String>::new())?;
            f.member(
                "networks",
                vec![NetworkCfg {
                    network: self.network.clone(),
                    hostname: self.hostname.clone(),
                }],
            )?;
            f.member(
                "dns",
                &DnsCfg {
                    nameservers: vec![],
                    options: vec![],
                    search_domains: vec![],
                },
            )?;
            f.member("rosetta", self.rosetta)?;
            f.member(
                "initProcess",
                &InitProcessCfg {
                    exe: self.init_exe.clone(),
                    args: self.init_args.clone(),
                    env: self.init_env.clone(),
                    wd: self.working_directory.clone(),
                    user: self.user.clone(),
                    terminal: self.terminal,
                },
            )?;
            f.member(
                "platform",
                &ContainerPlatform {
                    architecture: self.architecture.clone(),
                },
            )?;
            f.member("resources", &ResourcesCfg { cpus: self.cpus })?;
            f.member("runtimeHandler", "container-runtime-linux")?;
            f.member("virtualization", false)?;
            f.member("ssh", self.ssh)?;
            f.member("readOnly", self.read_only)?;
            f.member("useInit", self.use_init)?;
            f.member("capAdd", &self.cap_add)?;
            f.member("capDrop", &self.cap_drop)?;
            f.member("shmSize", self.shm_size)?;
            f.member("stopSignal", &Option::<String>::None)?;
            f.member("creationDate", self.creation_date)
        })
    }
}

struct ImageCfg {
    reference: String,
    descriptor_raw: String,
}
impl DisplayJson for ImageCfg {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("reference", &self.reference)?;
            f.member("descriptor", RawJsonStr(&self.descriptor_raw))
        })
    }
}

struct RawJsonStr<'a>(&'a str);
impl DisplayJson for RawJsonStr<'_> {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        write!(f.inner_mut(), "{}", self.0)
    }
}

struct MountCfg {
    source: String,
    destination: String,
    fs_type: String,
    volume_name: Option<String>,
    /// `ro` / `rw` に加え、tmpfs では `size=` / `mode=` を含む。
    options: Vec<String>,
}
impl DisplayJson for MountCfg {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            match self.fs_type.as_str() {
                "virtiofs" => f.member("type", &FsTypeObj::Virtiofs)?,
                "volume" => f.member(
                    "type",
                    &FsTypeObj::Volume {
                        name: self.volume_name.clone().unwrap_or_default(),
                    },
                )?,
                _ => f.member("type", &FsTypeObj::Tmpfs)?,
            };
            f.member("source", &self.source)?;
            f.member("destination", &self.destination)?;
            f.member("options", &self.options)
        })
    }
}

enum FsTypeObj {
    Virtiofs,
    Volume { name: String },
    Tmpfs,
}
impl DisplayJson for FsTypeObj {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        match self {
            FsTypeObj::Virtiofs => f.object(|f| f.member("virtiofs", &EmptyObj)),
            FsTypeObj::Volume { name } => {
                f.object(|f| f.member("volume", &VolType { name: name.clone() }))
            }
            FsTypeObj::Tmpfs => f.object(|f| f.member("tmpfs", &EmptyObj)),
        }
    }
}

struct EmptyObj;
impl DisplayJson for EmptyObj {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|_| Ok(()))
    }
}

struct VolType {
    name: String,
}
impl DisplayJson for VolType {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("name", &self.name)?;
            f.member("format", "raw")?;
            f.member("cache", "auto")?;
            f.member("sync", "fsync")
        })
    }
}

struct PortCfg {
    host_address: String,
    host_port: u16,
    container_port: u16,
    proto: String,
}
impl DisplayJson for PortCfg {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("hostAddress", &self.host_address)?;
            f.member("hostPort", self.host_port)?;
            f.member("containerPort", self.container_port)?;
            f.member("proto", &self.proto)?;
            f.member("count", 1u16)
        })
    }
}

/// コンテナ init プロセスの設定 (`containerCreate` の `initProcess`)。
struct InitProcessCfg {
    exe: String,
    args: Vec<String>,
    env: Vec<String>,
    wd: String,
    user: ProcessUser,
    terminal: bool,
}
impl DisplayJson for InitProcessCfg {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("executable", &self.exe)?;
            f.member("arguments", &self.args)?;
            f.member("environment", &self.env)?;
            f.member("workingDirectory", &self.wd)?;
            f.member("terminal", self.terminal)?;
            f.member("user", &self.user)?;
            f.member("supplementalGroups", Vec::<u32>::new())?;
            f.member("rlimits", Vec::<u32>::new())
        })
    }
}

/// XPC `ProcessConfiguration.user` の値。
#[derive(Clone, Debug)]
enum ProcessUser {
    /// 数値 UID / GID。
    Id { uid: u32, gid: u32 },
    /// コンテナ内で解決されるユーザー文字列 (`name`、`name:group`、`uid:gid` 等)。
    Raw { user_string: String },
}

impl DisplayJson for ProcessUser {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        match self {
            ProcessUser::Id { uid, gid } => f.object(|f| {
                f.member(
                    "id",
                    &ProcessUserId {
                        uid: *uid,
                        gid: *gid,
                    },
                )
            }),
            ProcessUser::Raw { user_string } => {
                f.object(|f| f.member("raw", &ProcessUserRaw { user_string }))
            }
        }
    }
}

struct ProcessUserId {
    uid: u32,
    gid: u32,
}
impl DisplayJson for ProcessUserId {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("uid", self.uid)?;
            f.member("gid", self.gid)
        })
    }
}

struct ProcessUserRaw<'a> {
    user_string: &'a str,
}
impl DisplayJson for ProcessUserRaw<'_> {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| f.member("userString", self.user_string))
    }
}

/// `ContainerRequest::user()` の文字列を `ProcessUser` にパースする。
/// 数値形式 (`uid` または `uid:gid`) は `ProcessUser::Id` に、
/// それ以外はコンテナ内解決用の `ProcessUser::Raw` にする。
fn parse_user(user: Option<&str>) -> Result<ProcessUser> {
    let Some(user) = user else {
        return Ok(ProcessUser::Id { uid: 0, gid: 0 });
    };
    if user.is_empty() {
        return Ok(ProcessUser::Id { uid: 0, gid: 0 });
    }

    // 数値のみまたは `uid:gid` 形式を判定。
    let parts: Vec<&str> = user.splitn(2, ':').collect();
    let uid_str = parts[0];
    let gid_str = parts.get(1).copied();

    if let Ok(uid) = uid_str.parse::<u32>() {
        let gid = match gid_str {
            Some(s) => s
                .parse::<u32>()
                .map_err(|_| crate::Error::other(format!("invalid gid in user string: {user}")))?,
            None => 0,
        };
        return Ok(ProcessUser::Id { uid, gid });
    }

    // 名前形式はコンテナ内で解決される raw 文字列として渡す。
    Ok(ProcessUser::Raw {
        user_string: user.to_string(),
    })
}

/// `containerCreate` 用の platform (`os` / `architecture` / `variant`)。
struct ContainerPlatform {
    architecture: String,
}
impl DisplayJson for ContainerPlatform {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("os", "linux")?;
            f.member("architecture", &self.architecture)?;
            f.member("variant", &Option::<String>::None)
        })
    }
}

/// `containerCreate` 用のリソース設定。
struct ResourcesCfg {
    cpus: i32,
}
impl DisplayJson for ResourcesCfg {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("cpus", self.cpus)?;
            // Apple container のデフォルト (ContainerSystemConfig の defaultMemory = "1g") と同値。
            f.member("memoryInBytes", 1024u64 * 1024 * 1024)?;
            f.member("storage", Option::<u64>::None)?;
            f.member("cpuOverhead", 1i32)
        })
    }
}

struct NetworkCfg {
    network: String,
    hostname: String,
}
impl DisplayJson for NetworkCfg {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("network", &self.network)?;
            f.member(
                "options",
                &NetOpts {
                    hostname: self.hostname.clone(),
                    mtu: 1280,
                },
            )
        })
    }
}

struct NetOpts {
    hostname: String,
    mtu: u32,
}
impl DisplayJson for NetOpts {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("hostname", &self.hostname)?;
            f.member("mtu", self.mtu)
        })
    }
}

struct DnsCfg {
    nameservers: Vec<String>,
    options: Vec<String>,
    search_domains: Vec<String>,
}
impl DisplayJson for DnsCfg {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("nameservers", &self.nameservers)?;
            f.member("options", &self.options)?;
            f.member("searchDomains", &self.search_domains)
        })
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use crate::{ContainerRequest, GenericImage, ImageExt, xpc::j};

    use super::*;

    #[test]
    fn shm_size_is_reflected_in_container_cfg_json() {
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "1"])
            .with_shm_size(2 * 1024 * 1024 * 1024);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.shm_size, Some(2 * 1024 * 1024 * 1024));

        let json = String::from_utf8(j(&cfg)).expect("設定が有効な UTF-8 JSON であること");
        assert!(json.contains("\"shmSize\":2147483648"));
    }

    #[test]
    fn shm_size_is_null_by_default_in_container_cfg_json() {
        let req: ContainerRequest<GenericImage> =
            GenericImage::new("alpine", "latest").with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.shm_size, None);

        let json = String::from_utf8(j(&cfg)).expect("設定が有効な UTF-8 JSON であること");
        assert!(json.contains("\"shmSize\":null"));
    }

    #[test]
    fn user_numeric_is_reflected_in_container_cfg_json() {
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "1"])
            .with_user("1000:1000");
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");

        let json = String::from_utf8(j(&cfg)).expect("設定が有効な UTF-8 JSON であること");
        assert!(json.contains("\"user\":{\"id\":{\"uid\":1000,\"gid\":1000}}"));
    }

    #[test]
    fn network_is_reflected_in_container_cfg_json() {
        // with_network で指定したネットワーク名が networks[0].network に反映されること。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "1"])
            .with_network("my-net");
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.network, "my-net");

        let json = String::from_utf8(j(&cfg)).expect("設定が有効な UTF-8 JSON であること");
        assert!(json.contains("\"networks\":[{\"network\":\"my-net\""));
    }

    #[test]
    fn network_is_default_when_unspecified_in_container_cfg_json() {
        // 未指定時は Apple container 標準の "default" ネットワークに接続すること。
        let req: ContainerRequest<GenericImage> =
            GenericImage::new("alpine", "latest").with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.network, "default");

        let json = String::from_utf8(j(&cfg)).expect("設定が有効な UTF-8 JSON であること");
        assert!(json.contains("\"networks\":[{\"network\":\"default\""));
    }

    #[test]
    fn hostname_is_reflected_in_container_cfg_json() {
        // with_hostname で指定した値が networks[0].options.hostname に反映されること。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "1"])
            .with_hostname("guest-host");
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.hostname, "guest-host");

        let json = String::from_utf8(j(&cfg)).expect("設定が有効な UTF-8 JSON であること");
        assert!(
            json.contains("\"hostname\":\"guest-host\""),
            "JSON に指定ホスト名が含まれること: {json}"
        );
        assert!(
            !json.contains("\"hostname\":\"test-id\""),
            "フォールバックの id が残っていないこと: {json}"
        );
    }

    #[test]
    fn hostname_falls_back_to_container_name_when_unspecified() {
        // hostname 未指定 + with_container_name のとき container_name にフォールバックすること。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "1"])
            .with_container_name("named-box");
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.hostname, "named-box");
    }

    #[test]
    fn hostname_falls_back_to_id_when_name_unspecified() {
        // hostname も container_name も未指定のときコンテナ ID にフォールバックすること。
        let req: ContainerRequest<GenericImage> =
            GenericImage::new("alpine", "latest").with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "generated-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.hostname, "generated-id");
    }

    #[test]
    fn hostname_wins_over_container_name_when_both_set() {
        // with_hostname と with_container_name を異なる値で指定したとき hostname が勝つこと。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "1"])
            .with_container_name("container-name-value")
            .with_hostname("hostname-value");
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.hostname, "hostname-value");
        assert_eq!(req.hostname(), Some("hostname-value"));
        assert_eq!(
            req.container_name().as_deref(),
            Some("container-name-value")
        );
    }

    #[test]
    fn readonly_rootfs_true_is_reflected_in_container_cfg_json() {
        // with_readonly_rootfs(true) が JSON の "readOnly":true に反映されること。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "1"])
            .with_readonly_rootfs(true);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert!(cfg.read_only);
        assert!(req.readonly_rootfs());

        let json = String::from_utf8(j(&cfg)).expect("設定が有効な UTF-8 JSON であること");
        assert!(
            json.contains("\"readOnly\":true"),
            "JSON に readOnly:true が含まれること: {json}"
        );
    }

    #[test]
    fn readonly_rootfs_defaults_to_false_in_container_cfg_json() {
        // 未指定時は従来どおり "readOnly":false であること。
        let req: ContainerRequest<GenericImage> =
            GenericImage::new("alpine", "latest").with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert!(!cfg.read_only);
        assert!(!req.readonly_rootfs());

        let json = String::from_utf8(j(&cfg)).expect("設定が有効な UTF-8 JSON であること");
        assert!(
            json.contains("\"readOnly\":false"),
            "JSON に readOnly:false が含まれること: {json}"
        );
    }

    #[test]
    fn open_stdin_true_sets_terminal_in_container_cfg_json() {
        // with_open_stdin(true) が initProcess.terminal に反映されること。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "1"])
            .with_open_stdin(true);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert!(cfg.terminal);
        assert_eq!(req.open_stdin(), Some(true));

        let json = String::from_utf8(j(&cfg)).expect("設定が有効な UTF-8 JSON であること");
        assert!(
            json.contains("\"terminal\":true"),
            "JSON に terminal:true が含まれること: {json}"
        );
    }

    #[test]
    fn open_stdin_defaults_to_terminal_false() {
        // 未指定時は terminal:false (従来どおり)。
        let req: ContainerRequest<GenericImage> =
            GenericImage::new("alpine", "latest").with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert!(!cfg.terminal);
        assert_eq!(req.open_stdin(), None);

        let json = String::from_utf8(j(&cfg)).expect("設定が有効な UTF-8 JSON であること");
        assert!(
            json.contains("\"terminal\":false"),
            "JSON に terminal:false が含まれること: {json}"
        );
    }

    #[test]
    fn entrypoint_receives_whole_cmd_as_arguments() {
        // Docker の意味論どおり、entrypoint がある場合は CMD 全体が引数になること。
        // (旧実装は CMD の第 1 要素を捨てていた)
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_entrypoint("/entry.sh")
            .with_cmd(["serve", "--port", "80"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.init_exe, "/entry.sh");
        assert_eq!(cfg.init_args, vec!["serve", "--port", "80"]);
    }

    #[test]
    fn missing_cmd_and_entrypoint_is_an_error() {
        // cmd も entrypoint も無い場合、空 executable の壊れたコンテナを作らず
        // 明確なエラーになること。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest").into();
        let result = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        );
        assert!(result.is_err(), "空のコマンドはエラーになること");
    }

    #[test]
    fn image_config_fallback_resolves_default_cmd() {
        // ユーザー指定が無ければ image config の CMD を使う。
        let req: ContainerRequest<GenericImage> = GenericImage::new("nginx", "latest").into();
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig {
                entrypoint: None,
                cmd: Some(vec!["nginx".into(), "-g".into(), "daemon off;".into()]),
            },
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.init_exe, "nginx");
        assert_eq!(cfg.init_args, vec!["-g", "daemon off;"]);
    }

    #[test]
    fn user_cmd_overrides_image_config_cmd() {
        // ユーザー指定 cmd があれば image config の CMD は上書きされる。
        let req: ContainerRequest<GenericImage> =
            GenericImage::new("nginx", "latest").with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig {
                entrypoint: None,
                cmd: Some(vec!["nginx".into()]),
            },
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.init_exe, "sleep");
        assert_eq!(cfg.init_args, vec!["1"]);
    }

    #[test]
    fn user_entrypoint_overrides_image_config_entrypoint() {
        // ユーザー指定 entrypoint があれば image config の entrypoint は上書きされる。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_entrypoint("/bin/bash")
            .into();
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig {
                entrypoint: Some(vec!["/bin/sh".into(), "-c".into()]),
                cmd: Some(vec!["echo".into(), "image".into()]),
            },
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.init_exe, "/bin/bash");
        assert_eq!(cfg.init_args, vec!["echo", "image"]);
    }

    #[test]
    fn exposed_ports_get_auto_allocated_host_ports() {
        // with_exposed_port のポートに空きホストポートが自動割当されること。
        // (旧実装は expose_ports を黙って無視していた)
        use crate::core::ports::IntoContainerPort;
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_exposed_port(80.tcp())
            .with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.ports.len(), 1);
        assert_eq!(cfg.ports[0].container_port, 80);
        assert_ne!(
            cfg.ports[0].host_port, 0,
            "ホストポートが割り当てられること"
        );

        // 明示的なマッピングがある場合は自動割当で重複しないこと。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_exposed_port(80.tcp())
            .with_mapped_port(18080, 80.tcp())
            .with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert_eq!(cfg.ports.len(), 1);
        assert_eq!(cfg.ports[0].host_port, 18080);
    }

    #[test]
    fn sctp_exposed_port_is_rejected_by_build_config() {
        // with_exposed_port(*.sctp()) は XPC に渡さず明示エラーになること。
        use crate::core::ports::IntoContainerPort;
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_exposed_port(5060.sctp())
            .with_cmd(["sleep", "1"]);
        let result = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        );
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("SCTP exposed port は拒否されること"),
        };
        assert!(
            err.to_string()
                .contains("SCTP port publishing is not supported on Apple container"),
            "エラーメッセージが SCTP 未対応を明示すること: {err}"
        );
    }

    #[test]
    fn sctp_mapped_port_is_rejected_by_build_config() {
        // with_mapped_port(..., *.sctp()) も同様に明示エラーになること。
        use crate::core::ports::IntoContainerPort;
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_mapped_port(15060, 5060.sctp())
            .with_cmd(["sleep", "1"]);
        let result = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        );
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("SCTP mapped port は拒否されること"),
        };
        assert!(
            err.to_string()
                .contains("SCTP port publishing is not supported on Apple container"),
            "エラーメッセージが SCTP 未対応を明示すること: {err}"
        );
    }

    #[test]
    fn normalize_platform_maps_allowed_values() {
        // amd64 系は Rosetta 有効 + resolve 用に linux/amd64 を返す。
        assert_eq!(
            normalize_platform(Some("linux/amd64")),
            ("amd64", true, Some("linux/amd64"))
        );
        assert_eq!(
            normalize_platform(Some("amd64")),
            ("amd64", true, Some("linux/amd64"))
        );
        // arm64 系は Rosetta 無効 + resolve 用に linux/arm64 を返す。
        assert_eq!(
            normalize_platform(Some("linux/arm64")),
            ("arm64", false, Some("linux/arm64"))
        );
        assert_eq!(
            normalize_platform(Some("arm64")),
            ("arm64", false, Some("linux/arm64"))
        );
        // 未指定・許可外は arm64 / Rosetta 無効 / resolve 未指定に落とす。
        assert_eq!(normalize_platform(None), ("arm64", false, None));
        assert_eq!(
            normalize_platform(Some("linux/ppc64le")),
            ("arm64", false, None)
        );
        assert_eq!(normalize_platform(Some("x86_64")), ("arm64", false, None));
    }

    #[test]
    fn with_platform_amd64_enables_rosetta() {
        // 本家互換の "linux/amd64" と省略形 "amd64" の両方で Rosetta と architecture が効く。
        for platform in ["linux/amd64", "amd64"] {
            let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
                .with_platform(platform)
                .with_cmd(["sleep", "1"]);
            let cfg = build_config(
                &req,
                "test-id",
                "{}",
                &crate::core::client::image_config::ImageConfig::default(),
            )
            .expect("build_config が成功すること");
            assert!(
                cfg.rosetta,
                "platform {platform} が Rosetta を有効にすること"
            );
            assert_eq!(
                cfg.architecture, "amd64",
                "platform {platform} が architecture を amd64 に設定すること"
            );

            let json = String::from_utf8(j(&cfg)).expect("設定が有効な UTF-8 JSON であること");
            assert!(
                json.contains(r#""architecture":"amd64""#),
                "platform JSON が {platform} の amd64 を含むこと: {json}"
            );
        }

        // arm64 明示では Rosetta にならず architecture は arm64。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_platform("linux/arm64")
            .with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert!(!cfg.rosetta);
        assert_eq!(cfg.architecture, "arm64");

        // 未指定も arm64 / Rosetta 無効。
        let req: ContainerRequest<GenericImage> =
            GenericImage::new("alpine", "latest").with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert!(!cfg.rosetta);
        assert_eq!(cfg.architecture, "arm64");

        // 許可外は無視して arm64 / Rosetta 無効。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_platform("linux/ppc64le")
            .with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");
        assert!(!cfg.rosetta);
        assert_eq!(cfg.architecture, "arm64");
    }

    #[test]
    fn mounts_are_mapped_to_xpc_payload_configuration() {
        // Mount の種別・読み取り専用指定が XPC ペイロードの各フィールドに変換されること。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_mount(crate::core::mounts::Mount::bind_mount(
                "/host/bind",
                "/container/bind",
            ))
            .with_mount(
                crate::core::mounts::Mount::volume_mount("data-volume", "/container/volume")
                    .with_access_mode(crate::core::mounts::AccessMode::ReadOnly),
            )
            .with_mount(crate::core::mounts::Mount::tmpfs_mount("/container/tmpfs"))
            .with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("コンテナ設定の構築に成功すること");

        assert_eq!(cfg.mounts.len(), 3);
        assert_eq!(cfg.mounts[0].source, "/host/bind");
        assert_eq!(cfg.mounts[0].destination, "/container/bind");
        assert_eq!(cfg.mounts[0].fs_type, "virtiofs");
        assert_eq!(cfg.mounts[0].volume_name, None);
        assert_eq!(cfg.mounts[0].options, vec!["rw".to_string()]);
        assert_eq!(cfg.mounts[1].source, "data-volume");
        assert_eq!(cfg.mounts[1].fs_type, "volume");
        assert_eq!(cfg.mounts[1].volume_name.as_deref(), Some("data-volume"));
        assert_eq!(cfg.mounts[1].options, vec!["ro".to_string()]);
        assert_eq!(cfg.mounts[2].source, "tmpfs");
        assert_eq!(cfg.mounts[2].fs_type, "tmpfs");
        assert_eq!(cfg.mounts[2].volume_name, None);
        assert_eq!(cfg.mounts[2].options, vec!["rw".to_string()]);
    }

    #[test]
    fn tmpfs_size_and_mode_are_reflected_in_mount_options() {
        // tmpfs の size / mode が Filesystem.options の size= / mode= に載ること。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_mount(
                crate::core::mounts::Mount::tmpfs_mount("/container/tmpfs")
                    .with_size_bytes(64 * 1024 * 1024)
                    .with_mode(0o1777),
            )
            .with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("コンテナ設定の構築に成功すること");

        assert_eq!(cfg.mounts.len(), 1);
        assert_eq!(
            cfg.mounts[0].options,
            vec![
                "rw".to_string(),
                "size=67108864".to_string(),
                "mode=1777".to_string(),
            ]
        );

        let json = String::from_utf8(j(&cfg)).expect("設定が有効な UTF-8 JSON であること");
        assert!(
            json.contains("\"size=67108864\""),
            "JSON に size オプションが含まれること: {json}"
        );
        assert!(
            json.contains("\"mode=1777\""),
            "JSON に mode オプションが含まれること: {json}"
        );
    }

    #[test]
    fn capabilities_and_working_directory_are_mapped_to_container_cfg() {
        // capability の追加・削除と working directory の指定値が設定に反映されること。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_cap_add("NET_ADMIN")
            .with_cap_drop("MKNOD")
            .with_working_dir("/work")
            .with_cmd(["sleep", "1"]);
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("コンテナ設定の構築に成功すること");
        assert_eq!(cfg.cap_add, vec!["NET_ADMIN"]);
        assert_eq!(cfg.cap_drop, vec!["MKNOD"]);
        assert_eq!(cfg.working_directory, "/work");

        // privileged は個別の capability 追加より優先して全 capability を設定すること。
        let privileged_req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_cap_add("NET_ADMIN")
            .with_privileged(true)
            .with_cmd(["sleep", "1"]);
        let privileged_cfg = build_config(
            &privileged_req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("特権コンテナ設定の構築に成功すること");
        assert_eq!(privileged_cfg.cap_add, vec!["ALL"]);

        let default_req: ContainerRequest<GenericImage> =
            GenericImage::new("alpine", "latest").with_cmd(["sleep", "1"]);
        let default_cfg = build_config(
            &default_req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("既定のコンテナ設定の構築に成功すること");
        assert_eq!(default_cfg.working_directory, "/");
    }

    #[test]
    fn unix_secs_to_swift_reference_date_subtracts_offset() {
        // 2001-01-01T00:00:00Z (Unix epoch 978307200 秒) が Swift 参照日の 0 になる。
        assert_eq!(unix_secs_to_swift_reference_date(978_307_200.0), 0.0);
        // 2026-07-06T13:09:48Z 相当。Unix epoch 秒 − 978307200 になる。
        assert_eq!(
            unix_secs_to_swift_reference_date(1_783_170_588.0),
            1_783_170_588.0 - 978_307_200.0
        );
    }

    #[test]
    fn user_name_is_raw_in_container_cfg_json() {
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "1"])
            .with_user("nobody");
        let cfg = build_config(
            &req,
            "test-id",
            "{}",
            &crate::core::client::image_config::ImageConfig::default(),
        )
        .expect("build_config が成功すること");

        let json = String::from_utf8(j(&cfg)).expect("設定が有効な UTF-8 JSON であること");
        assert!(json.contains("\"user\":{\"raw\":{\"userString\":\"nobody\"}}"));
    }
}
