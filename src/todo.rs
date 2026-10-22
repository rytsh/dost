//! Todo lists: named lists of items the user (or an agent, over MCP) keeps with the dog, e.g.
//! "Release 0.3: tag, build, write notes". An item can have a reminder: the dog barks with it at
//! that time (a `remind` watch, removed when the item is done or deleted). Saved to
//! `todos.json` in the config folder on every change.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Longest list name and item text.
pub const NAME_MAX: usize = 60;
pub const TEXT_MAX: usize = 300;
/// The list items go to when none is named.
pub const DEFAULT_LIST: &str = "Todo";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    pub text: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Unix seconds of the reminder, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remind_at: Option<u64>,
    /// The watch that barks the reminder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<u64>,
    /// Unix seconds.
    #[serde(default)]
    pub created: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct List {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub items: Vec<Item>,
}

impl List {
    pub fn open_count(&self) -> usize {
        self.items.iter().filter(|i| !i.done).count()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Stored {
    next_id: u64,
    lists: Vec<List>,
}

pub struct Todos {
    stored: Mutex<Stored>,
    path: Option<PathBuf>,
}

/// A change made in the settings panel.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    NewList(String),
    Add {
        list: String,
        text: String,
    },
    /// Checks an item off, or back on.
    Toggle(u64),
    Remove(u64),
    RemoveList(u64),
    ClearDone(u64),
}

/// What changed, for the caller to set or remove the item's reminder watch.
#[derive(Debug, PartialEq)]
pub struct Changed {
    pub list: String,
    pub item: Item,
}

pub fn default_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("todos.json"))
}

fn clean(s: &str, max: usize) -> String {
    let s: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

impl Todos {
    pub fn new(path: Option<PathBuf>) -> Self {
        let stored = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        Todos {
            stored: Mutex::new(stored),
            path,
        }
    }

    pub fn lists(&self) -> Vec<List> {
        self.stored.lock().unwrap().lists.clone()
    }

    /// A list by id or by name (case-insensitive).
    fn find<'a>(lists: &'a mut [List], key: &str) -> Option<&'a mut List> {
        let key = key.trim();
        if let Ok(id) = key.trim_start_matches('#').parse::<u64>()
            && let Some(i) = lists.iter().position(|l| l.id == id)
        {
            return Some(&mut lists[i]);
        }
        lists
            .iter_mut()
            .find(|l| l.name.to_lowercase() == key.to_lowercase())
    }

    /// Creates a list (or returns the one with that name) and adds `items` to it.
    pub fn create_list(&self, name: &str, items: &[String]) -> Result<List, String> {
        let name = clean(name, NAME_MAX);
        if name.is_empty() {
            return Err("the list needs a name".into());
        }
        let mut st = self.stored.lock().unwrap();
        if Self::find(&mut st.lists, &name).is_none() {
            st.next_id += 1;
            let id = st.next_id;
            st.lists.push(List {
                id,
                name: name.clone(),
                items: Vec::new(),
            });
        }
        for t in items {
            Self::push(&mut st, &name, t, "", None)?;
        }
        let list = Self::find(&mut st.lists, &name).cloned().unwrap();
        self.save(&st);
        Ok(list)
    }

    fn push(
        st: &mut Stored,
        list: &str,
        text: &str,
        note: &str,
        remind_at: Option<u64>,
    ) -> Result<Item, String> {
        let text = clean(text, TEXT_MAX);
        if text.is_empty() {
            return Err("the item needs text".into());
        }
        st.next_id += 1;
        let item = Item {
            id: st.next_id,
            text,
            done: false,
            note: clean(note, TEXT_MAX),
            remind_at,
            watch: None,
            created: crate::watch::now_unix(),
        };
        let l = Self::find(&mut st.lists, list).ok_or_else(|| format!("no list \"{list}\""))?;
        l.items.push(item.clone());
        Ok(item)
    }

    /// Adds an item; the list is created when it does not exist (`DEFAULT_LIST` when empty).
    pub fn add(
        &self,
        list: &str,
        text: &str,
        note: &str,
        remind_at: Option<u64>,
    ) -> Result<Changed, String> {
        let list = if list.trim().is_empty() {
            DEFAULT_LIST.to_string()
        } else {
            list.to_string()
        };
        // Checked first, so a bad item does not leave a new empty list behind.
        if clean(text, TEXT_MAX).is_empty() {
            return Err("the item needs text".into());
        }
        let mut st = self.stored.lock().unwrap();
        if Self::find(&mut st.lists, &list).is_none() {
            drop(st);
            self.create_list(&list, &[])?;
            st = self.stored.lock().unwrap();
        }
        let item = Self::push(&mut st, &list, text, note, remind_at)?;
        let name = Self::find(&mut st.lists, &list).unwrap().name.clone();
        self.save(&st);
        Ok(Changed { list: name, item })
    }

    /// Changes an item (by id, in any list); `None` leaves a field as it is.
    pub fn update(
        &self,
        id: u64,
        done: Option<bool>,
        text: Option<&str>,
        note: Option<&str>,
        remind_at: Option<Option<u64>>,
    ) -> Result<Changed, String> {
        let mut st = self.stored.lock().unwrap();
        let (list, item) = st
            .lists
            .iter_mut()
            .find_map(|l| {
                let name = l.name.clone();
                l.items.iter_mut().find(|i| i.id == id).map(|i| (name, i))
            })
            .ok_or_else(|| format!("no item #{id}"))?;
        if let Some(d) = done {
            item.done = d;
        }
        if let Some(t) = text {
            let t = clean(t, TEXT_MAX);
            if t.is_empty() {
                return Err("the item needs text".into());
            }
            item.text = t;
        }
        if let Some(n) = note {
            item.note = clean(n, TEXT_MAX);
        }
        if let Some(r) = remind_at {
            item.remind_at = r;
        }
        let changed = Changed {
            list,
            item: item.clone(),
        };
        self.save(&st);
        Ok(changed)
    }

    /// Records the watch that barks an item's reminder.
    pub fn set_watch(&self, id: u64, watch: Option<u64>) {
        let mut st = self.stored.lock().unwrap();
        if let Some(i) = st
            .lists
            .iter_mut()
            .flat_map(|l| l.items.iter_mut())
            .find(|i| i.id == id)
        {
            i.watch = watch;
            self.save(&st);
        }
    }

    /// Removes an item; returns it (for its reminder watch).
    pub fn remove_item(&self, id: u64) -> Result<Item, String> {
        let mut st = self.stored.lock().unwrap();
        for l in &mut st.lists {
            if let Some(i) = l.items.iter().position(|i| i.id == id) {
                let item = l.items.remove(i);
                self.save(&st);
                return Ok(item);
            }
        }
        Err(format!("no item #{id}"))
    }

    /// Removes a list (by id or name) and its items; returns them (for their watches).
    pub fn remove_list(&self, key: &str) -> Result<List, String> {
        let mut st = self.stored.lock().unwrap();
        let id = Self::find(&mut st.lists, key)
            .map(|l| l.id)
            .ok_or_else(|| format!("no list \"{key}\""))?;
        let i = st.lists.iter().position(|l| l.id == id).unwrap();
        let list = st.lists.remove(i);
        self.save(&st);
        Ok(list)
    }

    /// Removes the done items of a list; returns how many.
    pub fn clear_done(&self, key: &str) -> Result<usize, String> {
        let mut st = self.stored.lock().unwrap();
        let l = Self::find(&mut st.lists, key).ok_or_else(|| format!("no list \"{key}\""))?;
        let before = l.items.len();
        l.items.retain(|i| !i.done);
        let n = before - l.items.len();
        self.save(&st);
        Ok(n)
    }

    /// One list (by id or name).
    pub fn list(&self, key: &str) -> Option<List> {
        let mut st = self.stored.lock().unwrap();
        Self::find(&mut st.lists, key).cloned()
    }

    fn save(&self, st: &Stored) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(data) = serde_json::to_vec_pretty(st) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, data).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }
}

