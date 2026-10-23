//! The user's own special days (birthdays, anniversaries...) and which of the built-in days are
//! on. Kept in `days.json` next to the settings; added from Settings → Special days or over MCP
//! (`day_add`). Behaviour is the same in every language; only the text follows it.

use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{Datelike, NaiveDate, Timelike};
use serde::{Deserialize, Serialize};

use crate::i18n::{t, tf};
use crate::special::Pin;

/// The built-in days, each with its own switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Builtin {
    /// 10 November: ribbon, minute of silence at 09:05, remembrance bubble.
    Remembrance,
    /// 23 April, 19 May, 30 August, 29 October: flag and greeting.
    National,
    /// The dog's own birthday: party hat.
    DogBirthday,
    /// 24-31 October: witch hat and pumpkin.
    Halloween,
    /// 20 December - 1 January: Santa hat.
    Christmas,
}

impl Builtin {
    pub const ALL: [Builtin; 5] = [
        Builtin::Remembrance,
        Builtin::National,
        Builtin::DogBirthday,
        Builtin::Halloween,
        Builtin::Christmas,
    ];
}

/// What kind of day it is: picks the default text and look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Birthday,
    Anniversary,
    Holiday,
    Other,
}

/// What the dog wears on the day.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Look {
    /// Birthday: party hat; others: nothing.
    #[default]
    Auto,
    None,
    PartyHat,
    Heart,
    Star,
    Ribbon,
    Flag,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Day {
    pub id: u64,
    /// Whose day or what it is ("Ayşe", "Wedding").
    pub name: String,
    #[serde(default)]
    pub kind: Kind,
    pub month: u32,
    pub day: u32,
    /// The year it started (birth, wedding): the bubble says how many years. For a one-off
    /// day, the year it happens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    /// Only once (in `year`), then it is removed.
    #[serde(default)]
    pub once: bool,
    /// The bubble's text; empty: a default for the kind, in the app's language.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    /// The bubble's time, "HH:MM" (default 09:00).
    #[serde(default = "nine")]
    pub at: String,
    /// Also remind this many days before (0: no).
    #[serde(default)]
    pub remind_days_before: u32,
    #[serde(default)]
    pub look: Look,
    /// Also to the phone (when the phone settings send at all).
    #[serde(default)]
    pub phone: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

fn nine() -> String {
    "09:00".into()
}

/// A request for a new day.
#[derive(Clone, Debug, PartialEq)]
pub struct NewDay {
    pub name: String,
    pub kind: Kind,
    pub month: u32,
    pub day: u32,
    pub year: Option<i32>,
    pub once: bool,
    pub message: String,
    pub at: String,
    pub remind_days_before: u32,
    pub look: Look,
    pub phone: bool,
    pub source: Option<String>,
}

/// Something due today for a day: the bubble, or the reminder before it.
#[derive(Clone, Debug, PartialEq)]
pub struct Due {
    pub id: u64,
    /// Days until the day (0: today).
    pub days_before: u32,
    pub title: String,
    pub body: String,
    pub phone: bool,
}

/// Longest name, in characters.
const NAME_MAX: usize = 60;
/// Longest message, in characters.
const MESSAGE_MAX: usize = 300;
/// Most days a reminder can come before.
const REMIND_MAX: u32 = 30;
const YEARS: std::ops::RangeInclusive<i32> = 1900..=2200;
/// How long a shown bubble is remembered (days).
const SHOWN_KEEP: i64 = 400;

/// What is kept in `days.json`.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    next_id: u64,
    days: Vec<Day>,
    /// Only the built-in days that are turned off.
    builtins_off: Vec<Builtin>,
    /// Bubbles already shown: (day id, the day's date "YYYY-MM-DD", days before).
    shown: Vec<(u64, String, u32)>,
}

/// The days and the built-in switches.
pub struct Days {
    state: Mutex<Stored>,
    path: Option<PathBuf>,
}

