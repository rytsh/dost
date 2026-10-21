//! Channel for sending notifications to the dog.
//!
//! Notification sources (MCP, status watches, ...) take a clone of [`Notifier`] and
//! call [`Notifier::send`] from any thread. The dog shows the notification in a bubble.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// How long the bubble stays on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timeout {
    /// The default duration from settings.
    Default,
    /// Until the user clicks it.
    Sticky,
    After(Duration),
}

#[derive(Clone, Debug)]
pub struct Notification {
    pub title: String,
    pub body: String,
    pub timeout: Timeout,
}

impl Notification {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Notification {
            title: title.into(),
            body: body.into(),
            timeout: Timeout::Default,
        }
    }

    pub fn sticky(mut self) -> Self {
        self.timeout = Timeout::Sticky;
        self
    }

    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Timeout::After(d);
        self
    }

    /// `default`: the duration from settings (`None` = until clicked).
    pub fn resolve_timeout(&self, default: Option<Duration>) -> Option<Duration> {
        match self.timeout {
            Timeout::Default => default,
            Timeout::Sticky => None,
            Timeout::After(d) => Some(d),
        }
    }
}

#[derive(Clone)]
pub struct Notifier(Sender<Notification>);

impl Notifier {
    /// Returns `false` if the app has shut down.
    pub fn send(&self, n: Notification) -> bool {
        self.0.send(n).is_ok()
    }
}

pub fn channel() -> (Notifier, Receiver<Notification>) {
    let (tx, rx) = mpsc::channel();
    (Notifier(tx), rx)
}