/// A list as text for an agent: "Release 0.3 (#4, 2 open)\n  [ ] #5 tag ...".
pub fn render(l: &List) -> String {
    let mut s = format!("{} (#{}, {} open)", l.name, l.id, l.open_count());
    if l.items.is_empty() {
        s.push_str("\n  (empty)");
    }
    for i in &l.items {
        s.push_str(&format!(
            "\n  [{}] #{} {}",
            if i.done { "x" } else { " " },
            i.id,
            i.text
        ));
        if let Some(at) = i.remind_at {
            s.push_str(&format!(" (reminder {})", local_time(at)));
        }
        if !i.note.is_empty() {
            s.push_str(&format!("\n      {}", i.note));
        }
    }
    s
}

/// "15:30" today, "Tue 15:30" within a week, "3 Mar 15:30" further away; local time.
pub fn local_time(at: u64) -> String {
    use chrono::{Local, TimeZone};
    let chrono::LocalResult::Single(t) = Local.timestamp_opt(at as i64, 0) else {
        return at.to_string();
    };
    let now = Local::now();
    let days = (t.date_naive() - now.date_naive()).num_days();
    if days == 0 {
        t.format("%H:%M").to_string()
    } else if (0..7).contains(&days) {
        t.format("%a %H:%M").to_string()
    } else {
        t.format("%-d %b %H:%M").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_items() {
        let dir = std::env::temp_dir().join(format!("dost-todo-{}", std::process::id()));
        let path = dir.join("todos.json");
        let t = Todos::new(Some(path.clone()));
        let l = t
            .create_list("Release 0.3", &["tag".into(), "build".into()])
            .unwrap();
        assert_eq!(l.items.len(), 2);
        // Same name, any case: the same list.
        assert_eq!(t.create_list("release 0.3", &[]).unwrap().id, l.id);
        let c = t.add("", "buy milk", "", Some(42)).unwrap();
        assert_eq!(c.list, DEFAULT_LIST);
        assert_eq!(c.item.remind_at, Some(42));
        let tag = l.items[0].id;
        t.update(tag, Some(true), None, Some("v0.3.0"), None)
            .unwrap();
        assert_eq!(t.list(&format!("#{}", l.id)).unwrap().open_count(), 1);
        assert!(render(&t.list("Release 0.3").unwrap()).contains("[x]"));
        assert_eq!(t.clear_done("Release 0.3").unwrap(), 1);
        assert!(t.update(999, Some(true), None, None, None).is_err());
        assert!(t.add("x", "  ", "", None).is_err());

        // Saved and loaded back.
        let again = Todos::new(Some(path));
        assert_eq!(again.lists(), t.lists());
        assert_eq!(again.remove_list("todo").unwrap().items.len(), 1);
        assert_eq!(again.lists().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
