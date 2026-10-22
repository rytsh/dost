//! Special days, under one switch (settings → General → *Special days*):
//!
//! - **10 November**: Atatürk's day of remembrance. All day the dog wears a black ribbon on
//!   its collar. At 09:05 it stops, sits still and stays silent for a minute (notifications
//!   wait, nothing barks; a siren if the user wants one), then a quiet bubble remembers him.
//! - **National holidays** (23 April, 19 May, 30 August, 29 October): a little Turkish flag
//!   on the collar.

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

/// What a day brings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Day {
    /// 10 November.
    Remembrance,
    /// 23 April, 19 May, 30 August, 29 October.
    National,
}

impl Day {
    pub fn on(d: NaiveDate) -> Option<Day> {
        match (d.month(), d.day()) {
            (11, 10) => Some(Day::Remembrance),
            (4, 23) | (5, 19) | (8, 30) | (10, 29) => Some(Day::National),
            _ => None,
        }
    }
}

/// The minute of silence starts at 09:05 and lasts this long (seconds).
pub const SILENCE_SECS: u32 = 60;
const SILENCE_AT: u32 = 9 * 3600 + 5 * 60;

/// Seconds of the minute of silence left at local time `now` (`None` outside it).
pub fn silence_left(now: NaiveDateTime) -> Option<u32> {
    if Day::on(now.date()) != Some(Day::Remembrance) {
        return None;
    }
    let s = now.num_seconds_from_midnight();
    (SILENCE_AT..SILENCE_AT + SILENCE_SECS)
        .contains(&s)
        .then(|| SILENCE_AT + SILENCE_SECS - s)
}

/// Whether the remembrance bubble is due at `now`: on 10 November, once the minute of
/// silence is over (or right away when the app starts later that day).
pub fn remembrance_due(now: NaiveDateTime) -> bool {
    Day::on(now.date()) == Some(Day::Remembrance)
        && now.num_seconds_from_midnight() >= SILENCE_AT + SILENCE_SECS
}

/// The remembrance bubble: (title, text).
pub fn remembrance_text(turkish: bool) -> (&'static str, &'static str) {
    if turkish {
        (
            "10 Kasım",
            "Cumhuriyetimizin kurucusu Ulu Önder Mustafa Kemal Atatürk'ü, ebediyete intikalinin yıl dönümünde saygı, sevgi ve özlemle anıyoruz.",
        )
    } else {
        (
            "10 November",
            "Remembering Mustafa Kemal Atatürk, founder of the Republic of Türkiye, who died on this day in 1938 at 09:05.",
        )
    }
}

/// The text of the first holiday bubble of a national holiday, if it has one.
pub fn holiday_text(d: NaiveDate, turkish: bool) -> Option<(&'static str, &'static str)> {
    let (tr, en) = match (d.month(), d.day()) {
        (4, 23) => (
            ("23 Nisan", "Ulusal Egemenlik ve Çocuk Bayramı kutlu olsun!"),
            ("23 April", "Happy National Sovereignty and Children's Day!"),
        ),
        (5, 19) => (
            (
                "19 Mayıs",
                "Atatürk'ü Anma, Gençlik ve Spor Bayramı kutlu olsun!",
            ),
            (
                "19 May",
                "Happy Commemoration of Atatürk, Youth and Sports Day!",
            ),
        ),
        (8, 30) => (
            ("30 Ağustos", "Zafer Bayramı kutlu olsun!"),
            ("30 August", "Happy Victory Day!"),
        ),
        (10, 29) => (
            ("29 Ekim", "Cumhuriyet Bayramı kutlu olsun!"),
            ("29 October", "Happy Republic Day!"),
        ),
        _ => return None,
    };
    Some(if turkish { tr } else { en })
}

/// A remembrance siren: a slow rise, a long steady tone and a fall (16-bit mono WAV).
pub fn siren_wav() -> Vec<u8> {
    const RATE: u32 = 22_050;
    let secs = 10.0f32;
    let n = (RATE as f32 * secs) as usize;
    let mut samples = Vec::with_capacity(n);
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / RATE as f32;
        // Up over 2.5 s, steady, down over the last 2.5 s.
        let rise = (t / 2.5).min(1.0);
        let fall = ((secs - t) / 2.5).min(1.0);
        let k = rise.min(fall);
        let freq = 220.0 + 360.0 * k;
        phase += freq / RATE as f32;
        // A soft, slightly hollow tone (odd harmonics), not a pure beep.
        let w = (phase * std::f32::consts::TAU).sin()
            + 0.25 * (3.0 * phase * std::f32::consts::TAU).sin();
        let env = (t / 0.4).min(1.0) * ((secs - t) / 0.6).min(1.0);
        samples.push((w * 0.55 * env * 0.8 * i16::MAX as f32) as i16);
    }
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// What is pinned on the collar (sprite pixels, drawn by the renderer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pin {
    /// A black mourning ribbon.
    Ribbon,
    /// A small Turkish flag.
    Flag,
}

