//! 標準ロガー付きのログコンシューマ。元の 0.27 と概ね同一シグネチャ。
//!
//! 本家は `log::Level` を受け取るが、本クレートは `tracing` 規約に合わせ
//! `tracing::Level` を使う。既定出力は従来どおり `eprintln!` で、
//! レベルを指定したストリームだけ `tracing` に流す。

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use tracing::Level;

use crate::core::logs::LogFrame;

/// ログを標準エラーまたは tracing に出すログコンシューマ。
#[derive(Debug)]
pub struct LoggingConsumer {
    stdout_level: Option<Level>,
    stderr_level: Option<Level>,
    prefix: Option<String>,
}

impl LoggingConsumer {
    /// 既定のコンシューマを作る (両ストリームとも `eprintln!`)。
    pub fn new() -> Self {
        Self {
            stdout_level: None,
            stderr_level: None,
            prefix: None,
        }
    }

    /// stdout を tracing の指定レベルで出す。
    pub fn with_stdout_level(mut self, level: Level) -> Self {
        self.stdout_level = Some(level);
        self
    }

    /// stderr を tracing の指定レベルで出す。
    pub fn with_stderr_level(mut self, level: Level) -> Self {
        self.stderr_level = Some(level);
        self
    }

    /// 各ログメッセージの先頭に接頭辞を付ける (接頭辞と本文の間に半角スペース)。
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    fn format_message<'a>(&self, message: &'a str) -> Cow<'a, str> {
        let message = message.trim_end_matches(['\n', '\r']);
        if let Some(prefix) = &self.prefix {
            Cow::Owned(format!("{prefix} {message}"))
        } else {
            Cow::Borrowed(message)
        }
    }
}

impl Default for LoggingConsumer {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::core::logs::consumer::LogConsumer for LoggingConsumer {
    fn accept<'a>(&'a self, record: &'a LogFrame) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        let bytes = record.bytes().to_vec();
        Box::pin(async move {
            let text = String::from_utf8_lossy(bytes.as_ref());
            let message = self.format_message(&text);
            let source = match record.source() {
                crate::core::logs::LogSource::StdOut => "stdout",
                crate::core::logs::LogSource::StdErr => "stderr",
                crate::core::logs::LogSource::BothStd => "both",
            };
            let level = match record.source() {
                crate::core::logs::LogSource::StdOut => self.stdout_level,
                crate::core::logs::LogSource::StdErr => self.stderr_level,
                crate::core::logs::LogSource::BothStd => self.stdout_level.or(self.stderr_level),
            };
            match level {
                Some(level) => match level {
                    Level::ERROR => tracing::error!("{message}"),
                    Level::WARN => tracing::warn!("{message}"),
                    Level::INFO => tracing::info!("{message}"),
                    Level::DEBUG => tracing::debug!("{message}"),
                    Level::TRACE => tracing::trace!("{message}"),
                },
                None => {
                    eprintln!("{source}: {message}");
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_message_trims_trailing_newlines() {
        let consumer = LoggingConsumer::new();
        assert_eq!(consumer.format_message("hello\n"), "hello");
        assert_eq!(consumer.format_message("hello\r\n"), "hello");
    }

    #[test]
    fn format_message_adds_prefix_with_space() {
        let consumer = LoggingConsumer::new().with_prefix("ctr");
        assert_eq!(consumer.format_message("line\n"), "ctr line");
    }

    #[test]
    fn with_levels_store_tracing_levels() {
        let consumer = LoggingConsumer::new()
            .with_stdout_level(Level::DEBUG)
            .with_stderr_level(Level::WARN);
        assert_eq!(consumer.stdout_level, Some(Level::DEBUG));
        assert_eq!(consumer.stderr_level, Some(Level::WARN));
    }
}