impl Days {
    /// Loads `path` (when given); every change is written back.
    pub fn new(path: Option<PathBuf>) -> Self {
        let stored = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|d| serde_json::from_slice::<Stored>(&d).ok())
            .unwrap_or_default();
        Days {
            state: Mutex::new(stored),
            path,
        }
    }

    /// The days, the next one first.
    pub fn list(&self) -> Vec<Day> {
        self.list_from(today())
    }

    fn list_from(&self, from: NaiveDate) -> Vec<Day> {
        let mut days = self.state.lock().unwrap().days.clone();
        days.sort_by_key(|d| (next_on(d, from).unwrap_or(NaiveDate::MAX), d.id));
        days
    }

    /// Adds a day; errors on a bad date, time or empty name.
    pub fn add(&self, n: NewDay) -> Result<Day, String> {
        self.add_on(n, today())
    }

    fn add_on(&self, n: NewDay, today: NaiveDate) -> Result<Day, String> {
        let name = n.name.trim().to_string();
        if name.is_empty() {
            return Err("the name is empty".into());
        }
        if name.chars().count() > NAME_MAX {
            return Err(format!("the name is longer than {NAME_MAX} characters"));
        }
        if !(1..=12).contains(&n.month) {
            return Err(format!("month {} is out of range 1-12", n.month));
        }
        // 2000 is a leap year, so 29 February passes.
        if NaiveDate::from_ymd_opt(2000, n.month, n.day).is_none() {
            return Err(format!(
                "{} {} is not a date",
                n.day,
                EN_MONTHS[n.month as usize - 1]
            ));
        }
        if let Some(y) = n.year
            && !YEARS.contains(&y)
        {
            return Err(format!(
                "year {y} is out of range {}-{}",
                YEARS.start(),
                YEARS.end()
            ));
        }
        if n.once {
            let Some(y) = n.year else {
                return Err("a one-off day needs a year".into());
            };
            let Some(date) = NaiveDate::from_ymd_opt(y, n.month, n.day) else {
                return Err(format!(
                    "{} {} {y} is not a date",
                    n.day,
                    EN_MONTHS[n.month as usize - 1]
                ));
            };
            if date < today {
                return Err(format!("{date} is in the past"));
            }
        }
        if n.remind_days_before > REMIND_MAX {
            return Err(format!("remind at most {REMIND_MAX} days before"));
        }
        let message = n.message.trim().to_string();
        if message.chars().count() > MESSAGE_MAX {
            return Err(format!(
                "the message is longer than {MESSAGE_MAX} characters"
            ));
        }
        let at = match n.at.trim() {
            "" => nine(),
            s => {
                let m = crate::settings::parse_hhmm(s)
                    .ok_or_else(|| format!("bad time \"{s}\"; use HH:MM"))?;
                format!("{:02}:{:02}", m / 60, m % 60)
            }
        };
        let mut st = self.state.lock().unwrap();
        st.next_id += 1;
        let day = Day {
            id: st.next_id,
            name,
            kind: n.kind,
            month: n.month,
            day: n.day,
            year: n.year,
            once: n.once,
            message,
            at,
            remind_days_before: n.remind_days_before,
            look: n.look,
            phone: n.phone,
            source: n.source,
        };
        st.days.push(day.clone());
        self.save(&st);
        Ok(day)
    }

    pub fn remove(&self, id: u64) -> bool {
        let mut st = self.state.lock().unwrap();
        let before = st.days.len();
        st.days.retain(|d| d.id != id);
        let removed = st.days.len() != before;
        if removed {
            st.shown.retain(|s| s.0 != id);
            self.save(&st);
        }
        removed
    }

    /// Whether a built-in day is on (all are by default).
    pub fn builtin_on(&self, b: Builtin) -> bool {
        !self.state.lock().unwrap().builtins_off.contains(&b)
    }

    pub fn set_builtin(&self, b: Builtin, on: bool) {
        let mut st = self.state.lock().unwrap();
        if on != st.builtins_off.contains(&b) {
            return;
        }
        if on {
            st.builtins_off.retain(|x| *x != b);
        } else {
            st.builtins_off.push(b);
        }
        self.save(&st);
    }

    /// The user's days on `date`.
    pub fn on(&self, date: NaiveDate) -> Vec<Day> {
        let mut days: Vec<Day> = (self.state.lock().unwrap().days.iter())
            .filter(|d| falls_on(d, date))
            .cloned()
            .collect();
        days.sort_by_key(|d| d.id);
        days
    }

    /// What is pinned on the collar on `date` for the user's days (the first that has one).
    pub fn pin(&self, date: NaiveDate) -> Option<Pin> {
        self.on(date).iter().find_map(pin_of)
    }

    /// Whether a user's day on `date` puts the party hat on.
    pub fn party_hat(&self, date: NaiveDate) -> bool {
        self.on(date)
            .iter()
            .any(|d| d.look == Look::PartyHat || (d.look == Look::Auto && d.kind == Kind::Birthday))
    }

    /// Bubbles due at local time `now` that were not shown yet today: each day's own at its
    /// time, and the "in N days" reminders. Marks them shown (also saved, so a restart does
    /// not repeat them); removes one-off days once they are over.
    pub fn take_due(&self, now: chrono::NaiveDateTime) -> Vec<Due> {
        let today = now.date();
        let minute = now.hour() * 60 + now.minute();
        let mut st = self.state.lock().unwrap();
        let st = &mut *st;
        let mut out = Vec::new();
        for d in &st.days {
            if minute < crate::settings::parse_hhmm(&d.at).unwrap_or(9 * 60) {
                continue;
            }
            let mut check = vec![(today, 0)];
            if d.remind_days_before > 0
                && let Some(t) =
                    today.checked_add_days(chrono::Days::new(d.remind_days_before.into()))
            {
                check.push((t, d.remind_days_before));
            }
            for (date, before) in check {
                let key = (d.id, date.format(DATE).to_string(), before);
                if !falls_on(d, date) || st.shown.contains(&key) {
                    continue;
                }
                let (title, body) = texts(d, date, before);
                out.push(Due {
                    id: d.id,
                    days_before: before,
                    title,
                    body,
                    phone: d.phone,
                });
                st.shown.push(key);
            }
        }
        let (days, shown) = (st.days.len(), st.shown.len());
        st.days.retain(|d| !d.once || next_on(d, today).is_some());
        let keep = today - chrono::TimeDelta::days(SHOWN_KEEP);
        st.shown
            .retain(|(_, date, _)| NaiveDate::parse_from_str(date, DATE).is_ok_and(|d| d >= keep));
        if !out.is_empty() || days != st.days.len() || shown != st.shown.len() {
            self.save(st);
        }
        out
    }

    /// The next date a day falls on, from `from` (inclusive).
    pub fn next_date(&self, d: &Day, from: NaiveDate) -> Option<NaiveDate> {
        next_on(d, from)
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

const DATE: &str = "%Y-%m-%d";

fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// The day's date in `year`; 29 February is 28 February in other years.
fn date_in(d: &Day, year: i32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, d.month, d.day).or_else(|| {
        (d.month == 2 && d.day == 29)
            .then(|| NaiveDate::from_ymd_opt(year, 2, 28))
            .flatten()
    })
}

