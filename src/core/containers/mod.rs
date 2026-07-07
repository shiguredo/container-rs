//! `ContainerAsync` と関連型。元の 0.27 の `core::containers` に相当。

pub(crate) mod async_container;
pub(crate) mod request;

pub use async_container::{ContainerAsync, exec::ExecResult};
pub use request::{CgroupnsMode, ContainerRequest, ExtraHost, PortMapping};

#[cfg(feature = "blocking")]
pub(crate) mod sync_container;

#[cfg(feature = "blocking")]
pub use sync_container::{Container, SyncExecResult};