impl Pin {
    pub fn on(d: NaiveDate) -> Option<Pin> {
        match Day::on(d)? {
            Day::Remembrance => Some(Pin::Ribbon),
            Day::National => Some(Pin::Flag),
        }
    }

    /// The picture: rows of characters, one per sprite pixel (`.` transparent), and the
    /// colour of each character.
    pub fn pixels(self) -> (&'static [&'static str], &'static [(char, [u8; 4])]) {
        match self {
            Pin::Ribbon => (
                &[
                    "...lll...", //
                    "..lkkkl..", //
                    ".lkglgkl.", //
                    ".lkl.lkl.", //
                    ".lkl.lkl.", //
                    "..lkkkl..", //
                    "..lkkkl..", //
                    ".lkklkkl.", //
                    ".lkl.lkl.", //
                    "lkl...lkl", //
                    "lkl...lkl", //
                    "ll.....ll", //
                ],
                &[
                    ('k', [0x12, 0x12, 0x14, 255]),
                    ('g', [0x55, 0x55, 0x5c, 255]),
                    ('l', [0xd9, 0xd6, 0xd0, 255]),
                ],
            ),
            Pin::Flag => (
                &[
                    "o...............", //
                    "orrrrrrrrrrrrrrr", //
                    "orrrwwwrrrrrrrrr", //
                    "orrwwrrrrrrrrrrr", //
                    "orwwrrrrrwrrrrrr", //
                    "orwwrrrrwwwrrrrr", //
                    "orwwrrrrrwrrrrrr", //
                    "orrwwrrrrrrrrrrr", //
                    "orrrwwwrrrrrrrrr", //
                    "orrrrrrrrrrrrrrr", //
                    "o...............", //
                    "o...............", //
                ],
                &[
                    ('r', [0xe3, 0x0a, 0x17, 255]),
                    ('w', [0xff, 0xff, 0xff, 255]),
                    ('o', [0x5a, 0x3b, 0x22, 255]),
                ],
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    #[test]
    fn days() {
        let d = |s| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        assert_eq!(Day::on(d("2026-11-10")), Some(Day::Remembrance));
        assert_eq!(Day::on(d("2026-10-29")), Some(Day::National));
        assert_eq!(Day::on(d("2026-10-28")), None);
        assert_eq!(Pin::on(d("2026-11-10")), Some(Pin::Ribbon));
        assert!(holiday_text(d("2026-04-23"), true).is_some());
    }

    #[test]
    fn minute_of_silence() {
        assert_eq!(silence_left(at("2026-11-10 09:04:59")), None);
        assert_eq!(silence_left(at("2026-11-10 09:05:00")), Some(60));
        assert_eq!(silence_left(at("2026-11-10 09:05:59")), Some(1));
        assert_eq!(silence_left(at("2026-11-10 09:06:00")), None);
        assert_eq!(silence_left(at("2026-11-11 09:05:30")), None);
        assert!(!remembrance_due(at("2026-11-10 09:05:30")));
        assert!(remembrance_due(at("2026-11-10 09:06:00")));
        assert!(remembrance_due(at("2026-11-10 17:00:00")));
        assert!(!remembrance_due(at("2026-11-09 17:00:00")));
    }

    #[test]
    fn siren_is_a_wav() {
        let w = siren_wav();
        assert_eq!(&w[..4], b"RIFF");
        assert!(crate::sound::scaled_wav(&w, 50).is_some());
    }

    #[test]
    fn pins_are_rectangular() {
        for p in [Pin::Ribbon, Pin::Flag] {
            let (rows, colors) = p.pixels();
            assert!(rows.iter().all(|r| r.len() == rows[0].len()));
            assert!(
                rows.iter()
                    .flat_map(|r| r.chars())
                    .all(|c| c == '.' || colors.iter().any(|(k, _)| *k == c))
            );
        }
    }
}
