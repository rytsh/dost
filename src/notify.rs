//! Channel for sending events to the dog.
//!
//! Event sources (MCP, status watches, ...) take a clone of [`Notifier`] and call
//! [`Notifier::send`] or [`Notifier::react`] from any thread. The dog shows notifications in a
//! bubble; questions ([`Notification::ask`]) carry a channel the answer is sent back on.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// How long the bubble stays on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timeout {
    /// The default duration from settings.
    Default,
    /// Until the user clicks it.
    Sticky,
    After(Duration),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressState {
    Running,
    Done,
    Failed,
}

/// A long task's progress, shown as a bar in the bubble and updated in place by `id`.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub id: String,
    /// 0.0-1.0; `None` draws an indeterminate bar.
    pub value: Option<f32>,
    pub state: ProgressState,
}

#[derive(Clone, Debug)]
pub struct Notification {
    /// Assigned by the app when received; links history entries and progress updates.
    pub id: u64,
    pub title: String,
    pub body: String,
    pub timeout: Timeout,
    /// Who sent it (e.g. "OpenCode"); shown above the title.
    pub source: Option<String>,
    /// Opened when the bubble is clicked (http/https only).
    pub url: Option<String>,
    /// Put on the clipboard when the bubble appears.
    pub copy: Option<String>,
    /// Answer buttons; the chosen one is sent on `reply`.
    pub choices: Vec<String>,
    pub reply: Option<Sender<String>>,
    /// Dropped (unanswered) after this, whether shown or still queued.
    pub expires: Option<Instant>,
    pub progress: Option<Progress>,
    /// Already shown once: shown again (a progress bubble taking turns) without barking.
    pub announced: bool,
    /// Caller's name for a question, so it can be withdrawn with [`Event::Dismiss`].
    pub key: Option<String>,
    /// Snoozed ("Later"): waits until then before it is shown again.
    pub not_before: Option<Instant>,
    /// Good or bad news: how the dog reacts (and sounds).
    pub mood: Mood,
}

/// How a notification feels to the dog.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mood {
    #[default]
    Neutral,
    /// Something finished well: the dog is happy.
    Good,
    /// Something failed: the dog is sad.
    Bad,
}

impl Mood {
    pub const NAMES: [&str; 3] = ["neutral", "good", "bad"];

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "neutral" => Mood::Neutral,
            "good" | "success" => Mood::Good,
            "bad" | "failure" | "error" => Mood::Bad,
            _ => return None,
        })
    }

    /// A guess from the words: "Build failed", "Tests passed", ...
    pub fn guess(title: &str, body: &str) -> Self {
        let t = format!("{title} {body}").to_lowercase();
        let has = |words: &[&str]| {
            words.iter().any(|w| {
                t.split(|c: char| !c.is_alphanumeric())
                    .any(|token| token.starts_with(w))
            })
        };
        if has(&[
            "fail", "error", "crash", "broke", "denied", "rejected", "panic",
        ]) {
            Mood::Bad
        } else if has(&[
            "pass", "success", "succeed", "done", "finish", "complete", "merged", "deployed",
            "ready",
        ]) {
            Mood::Good
        } else {
            Mood::Neutral
        }
    }
}

impl Notification {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Notification {
            id: 0,
            title: title.into(),
            body: body.into(),
            timeout: Timeout::Default,
            source: None,
            url: None,
            copy: None,
            choices: Vec::new(),
            reply: None,
            expires: None,
            progress: None,
            announced: false,
            key: None,
            not_before: None,
            mood: Mood::Neutral,
        }
    }

    /// Sets the mood, or guesses it from the text when `None`.
    pub fn mood(mut self, m: Option<Mood>) -> Self {
        self.mood = m.unwrap_or_else(|| Mood::guess(&self.title, &self.body));
        self
    }

    pub fn sticky(mut self) -> Self {
        self.timeout = Timeout::Sticky;
        self
    }

    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Timeout::After(d);
        self
    }

    pub fn source(mut self, s: Option<String>) -> Self {
        self.source = s;
        self
    }

    /// A question with answer buttons. Returns the notification and the channel the chosen
    /// answer arrives on; the channel disconnects if it expires (after `wait`) unanswered.
    pub fn ask(mut self, choices: Vec<String>, wait: Duration) -> (Self, Receiver<String>) {
        let (tx, rx) = mpsc::channel();
        self.choices = choices;
        self.reply = Some(tx);
        self.timeout = Timeout::Sticky;
        self.expires = Some(Instant::now() + wait);
        (self, rx)
    }

    pub fn is_question(&self) -> bool {
        self.reply.is_some()
    }

    /// Notifications that must not be skipped in do-not-disturb mode.
    pub fn is_important(&self) -> bool {
        self.is_question() || self.timeout == Timeout::Sticky || self.progress.is_some()
    }

    /// Can be put off with "Later": plain news, not something an agent waits on.
    pub fn can_snooze(&self) -> bool {
        !self.is_question() && self.progress.is_none()
    }

    pub fn is_expired(&self, now: Instant) -> bool {
        self.expires.is_some_and(|e| now >= e)
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

/// Something for the dog to do (MCP `react`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reaction {
    Happy,
    Bark,
    Sleep,
    Wake,
    Zoomies,
    Sad,
    /// Throws the toy for the dog to fetch.
    Fetch,
}

impl Reaction {
    pub const NAMES: [&str; 7] = ["happy", "bark", "sleep", "wake", "zoomies", "sad", "fetch"];

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "happy" => Reaction::Happy,
            "bark" => Reaction::Bark,
            "sleep" => Reaction::Sleep,
            "wake" => Reaction::Wake,
            "zoomies" => Reaction::Zoomies,
            "sad" => Reaction::Sad,
            "fetch" => Reaction::Fetch,
            _ => return None,
        })
    }
}

#[derive(Debug)]
pub enum Event {
    Notify(Box<Notification>),
    React(Reaction),
    /// Withdraws a question or notification by its `key` (answered elsewhere).
    Dismiss(String),
}

impl Event {
    pub fn into_notification(self) -> Option<Notification> {
        match self {
            Event::Notify(n) => Some(*n),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct Notifier(Sender<Event>);

impl Notifier {
    /// Returns `false` if the app has shut down.
    pub fn send(&self, n: Notification) -> bool {
        self.0.send(Event::Notify(Box::new(n))).is_ok()
    }

    pub fn dismiss(&self, key: String) -> bool {
        self.0.send(Event::Dismiss(key)).is_ok()
    }

    pub fn react(&self, r: Reaction) -> bool {
        self.0.send(Event::React(r)).is_ok()
    }
}

pub fn channel() -> (Notifier, Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    (Notifier(tx), rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mood_from_words() {
        assert_eq!(Mood::guess("Build failed", ""), Mood::Bad);
        assert_eq!(Mood::guess("CI", "3 errors"), Mood::Bad);
        assert_eq!(Mood::guess("Tests passed", ""), Mood::Good);
        assert_eq!(Mood::guess("Done", "cargo build"), Mood::Good);
        assert_eq!(Mood::guess("New mail", "from Ayşe"), Mood::Neutral);
        // Whole words only: "redone" is not "done", "terror" is not "error".
        assert_eq!(Mood::guess("redone", "terror"), Mood::Neutral);
        let n = Notification::new("Deploy", "ok").mood(Some(Mood::Bad));
        assert_eq!(n.mood, Mood::Bad);
    }
}
