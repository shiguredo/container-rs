//! `ExecCommand`。元の testcontainers 0.27 のサブセット（差分の正は `docs/TESTCONTAINERS.md` の該当節 12.1）。

use std::collections::HashMap;

use crate::core::{WaitFor, wait::CmdWaitFor};

#[derive(Debug)]
pub struct ExecCommand {
    pub(crate) cmd: Vec<String>,
    pub(crate) cmd_ready_condition: CmdWaitFor,
    pub(crate) container_ready_conditions: Vec<WaitFor>,
    pub(crate) env_vars: HashMap<String, String>,
}

impl ExecCommand {
    pub fn new(cmd: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            cmd: cmd.into_iter().map(Into::into).collect(),
            cmd_ready_condition: CmdWaitFor::Nothing,
            container_ready_conditions: vec![],
            env_vars: HashMap::new(),
        }
    }

    pub fn with_container_ready_conditions(mut self, ready_conditions: Vec<WaitFor>) -> Self {
        self.container_ready_conditions = ready_conditions;
        self
    }

    pub fn with_cmd_ready_condition(mut self, ready_conditions: impl Into<CmdWaitFor>) -> Self {
        self.cmd_ready_condition = ready_conditions.into();
        self
    }

    /// exec プロセスに追加する環境変数を設定する。
    ///
    /// macOS (Apple container) ではコンテナ作成時の env に重ねて送り、
    /// 同名キーはこのメソッドの値が優先される。
    /// Linux ではコンテナの実際の env (inspect の `Config.Env`) を基底にし、
    /// このメソッドの値で上書きマージして `ExecConfig.Env` に設定する。
    /// 空の場合はコンテナ env を継承する (Docker の既定動作)。
    pub fn with_env_vars(
        mut self,
        env_vars: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.env_vars
            .extend(env_vars.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }
}

impl Default for ExecCommand {
    fn default() -> Self {
        Self::new(Vec::<String>::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_env_vars_stores_entries() {
        // with_env_vars で渡したキー・値がフィールドに残ること。
        let exec =
            ExecCommand::new(["true"]).with_env_vars([("KEY1", "value1"), ("KEY2", "value2")]);
        assert_eq!(
            exec.env_vars.get("KEY1").map(String::as_str),
            Some("value1")
        );
        assert_eq!(
            exec.env_vars.get("KEY2").map(String::as_str),
            Some("value2")
        );
    }

    #[test]
    fn with_env_vars_can_be_called_multiple_times() {
        // 複数回呼ぶと累積し、同名キーは後勝ちになること。
        let exec = ExecCommand::new(["true"])
            .with_env_vars([("A", "1"), ("B", "2")])
            .with_env_vars([("B", "overridden"), ("C", "3")]);
        assert_eq!(exec.env_vars.get("A").map(String::as_str), Some("1"));
        assert_eq!(
            exec.env_vars.get("B").map(String::as_str),
            Some("overridden")
        );
        assert_eq!(exec.env_vars.get("C").map(String::as_str), Some("3"));
    }
}