/// Whether the day falls on `date`.
fn falls_on(d: &Day, date: NaiveDate) -> bool {
    if d.once && d.year != Some(date.year()) {
        return false;
    }
    date_in(d, date.year()) == Some(date)
}

/// The next date the day falls on, from `from` (inclusive).
fn next_on(d: &Day, from: NaiveDate) -> Option<NaiveDate> {
    if d.once {
        return date_in(d, d.year?).filter(|x| *x >= from);
    }
    (from.year()..=from.year() + 1)
        .filter_map(|y| date_in(d, y))
        .find(|x| *x >= from)
}

/// The collar pin a day asks for.
fn pin_of(d: &Day) -> Option<Pin> {
    match d.look {
        Look::Heart => Some(Pin::Heart),
        Look::Star => Some(Pin::Star),
        Look::Ribbon => Some(Pin::Ribbon),
        Look::Flag => Some(Pin::Flag),
        Look::Auto => match d.kind {
            Kind::Anniversary => Some(Pin::Heart),
            Kind::Holiday => Some(Pin::Star),
            Kind::Birthday | Kind::Other => None,
        },
        Look::PartyHat | Look::None => None,
    }
}

const EN_MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Turkish month names with their "on" suffix (Turkish grammar).
const TR_MONTHS: [&str; 12] = [
    "Ocak'ta",
    "Şubat'ta",
    "Mart'ta",
    "Nisan'da",
    "Mayıs'ta",
    "Haziran'da",
    "Temmuz'da",
    "Ağustos'ta",
    "Eylül'de",
    "Ekim'de",
    "Kasım'da",
    "Aralık'ta",
];

