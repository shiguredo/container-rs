//! コンテナ操作の中核モジュール。

pub mod client;
pub mod containers;
pub mod copy;
pub mod env;
pub mod error;
pub mod healthcheck;
pub mod host;
pub mod image;
pub mod logs;
pub mod mounts;
pub mod ports;
// macOS の XPC 経路 (コンテナ ID / exec ID / 一時ファイル名) 専用。
// Linux では呼び出し元がコンパイルされないためモジュールごとゲートする。
#[cfg(target_os = "macos")]
pub(crate) mod util;
pub mod wait;

#[cfg(feature = "blocking")]
pub use self::containers::{Container, SyncExecResult};
pub use self::{
    containers::{
        CgroupnsMode, ContainerAsync, ContainerRequest, ExecResult, ExtraHost, PortMapping,
    },
    copy::{
        CopyDataSource, CopyFileFromContainer, CopyFromContainerError, CopyTargetOptions,
        CopyToContainer, CopyToContainerError,
    },
    healthcheck::Healthcheck,
    host::Host,
    image::{ContainerState, ExecCommand, Image, ImageExt},
    mounts::{AccessMode, Mount, MountTmpfsOptions, MountType},
    ports::{ContainerPort, IntoContainerPort, Ports},
    wait::{CmdWaitFor, WaitFor},
};
