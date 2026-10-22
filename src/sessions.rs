//! Agent sessions: what each coding agent (an OpenCode session, a Claude Code or Codex
//! terminal) is doing right now, so many of them can be followed at a glance. Agents report
//! through MCP (`session`), the hooks and the OpenCode plugin; questions and progress bars
//! from a session update it too.

use std::sync::Mutex;

use serde::Serialize;

/// Finished sessions are forgotten after this long (seconds).
const KEEP_DONE: u64 = 30 * 60;
/// A session that has said nothing for this long is forgotten (seconds).
const KEEP_SILENT: u64 = 3 * 60 * 60;
/// At most this many are kept.
const MAX: usize = 20;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Working,
    /// Waiting for the user: a question, a permission, its turn is over.
    Waiting,
    Done,
    Failed,
    Idle,
}

impl State {
    pub const NAMES: [&str; 5] = ["working", "waiting", "done", "failed", "idle"];

    pub fn parse(s: &str) -> Option<State> {
        Some(match s {
            "working" | "running" | "busy" => State::Working,
            "waiting" | "asking" | "your_turn" => State::Waiting,
            "done" | "finished" | "succeeded" => State::Done,
            "failed" | "error" => State::Failed,
            "idle" => State::Idle,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            State::Working => "working",
            State::Waiting => "waiting for you",
            State::Done => "done",
            State::Failed => "failed",
            State::Idle => "idle",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Session {
    /// The agent's own id for it.
    pub id: String,
    /// Which agent ("OpenCode", "Claude Code").
    pub source: String,
    /// What it works on (the session's title).
    pub title: String,
    /// The project folder's name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project: String,
    pub state: State,
    /// The current step or the last message.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Opened when the session is clicked: a link, or an app to bring to the front.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
    /// Unix seconds: when it started working, and when it last reported.
    pub since: u64,
    pub updated: u64,
}

/// A report about a session; fields left `None` keep their value.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub id: String,
    pub source: Option<String>,
    pub title: Option<String>,
    pub project: Option<String>,
    pub state: Option<State>,
    pub detail: Option<String>,
    pub open: Option<String>,
    /// The session ended: it is removed.
    pub end: bool,
}

#[derive(Default)]
pub struct Sessions {
    list: Mutex<Vec<Session>>,
}

impl Sessions {
    /// Applies a report at `now` (Unix seconds). Returns whether anything changed.
    pub fn report(&self, r: Report, now: u64) -> bool {
        let Ok(mut list) = self.list.lock() else {
            return false;
        };
        if r.end {
            let before = list.len();
            list.retain(|s| s.id != r.id);
            return list.len() != before;
        }
        let i = match list.iter().position(|s| s.id == r.id) {
            Some(i) => i,
            None => {
                list.push(Session {
                    id: r.id.clone(),
                    source: r.source.clone().unwrap_or_else(|| "Agent".into()),
                    since: now,
                    ..Session::default()
                });
                list.len() - 1
            }
        };
        let s = &mut list[i];
        let old = s.clone();
        if let Some(v) = r.source {
            s.source = v;
        }
        if let Some(v) = r.title {
            s.title = v;
        }
        if let Some(v) = r.project {
            s.project = v;
        }
        if let Some(v) = r.detail {
            s.detail = v;
        }
        if let Some(v) = r.open {
            s.open = Some(v).filter(|o| !o.is_empty());
        }
        if let Some(st) = r.state {
            if st == State::Working && s.state != State::Working {
                s.since = now;
            }
            s.state = st;
        }
        s.updated = now;
        let changed = *s != old;
        Self::prune(&mut list, now);
        changed
    }

    /// Forgets finished and silent sessions. Returns whether any went.
    pub fn expire(&self, now: u64) -> bool {
        let Ok(mut list) = self.list.lock() else {
            return false;
        };
        let before = list.len();
        Self::prune(&mut list, now);
        list.len() != before
    }

    fn prune(list: &mut Vec<Session>, now: u64) {
        list.retain(|s| {
            let age = now.saturating_sub(s.updated);
            match s.state {
                State::Done | State::Failed | State::Idle => age < KEEP_DONE,
                _ => age < KEEP_SILENT,
            }
        });
        if list.len() > MAX {
            list.sort_by_key(|s| std::cmp::Reverse(s.updated));
            list.truncate(MAX);
        }
    }

    /// Waiting ones first, then working, then the rest; newest first within each.
    pub fn list(&self) -> Vec<Session> {
        let mut v = self.list.lock().map(|l| l.clone()).unwrap_or_default();
        let rank = |s: &Session| match s.state {
            State::Waiting => 0,
            State::Working => 1,
            State::Failed => 2,
            State::Done => 3,
            State::Idle => 4,
        };
        v.sort_by(|a, b| rank(a).cmp(&rank(b)).then(b.updated.cmp(&a.updated)));
        v
    }

    pub fn remove(&self, id: &str) -> bool {
        self.report(
            Report {
                id: id.into(),
                end: true,
                ..Report::default()
            },
            0,
        )
    }

    /// Removes finished ones (the panel's "Clear").
    pub fn clear_finished(&self) {
        if let Ok(mut list) = self.list.lock() {
            list.retain(|s| matches!(s.state, State::Working | State::Waiting));
        }
    }

    pub fn waiting(&self) -> usize {
        self.list
            .lock()
            .map(|l| l.iter().filter(|s| s.state == State::Waiting).count())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(id: &str, state: State) -> Report {
        Report {
            id: id.into(),
            source: Some("OpenCode".into()),
            state: Some(state),
            ..Report::default()
        }
    }

    #[test]
    fn report_order_and_expiry() {
        let s = Sessions::default();
        assert!(s.report(rep("a", State::Working), 100));
        assert!(s.report(rep("b", State::Waiting), 110));
        assert!(!s.report(rep("b", State::Waiting), 110), "same again");
        s.report(rep("c", State::Done), 120);
        let ids: Vec<String> = s.list().into_iter().map(|x| x.id).collect();
        assert_eq!(ids, ["b", "a", "c"]);
        assert_eq!(s.waiting(), 1);
        // Done ones go after a while; working ones stay longer.
        assert!(s.expire(120 + KEEP_DONE));
        assert_eq!(s.list().len(), 2);
        assert!(s.remove("a"));
        s.clear_finished();
        assert_eq!(s.list().len(), 1);
    }

    #[test]
    fn working_again_restarts_the_clock() {
        let s = Sessions::default();
        s.report(rep("a", State::Working), 100);
        s.report(rep("a", State::Waiting), 200);
        s.report(rep("a", State::Working), 300);
        assert_eq!(s.list()[0].since, 300);
        s.report(
            Report {
                id: "a".into(),
                detail: Some("bash".into()),
                ..Report::default()
            },
            310,
        );
        let a = &s.list()[0];
        assert_eq!(
            (a.since, a.detail.as_str(), a.state),
            (300, "bash", State::Working)
        );
    }
}