/// "14 March" in the app's language (month names are translated).
pub fn day_month(day: u32, month: u32) -> String {
    let m = match month {
        1 => t("January"),
        2 => t("February"),
        3 => t("March"),
        4 => t("April"),
        5 => t("May"),
        6 => t("June"),
        7 => t("July"),
        8 => t("August"),
        9 => t("September"),
        10 => t("October"),
        11 => t("November"),
        _ => t("December"),
    };
    tf("{n} {month}", &[("n", &day.to_string()), ("month", m)])
}

/// The bubble for a day on `date`, `before` days ahead (0: today): (title, body).
fn texts(d: &Day, date: NaiveDate, before: u32) -> (String, String) {
    let name = d.name.as_str();
    let turkish = crate::i18n::is_turkish();
    if before > 0 {
        let n = before.to_string();
        let title = match before {
            1 => tf("{name} tomorrow", &[("name", name)]),
            _ => tf("{name} in {n} days", &[("name", name), ("n", &n)]),
        };
        let kind = match d.kind {
            Kind::Birthday => t("Birthday"),
            Kind::Anniversary => t("Anniversary"),
            _ => name,
        };
        // Turkish puts "on" on the month: 14 Mart'ta.
        let on = if turkish {
            format!("{} {}", date.day(), TR_MONTHS[date.month0() as usize])
        } else {
            day_month(date.day(), date.month())
        };
        let body = tf("{name} on {date}.", &[("name", kind), ("date", &on)]);
        return (title, body);
    }
    let years = d
        .year
        .filter(|_| !d.once)
        .map(|y| date.year() - y)
        .filter(|n| *n > 0);
    // Turkish says whose birthday with a suffix: Ayşe'nin.
    let whose = if turkish {
        genitive(name)
    } else {
        name.to_string()
    };
    let (title, body) = match d.kind {
        Kind::Birthday => (
            tf("🎂 {name}'s birthday", &[("name", &whose)]),
            match years {
                Some(n) => tf(
                    "{name} is {n} today!",
                    &[("name", name), ("n", &n.to_string())],
                ),
                None => tf("Wish {name} a happy birthday!", &[("name", &whose)]),
            },
        ),
        Kind::Anniversary => (
            format!("💞 {name}"),
            match years {
                Some(n) => tf("{n} years today.", &[("n", &n.to_string())]),
                None => t("Today is the day.").into(),
            },
        ),
        _ => (
            format!("⭐ {name}"),
            tf("Today is {name}.", &[("name", name)]),
        ),
    };
    let body = if d.message.is_empty() {
        body
    } else {
        d.message.clone()
    };
    (title, body)
}

/// A name with the Turkish possessive ("of") suffix: Ayşe'nin, Mehmet'in, Oya'nın, Umut'un.
/// The last vowel picks ı/i/u/ü; after a vowel an n comes first.
pub fn genitive(name: &str) -> String {
    let name = name.trim();
    let lower = |c: char| match c {
        'I' => 'ı',
        'İ' => 'i',
        c => c.to_lowercase().next().unwrap_or(c),
    };
    let vowel = |c: char| "aeıioöuü".contains(c);
    let last_vowel = name.chars().rev().map(lower).find(|c| vowel(*c));
    let suffix = match last_vowel {
        Some('a' | 'ı') => 'ı',
        Some('o' | 'u') => 'u',
        Some('ö' | 'ü') => 'ü',
        _ => 'i',
    };
    let n = if name.chars().last().map(lower).is_some_and(vowel) {
        "n"
    } else {
        ""
    };
    format!("{name}'{n}{suffix}n")
}

