//! How the dog looks: fur and collar colours, what it wears, holiday outfits and the bubble
//! theme. Platform-independent; [`crate::render::Renderer`] applies it.

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use tiny_skia::{ColorU8, Pixmap, PremultipliedColorU8};

/// Where an item sits in `assets/items.png` and how it is worn.
#[derive(Clone, Copy, Debug)]
pub struct ItemSprite {
    /// (x, y, w, h) in the items atlas.
    pub rect: (u32, u32, u32, u32),
    /// The point of the item that goes on the dog (item pixels).
    pub pin: (i32, i32),
    /// Where the pin goes on idle's first frame, relative to the feet point (sprite pixels).
    pub at: (i32, i32),
    /// Follows the collar instead of the head.
    pub neck: bool,
}

include!("../assets/items_atlas.rs");

/// Things the dog can wear or carry, in `assets/items.png` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Bone,
    Beanie,
    PartyHat,
    WitchHat,
    SantaHat,
    Bandana,
    Glasses,
    Pumpkin,
}

impl Item {
    pub fn sprite(self) -> ItemSprite {
        ITEMS[self as usize]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fur {
    #[default]
    Golden,
    Cream,
    Chocolate,
    Black,
    Grey,
    Red,
}

impl Fur {
    pub const ALL: [Fur; 6] = [
        Fur::Golden,
        Fur::Cream,
        Fur::Chocolate,
        Fur::Black,
        Fur::Grey,
        Fur::Red,
    ];

    /// Colour of the swatch in the panel.
    pub fn swatch(self) -> ColorU8 {
        match self {
            Fur::Golden => rgb(0xff, 0xb6, 0x21),
            Fur::Cream => rgb(0xf0, 0xd9, 0xa8),
            Fur::Chocolate => rgb(0x8a, 0x50, 0x25),
            Fur::Black => rgb(0x3a, 0x33, 0x30),
            Fur::Grey => rgb(0xa7, 0xa5, 0xa2),
            Fur::Red => rgb(0xd0, 0x5a, 0x2a),
        }
    }

    /// New (hue, saturation, lightness) for a fur pixel.
    fn map(self, (h, s, l): (f32, f32, f32)) -> (f32, f32, f32) {
        match self {
            Fur::Golden => (h, s, l),
            Fur::Cream => (38.0, s * 0.5, 0.45 + 0.5 * l),
            Fur::Chocolate => (h - 8.0, s * 0.7, l * 0.6),
            Fur::Black => (h, s * 0.2, 0.04 + l * 0.33),
            Fur::Grey => (h, 0.06, 0.12 + 0.75 * l),
            Fur::Red => (h - 18.0, s, l * 0.82),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Collar {
    #[default]
    Red,
    Blue,
    Green,
    Pink,
    Purple,
    Black,
}

impl Collar {
    pub const ALL: [Collar; 6] = [
        Collar::Red,
        Collar::Blue,
        Collar::Green,
        Collar::Pink,
        Collar::Purple,
        Collar::Black,
    ];

    pub fn swatch(self) -> ColorU8 {
        match self {
            Collar::Red => rgb(0xe0, 0x1e, 0x14),
            Collar::Blue => rgb(0x24, 0x6b, 0xe0),
            Collar::Green => rgb(0x2f, 0xa0, 0x4a),
            Collar::Pink => rgb(0xf0, 0x6a, 0xb4),
            Collar::Purple => rgb(0x8a, 0x3c, 0xd0),
            Collar::Black => rgb(0x2a, 0x28, 0x2c),
        }
    }

    fn map(self, (h, s, l): (f32, f32, f32)) -> (f32, f32, f32) {
        match self {
            Collar::Red => (h, s, l),
            Collar::Blue => (215.0, s * 0.85, l * 1.05),
            Collar::Green => (130.0, s * 0.7, l * 0.95),
            Collar::Pink => (328.0, s * 0.9, (l + 0.14).min(0.85)),
            Collar::Purple => (275.0, s * 0.75, l * 1.05),
            Collar::Black => (h, 0.08, l * 0.35),
        }
    }
}

/// What the dog wears (holiday hats come on top of it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wear {
    #[default]
    None,
    Bandana,
    Beanie,
    Glasses,
}

impl Wear {
    pub const ALL: [(Wear, &str); 4] = [
        (Wear::None, "None"),
        (Wear::Bandana, "Bandana"),
        (Wear::Beanie, "Beanie"),
        (Wear::Glasses, "Glasses"),
    ];
}

/// Colours of the speech bubble.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Classic,
    Dark,
    Sky,
    Mint,
}

impl Theme {
    pub const ALL: [(Theme, &str); 4] = [
        (Theme::Classic, "Classic"),
        (Theme::Dark, "Dark"),
        (Theme::Sky, "Sky"),
        (Theme::Mint, "Mint"),
    ];
}

/// Days the dog gets dressed up by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Season {
    /// The day it was adopted (first started), every year after the first.
    Birthday,
    /// 24-31 October.
    Halloween,
    /// 20 December - 1 January.
    Christmas,
}

impl Season {
    pub fn on(today: NaiveDate, birthday: Option<NaiveDate>) -> Option<Season> {
        if let Some(b) = birthday
            && today > b
            && (today.month(), today.day()) == (b.month(), b.day())
        {
            return Some(Season::Birthday);
        }
        match (today.month(), today.day()) {
            (10, 24..=31) => Some(Season::Halloween),
            (12, 20..=31) | (1, 1) => Some(Season::Christmas),
            _ => None,
        }
    }
}

/// What is drawn on the dog, back to front.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outfit {
    pub neck: Option<Item>,
    pub face: Option<Item>,
    pub hat: Option<Item>,
    /// The toy to fetch (and carry in the mouth).
    pub toy: Option<Item>,
}

impl Outfit {
    pub fn new(wear: Wear, season: Option<Season>) -> Self {
        let hat = match season {
            Some(Season::Birthday) => Some(Item::PartyHat),
            Some(Season::Halloween) => Some(Item::WitchHat),
            Some(Season::Christmas) => Some(Item::SantaHat),
            None => (wear == Wear::Beanie).then_some(Item::Beanie),
        };
        Outfit {
            neck: (wear == Wear::Bandana).then_some(Item::Bandana),
            face: (wear == Wear::Glasses).then_some(Item::Glasses),
            hat,
            toy: Some(if season == Some(Season::Halloween) {
                Item::Pumpkin
            } else {
                Item::Bone
            }),
        }
    }

