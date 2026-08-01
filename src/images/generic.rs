//! `GenericImage`。元の 0.27 の `GenericImage` と同一シグネチャ。

use crate::{
    Image,
    core::{WaitFor, ports::ContainerPort},
};

/// 設定可能な汎用イメージ。`ContainerAsync` の起点。
#[must_use]
#[derive(Debug, Clone)]
pub struct GenericImage {
    name: String,
    tag: String,
    wait_for: Vec<WaitFor>,
    entrypoint: Option<String>,
    exposed_ports: Vec<ContainerPort>,
}

impl GenericImage {
    /// イメージ名とタグを指定して汎用イメージを作る。
    pub fn new<S: Into<String>>(name: S, tag: S) -> GenericImage {
        Self {
            name: name.into(),
            tag: tag.into(),
            wait_for: Vec::new(),
            entrypoint: None,
            exposed_ports: Vec::new(),
        }
    }

    /// 準備完了条件を追加する。
    pub fn with_wait_for(mut self, wait_for: WaitFor) -> Self {
        self.wait_for.push(wait_for);
        self
    }

    /// entrypoint を設定する。
    pub fn with_entrypoint(mut self, entrypoint: &str) -> Self {
        self.entrypoint = Some(entrypoint.to_string());
        self
    }

    /// 公開ポートを追加する。
    pub fn with_exposed_port(mut self, port: ContainerPort) -> Self {
        self.exposed_ports.push(port);
        self
    }
}

impl Image for GenericImage {
    fn name(&self) -> &str {
        &self.name
    }

    fn tag(&self) -> &str {
        &self.tag
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        self.wait_for.clone()
    }

    fn entrypoint(&self) -> Option<&str> {
        self.entrypoint.as_deref()
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &self.exposed_ports
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ports::IntoContainerPort;

    #[test]
    fn generic_image_name_and_tag_are_stored() {
        let image = GenericImage::new("alpine", "latest");
        assert_eq!(image.name(), "alpine", "name が保存されること");
        assert_eq!(image.tag(), "latest", "tag が保存されること");
    }

    #[test]
    fn generic_image_with_wait_for_appends_condition() {
        let image = GenericImage::new("alpine", "latest")
            .with_wait_for(WaitFor::message_on_stdout("READY"));
        let conditions = image.ready_conditions();
        assert_eq!(conditions.len(), 1, "wait_for が 1 件追加されること");
    }

    #[test]
    fn generic_image_with_entrypoint_is_reflected() {
        let image = GenericImage::new("alpine", "latest").with_entrypoint("/bin/sh");
        assert_eq!(
            image.entrypoint(),
            Some("/bin/sh"),
            "entrypoint が反映されること"
        );
    }

    #[test]
    fn generic_image_with_exposed_port_is_reflected() {
        let image = GenericImage::new("nginx", "latest").with_exposed_port(80u16.tcp());
        let ports = image.expose_ports();
        assert_eq!(ports.len(), 1, "exposed_port が 1 件追加されること");
        assert_eq!(ports[0], 80u16.tcp(), "追加したポートが一致すること");
    }
}