/// Where the days are kept: `days.json` next to the settings.
pub fn default_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("days.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::with_lang;
    use crate::settings::Lang;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn at(y: i32, m: u32, d: u32, h: u32, mi: u32) -> chrono::NaiveDateTime {
        date(y, m, d).and_hms_opt(h, mi, 0).unwrap()
    }

    fn new_day(name: &str, kind: Kind, month: u32, day: u32) -> NewDay {
        NewDay {
            name: name.into(),
            kind,
            month,
            day,
            year: None,
            once: false,
            message: String::new(),
            at: String::new(),
            remind_days_before: 0,
            look: Look::Auto,
            phone: false,
            source: None,
        }
    }

    fn temp(name: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("dost-days-{name}-{}-{n}", std::process::id()))
            .join("days.json")
    }

    #[test]
    fn validation() {
        let days = Days::new(None);
        let today = date(2026, 10, 22);
        let err = |n: NewDay| days.add_on(n, today).unwrap_err();
        assert!(err(new_day("  ", Kind::Birthday, 1, 1)).contains("empty"));
        assert!(err(new_day(&"x".repeat(61), Kind::Birthday, 1, 1)).contains("60"));
        assert!(err(new_day("A", Kind::Birthday, 13, 1)).contains("month 13"));
        assert!(err(new_day("A", Kind::Birthday, 4, 31)).contains("not a date"));
        assert!(err(new_day("A", Kind::Birthday, 0, 1)).contains("month 0"));
        let mut n = new_day("A", Kind::Other, 1, 1);
        n.at = "25:00".into();
        assert!(err(n).contains("HH:MM"));
        let mut n = new_day("A", Kind::Other, 1, 1);
        n.once = true;
        assert!(err(n).contains("needs a year"));
        let mut n = new_day("A", Kind::Other, 2, 29);
        n.once = true;
        n.year = Some(2027);
        assert!(err(n).contains("not a date"));
        let mut n = new_day("A", Kind::Other, 10, 21);
        n.once = true;
        n.year = Some(2026);
        assert!(err(n).contains("past"));
        let mut n = new_day("A", Kind::Other, 1, 1);
        n.remind_days_before = 31;
        assert!(err(n).contains("30"));
        let mut n = new_day("A", Kind::Other, 1, 1);
        n.year = Some(1899);
        assert!(err(n).contains("1900"));
        let mut n = new_day("A", Kind::Other, 1, 1);
        n.message = "x".repeat(301);
        assert!(err(n).contains("300"));

        let mut n = new_day(" Ayşe ", Kind::Birthday, 2, 29);
        n.at = "8:5".into();
        let d = days.add_on(n, today).unwrap();
        assert_eq!((d.name.as_str(), d.at.as_str(), d.id), ("Ayşe", "08:05", 1));
        let mut n = new_day("Trip", Kind::Other, 10, 22);
        n.once = true;
        n.year = Some(2026);
        assert_eq!(days.add_on(n, today).unwrap().at, "09:00");
    }

    #[test]
    fn leap_day() {
        let days = Days::new(None);
        let d = days
            .add_on(new_day("Leap", Kind::Birthday, 2, 29), date(2026, 1, 1))
            .unwrap();
        assert_eq!(days.on(date(2027, 2, 28)).len(), 1);
        assert!(days.on(date(2027, 3, 1)).is_empty());
        assert_eq!(days.on(date(2028, 2, 29)).len(), 1);
        assert!(days.on(date(2028, 2, 28)).is_empty());
        assert_eq!(
            days.next_date(&d, date(2027, 3, 1)),
            Some(date(2028, 2, 29))
        );
        assert_eq!(
            days.next_date(&d, date(2026, 1, 1)),
            Some(date(2026, 2, 28))
        );
    }

    #[test]
    fn order_across_year_end() {
        let days = Days::new(None);
        let from = date(2026, 12, 20);
        let jan = days
            .add_on(new_day("Jan", Kind::Other, 1, 5), from)
            .unwrap();
        let dec = days
            .add_on(new_day("Dec", Kind::Other, 12, 25), from)
            .unwrap();
        let mut n = new_day("Once", Kind::Other, 12, 31);
        n.once = true;
        n.year = Some(2026);
        days.add_on(n, from).unwrap();
        assert_eq!(days.next_date(&jan, from), Some(date(2027, 1, 5)));
        assert_eq!(days.next_date(&dec, from), Some(date(2026, 12, 25)));
        let names: Vec<String> = days.list_from(from).into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["Dec", "Once", "Jan"]);
        let names: Vec<String> = days
            .list_from(date(2027, 1, 1))
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, ["Jan", "Dec", "Once"]);
    }

    #[test]
    fn once_days_go_after_their_bubble() {
        let days = Days::new(None);
        let mut n = new_day("Concert", Kind::Other, 10, 22);
        n.once = true;
        n.year = Some(2026);
        n.at = "18:00".into();
        days.add_on(n, date(2026, 10, 1)).unwrap();
        assert!(days.on(date(2027, 10, 22)).is_empty());
        assert!(days.take_due(at(2026, 10, 22, 17, 0)).is_empty());
        assert_eq!(days.take_due(at(2026, 10, 22, 18, 0)).len(), 1);
        assert_eq!(days.list_from(date(2026, 10, 22)).len(), 1);
        assert!(days.take_due(at(2026, 10, 23, 9, 0)).is_empty());
        assert!(days.list_from(date(2026, 10, 23)).is_empty());
    }

    #[test]
    fn pins_and_hats() {
        let days = Days::new(None);
        let today = date(2026, 1, 1);
        let add = |name: &str, kind, day, look| {
            let mut n = new_day(name, kind, 3, day);
            n.look = look;
            days.add_on(n, today).unwrap();
        };
        add("b", Kind::Birthday, 1, Look::Auto);
        add("a", Kind::Anniversary, 2, Look::Auto);
        add("h", Kind::Holiday, 3, Look::Auto);
        add("o", Kind::Other, 4, Look::Auto);
        add("s", Kind::Birthday, 5, Look::Star);
        add("p", Kind::Other, 6, Look::PartyHat);
        add("n", Kind::Birthday, 7, Look::None);
        add("f", Kind::Other, 8, Look::Flag);
        add("r", Kind::Other, 8, Look::Ribbon);
        let on = |d| (days.pin(date(2026, 3, d)), days.party_hat(date(2026, 3, d)));
        assert_eq!(on(1), (None, true));
        assert_eq!(on(2), (Some(Pin::Heart), false));
        assert_eq!(on(3), (Some(Pin::Star), false));
        assert_eq!(on(4), (None, false));
        assert_eq!(on(5), (Some(Pin::Star), false));
        assert_eq!(on(6), (None, true));
        assert_eq!(on(7), (None, false));
        assert_eq!(on(8), (Some(Pin::Flag), false));
        assert_eq!(on(9), (None, false));
    }

    #[test]
    fn builtins() {
        let path = temp("builtins");
        let days = Days::new(Some(path.clone()));
        assert!(Builtin::ALL.iter().all(|b| days.builtin_on(*b)));
        days.set_builtin(Builtin::Halloween, false);
        days.set_builtin(Builtin::Halloween, false);
        assert!(!days.builtin_on(Builtin::Halloween));
        let again = Days::new(Some(path.clone()));
        assert!(!again.builtin_on(Builtin::Halloween));
        assert!(again.builtin_on(Builtin::Christmas));
        again.set_builtin(Builtin::Halloween, true);
        assert!(Days::new(Some(path.clone())).builtin_on(Builtin::Halloween));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn old_file_loads() {
        let path = temp("old");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"days":[{"id":3,"name":"Ali","month":5,"day":1}]}"#,
        )
        .unwrap();
        let days = Days::new(Some(path.clone()));
        let d = &days.on(date(2026, 5, 1))[0];
        assert_eq!(
            (d.kind, d.at.as_str(), d.look),
            (Kind::Birthday, "09:00", Look::Auto)
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn due_today_reminder_once_and_after_restart() {
        let path = temp("due");
        let days = Days::new(Some(path.clone()));
        let mut n = new_day("Ayşe", Kind::Birthday, 3, 14);
        n.year = Some(1990);
        n.at = "10:00".into();
        n.remind_days_before = 3;
        n.phone = true;
        let d = days.add_on(n, date(2026, 1, 1)).unwrap();

        assert!(days.take_due(at(2026, 3, 11, 9, 59)).is_empty());
        let due = days.take_due(at(2026, 3, 11, 10, 0));
        assert_eq!(
            due,
            [Due {
                id: d.id,
                days_before: 3,
                title: "Ayşe in 3 days".into(),
                body: "Birthday on 14 March.".into(),
                phone: true,
            }]
        );
        assert!(days.take_due(at(2026, 3, 11, 12, 0)).is_empty());
        assert!(days.take_due(at(2026, 3, 12, 12, 0)).is_empty());

        let again = Days::new(Some(path.clone()));
        assert!(again.take_due(at(2026, 3, 11, 13, 0)).is_empty());
        with_lang(Lang::Tr, || {
            assert!(again.take_due(at(2026, 3, 14, 9, 0)).is_empty());
            let due = again.take_due(at(2026, 3, 14, 10, 30));
            assert_eq!(due.len(), 1);
            assert_eq!(due[0].days_before, 0);
            assert_eq!(due[0].title, "🎂 Ayşe'nin doğum günü");
            assert_eq!(due[0].body, "Ayşe bugün 36 yaşında!");
            assert!(
                Days::new(Some(path.clone()))
                    .take_due(at(2026, 3, 14, 23, 0))
                    .is_empty()
            );
        });
        // Next year again.
        assert_eq!(again.take_due(at(2027, 3, 14, 10, 0)).len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn genitives() {
        for (name, want) in [
            ("Ayşe", "Ayşe'nin"),
            ("Ali", "Ali'nin"),
            ("Mehmet", "Mehmet'in"),
            ("Oya", "Oya'nın"),
            ("Umut", "Umut'un"),
            ("Gül", "Gül'ün"),
            ("Ahmet", "Ahmet'in"),
            ("Kadir", "Kadir'in"),
            ("Burak", "Burak'ın"),
            ("Ömür", "Ömür'ün"),
            ("Selin", "Selin'in"),
            ("Duru", "Duru'nun"),
            ("Ülkü", "Ülkü'nün"),
            ("Okyanus", "Okyanus'un"),
            ("IŞIK", "IŞIK'ın"),
        ] {
            assert_eq!(genitive(name), want);
        }
    }

    fn day(name: &str, kind: Kind, year: Option<i32>, message: &str) -> Day {
        Day {
            id: 1,
            name: name.into(),
            kind,
            month: 3,
            day: 14,
            year,
            once: false,
            message: message.into(),
            at: nine(),
            remind_days_before: 0,
            look: Look::Auto,
            phone: false,
            source: None,
        }
    }

    #[test]
    fn bubble_texts() {
        let on = date(2026, 3, 14);
        let t = |d: &Day, before, tr: bool| {
            with_lang(if tr { Lang::Tr } else { Lang::En }, || {
                texts(d, on, before)
            })
        };
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());

        let b = day("Oya", Kind::Birthday, Some(2000), "");
        assert_eq!(t(&b, 0, false), s("🎂 Oya's birthday", "Oya is 26 today!"));
        assert_eq!(
            t(&b, 0, true),
            s("🎂 Oya'nın doğum günü", "Oya bugün 26 yaşında!")
        );
        let b = day("Umut", Kind::Birthday, None, "");
        assert_eq!(
            t(&b, 0, false),
            s("🎂 Umut's birthday", "Wish Umut a happy birthday!")
        );
        assert_eq!(
            t(&b, 0, true),
            s(
                "🎂 Umut'un doğum günü",
                "Umut'un doğum gününü kutlamayı unutma!"
            )
        );
        assert_eq!(t(&b, 1, false), s("Umut tomorrow", "Birthday on 14 March."));
        assert_eq!(t(&b, 1, true), s("Yarın: Umut", "Doğum günü 14 Mart'ta."));

        let a = day("Wedding", Kind::Anniversary, Some(2016), "");
        assert_eq!(t(&a, 0, false), s("💞 Wedding", "10 years today."));
        assert_eq!(t(&a, 0, true), s("💞 Wedding", "Bugün 10. yıl."));
        assert_eq!(
            t(&a, 5, true),
            s("5 gün sonra: Wedding", "Yıl dönümü 14 Mart'ta.")
        );
        assert_eq!(
            t(&a, 5, false),
            s("Wedding in 5 days", "Anniversary on 14 March.")
        );
        // The same year: no "0 years".
        let a = day("Wedding", Kind::Anniversary, Some(2026), "");
        assert_eq!(t(&a, 0, false), s("💞 Wedding", "Today is the day."));
        assert_eq!(t(&a, 0, true), s("💞 Wedding", "Bugün o gün."));

        let h = day("Pi Day", Kind::Holiday, None, "");
        assert_eq!(t(&h, 0, false), s("⭐ Pi Day", "Today is Pi Day."));
        assert_eq!(t(&h, 0, true), s("⭐ Pi Day", "Bugün: Pi Day."));
        assert_eq!(
            t(&h, 2, false),
            s("Pi Day in 2 days", "Pi Day on 14 March.")
        );

        let m = day("Ali", Kind::Birthday, Some(2000), "Cake!");
        assert_eq!(t(&m, 0, true), s("🎂 Ali'nin doğum günü", "Cake!"));
    }
}