    /// Worn items, drawn in this order.
    pub fn worn(&self) -> impl Iterator<Item = Item> {
        [self.neck, self.face, self.hat].into_iter().flatten()
    }
}

/// `"YYYY-MM-DD"`.
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// Recolours the dog's fur and collar in place; outline, eyes' shine, the cream patches, the
/// tongue and the gold tag stay as drawn.
pub fn recolor(pm: &mut Pixmap, fur: Fur, collar: Collar) {
    if fur == Fur::Golden && collar == Collar::Red {
        return;
    }
    for p in pm.pixels_mut() {
        if p.alpha() == 0 {
            continue;
        }
        let c = p.demultiply();
        let hsl = to_hsl(c);
        let (h, s, l) = hsl;
        let mapped = if l < 0.08 {
            continue;
        } else if !(12.0..=345.0).contains(&h) && s > 0.5 && l < 0.6 {
            collar.map(hsl)
        } else if (50.0..=62.0).contains(&h) && s > 0.8 && l < 0.75 {
            continue; // the gold tag
        } else if l >= 0.8 {
            continue; // cream
        } else if (10.0..=65.0).contains(&h) {
            fur.map(hsl)
        } else {
            continue;
        };
        let (r, g, b) = from_hsl(mapped);
        if let Some(n) = ColorU8::from_rgba(r, g, b, c.alpha()).premultiply().into() {
            *p = n;
        }
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> ColorU8 {
    ColorU8::from_rgba(r, g, b, 255)
}

fn to_hsl(c: ColorU8) -> (f32, f32, f32) {
    let (r, g, b) = (
        c.red() as f32 / 255.0,
        c.green() as f32 / 255.0,
        c.blue() as f32 / 255.0,
    );
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d <= f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, s.min(1.0), l)
}

fn from_hsl((h, s, l): (f32, f32, f32)) -> (u8, u8, u8) {
    let (s, l) = (s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
    let h = h.rem_euclid(360.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let u = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (u(r), u(g), u(b))
}

/// A colour for a notification's source: from `overrides` (lowercase name -> colour), or a
/// stable pick from a small palette, so each agent keeps its colour.
pub fn source_color(source: &str, overrides: &[(String, ColorU8)]) -> ColorU8 {
    let key = source.trim().to_lowercase();
    if let Some((_, c)) = overrides.iter().find(|(k, _)| *k == key) {
        return *c;
    }
    const PALETTE: [ColorU8; 8] = [
        rgb(0xc9, 0x6f, 0x12),
        rgb(0x2f, 0x6f, 0xc4),
        rgb(0x3a, 0x93, 0x58),
        rgb(0x9b, 0x4c, 0xb8),
        rgb(0xc4, 0x4a, 0x36),
        rgb(0x1f, 0x8c, 0x8a),
        rgb(0xa3, 0x80, 0x12),
        rgb(0xc2, 0x43, 0x82),
    ];
    // FNV-1a: stable across runs and platforms.
    let mut h: u32 = 0x811c_9dc5;
    for b in key.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    PALETTE[(h % PALETTE.len() as u32) as usize]
}

/// `"#rrggbb"` -> colour.
pub fn parse_hex(s: &str) -> Option<ColorU8> {
    let s = s.trim().strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Premultiplied pixel from a straight colour.
pub fn premul(c: ColorU8) -> PremultipliedColorU8 {
    c.premultiply()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        parse_date(s).unwrap()
    }

    #[test]
    fn seasons() {
        let b = Some(d("2025-03-14"));
        assert_eq!(Season::on(d("2026-03-14"), b), Some(Season::Birthday));
        assert_eq!(Season::on(d("2025-03-14"), b), None, "not on the first day");
        assert_eq!(Season::on(d("2026-10-31"), b), Some(Season::Halloween));
        assert_eq!(Season::on(d("2026-10-21"), b), None);
        assert_eq!(Season::on(d("2026-12-24"), None), Some(Season::Christmas));
        assert_eq!(Season::on(d("2027-01-01"), None), Some(Season::Christmas));
    }

    #[test]
    fn outfits() {
        let o = Outfit::new(Wear::Glasses, Some(Season::Halloween));
        assert_eq!(o.hat, Some(Item::WitchHat));
        assert_eq!(o.face, Some(Item::Glasses));
        assert_eq!(o.toy, Some(Item::Pumpkin));
        let o = Outfit::new(Wear::Beanie, None);
        assert_eq!(o.worn().collect::<Vec<_>>(), [Item::Beanie]);
        assert_eq!(o.toy, Some(Item::Bone));
        // A holiday hat replaces the beanie.
        assert_eq!(
            Outfit::new(Wear::Beanie, Some(Season::Birthday)).hat,
            Some(Item::PartyHat)
        );
    }

    #[test]
    fn hsl_round_trip_and_recolor() {
        for c in [
            rgb(0xff, 0xb6, 0x21),
            rgb(0x99, 0x4f, 0x09),
            rgb(0x24, 0x6b, 0xe0),
        ] {
            let (r, g, b) = from_hsl(to_hsl(c));
            assert!((r as i32 - c.red() as i32).abs() <= 1);
            assert!((g as i32 - c.green() as i32).abs() <= 1);
            assert!((b as i32 - c.blue() as i32).abs() <= 1);
        }
        let mut pm = Pixmap::new(4, 1).unwrap();
        let px = [
            rgb(0xff, 0xb6, 0x21), // fur
            rgb(0xf3, 0x15, 0x0e), // collar
            rgb(0x05, 0x02, 0x00), // outline
            rgb(0xfe, 0xfe, 0xe7), // cream
        ];
        for (d, c) in pm.pixels_mut().iter_mut().zip(px) {
            *d = premul(c);
        }
        recolor(&mut pm, Fur::Black, Collar::Blue);
        let out: Vec<ColorU8> = pm.pixels().iter().map(|p| p.demultiply()).collect();
        assert!(to_hsl(out[0]).2 < 0.3, "fur got dark");
        assert!((200.0..230.0).contains(&to_hsl(out[1]).0), "collar is blue");
        assert_eq!(out[2], px[2]);
        assert_eq!(out[3], px[3]);
    }

    #[test]
    fn source_colors() {
        let a = source_color("OpenCode", &[]);
        assert_eq!(a, source_color(" opencode ", &[]));
        let red = parse_hex("#ff0000").unwrap();
        assert_eq!(source_color("CI", &[("ci".into(), red)]), red);
        assert!(parse_hex("red").is_none());
    }
}
