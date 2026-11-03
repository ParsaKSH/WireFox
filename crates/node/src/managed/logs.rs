//! Recent log lines kept in memory, so the panel can fetch them from nodes it
//! can't log in to.

use std::collections::VecDeque;
use std::fmt::{self, Write as _};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::pb::LogLine;

/// Lines kept by the managed node.
pub const DEFAULT_CAPACITY: usize = 2000;

/// A bounded ring of log lines; cloning shares the buffer.
#[derive(Clone)]
pub struct LogBuffer {
    inner: Arc<Mutex<VecDeque<LogLine>>>,
    capacity: usize,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        LogBuffer { inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))), capacity }
    }

    pub fn push(&self, line: LogLine) {
        let mut lines = self.inner.lock();
        if lines.len() == self.capacity {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// The most recent `n` lines, oldest first.
    pub fn tail(&self, n: usize) -> Vec<LogLine> {
        let lines = self.inner.lock();
        lines.iter().skip(lines.len().saturating_sub(n)).cloned().collect()
    }

    /// A tracing layer that records every event into this buffer.
    pub fn layer(&self) -> LogLayer {
        LogLayer(self.clone())
    }
}

pub struct LogLayer(LogBuffer);

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut message = MessageVisitor::default();
        event.record(&mut message);
        let unix_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let level = event.metadata().level().as_str().to_string();
        self.0.push(LogLine { unix_ms, level, message: message.finish() });
    }
}

/// Formats an event like the console output does: the message, then `key=value` fields.
#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl MessageVisitor {
    fn finish(self) -> String {
        match (self.message.is_empty(), self.fields.is_empty()) {
            (_, true) => self.message,
            (true, false) => self.fields,
            (false, false) => format!("{} {}", self.message, self.fields),
        }
    }

    fn field_separator(&mut self) {
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
    }
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            self.field_separator();
            let _ = write!(self.fields, "{}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            self.field_separator();
            let _ = write!(self.fields, "{}={value:?}", field.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[test]
    fn keeps_the_most_recent_lines() {
        let logs = LogBuffer::new(3);
        for i in 0..5 {
            logs.push(LogLine { unix_ms: i, level: "INFO".into(), message: i.to_string() });
        }
        let messages: Vec<_> = logs.tail(10).into_iter().map(|l| l.message).collect();
        assert_eq!(messages, ["2", "3", "4"]);
        assert_eq!(logs.tail(1)[0].message, "4");
        assert!(logs.tail(0).is_empty());
    }

    #[test]
    fn records_events_with_fields() {
        let logs = LogBuffer::new(10);
        let subscriber = tracing_subscriber::registry().with(logs.layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(peer = %"1.2.3.4:5", attempt = 2, "connection failed");
            tracing::info!(name = "eth0");
        });
        let lines = logs.tail(10);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].level, "WARN");
        assert_eq!(lines[0].message, "connection failed peer=1.2.3.4:5 attempt=2");
        assert!(lines[0].unix_ms > 0);
        assert_eq!(lines[1].message, "name=eth0");
    }
}
