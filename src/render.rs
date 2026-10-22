//! Draws the dog (pixel-art sprite atlas) and the speech bubble. Platform-independent.

use std::f32::consts::PI;

use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Hinting, Metrics, Shaping, SwashCache, Weight, Wrap,
};
use tiny_skia::{
    Color, ColorU8, FillRule, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Rect, Stroke,
    Transform,
};

use crate::dog::{Anim, DogFrame};
use crate::i18n::t;
use crate::look::{self, Collar, Fur, Item, Outfit, Theme};
use crate::notify::ProgressState;
use crate::special::Pin;

/// Output of `art/pack_atlas.py`: each row is an animation (in `Anim` order), each column a frame.
const ATLAS_PNG: &[u8] = include_bytes!("../assets/dog.png");
/// Output of `art/pack_items.py`: hats and toys, in `Item` order.
const ITEMS_PNG: &[u8] = include_bytes!("../assets/items.png");
/// Output of `art/collar.py`: where the collar is in `dog.png`, and the fur under it.
const COLLAR_PNG: &[u8] = include_bytes!("../assets/collar.png");
/// Text uses the system UI font; these families are tried in order.
const SYSTEM_FAMILIES: [&str; 7] = [
    "Segoe UI",
    "SF Pro Text",
    ".AppleSystemUIFont",
    "Helvetica Neue",
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
];
include!("../assets/dog_atlas.rs");

/// Logical space reserved for the bubble width.
pub const WIDTH: f32 = 280.0;
const BUBBLE_MAX_TEXT: f32 = 220.0;
/// The bubble on its own (card look): wider text, panel colours.
const CARD_MAX_TEXT: f32 = 250.0;
const CARD_SHADOW: f32 = 6.0;
const CARD_BG: ColorU8 = rgb(0xfb, 0xf7, 0xf1);
const BUBBLE_PAD: f32 = 10.0;
const BUBBLE_GAP: f32 = 2.0;
const BUBBLE_TAIL: f32 = 9.0;
/// Space left above the dog for the "z" letters (logical).
const HEADROOM: f32 = 26.0;
/// How far a worn or carried item may reach past the cell's sides (sprite pixels).
const ITEM_SPILL: u32 = 24;

const OUTLINE: ColorU8 = rgb(0x4a, 0x2e, 0x1b);
const INK: ColorU8 = rgb(0x2b, 0x2b, 0x33);
const MUTED: ColorU8 = rgb(0x76, 0x6e, 0x64);
/// Bubble body text: softer than the title.
const BODY: ColorU8 = rgb(0x4a, 0x46, 0x50);
/// Card look (bubble on its own): the settings panel's border.
const CARD_BORDER: ColorU8 = rgb(0xe8, 0xdf, 0xd2);
const ACCENT: ColorU8 = rgb(0xe0, 0x8e, 0x2b);
const ACCENT_INK: ColorU8 = rgb(0x9a, 0x55, 0x12);
const ACCENT_SOFT: ColorU8 = rgb(0xfc, 0xe9, 0xcf);
const TRACK: ColorU8 = rgb(0xe9, 0xe2, 0xd8);
const OK: ColorU8 = rgb(0x4f, 0xa8, 0x6a);
const FAIL: ColorU8 = rgb(0xc8, 0x4b, 0x3c);
/// Height of an answer button row / the progress bar row (logical, at text scale 1).
const CHOICE_H: f32 = 26.0;
const CHOICE_GAP: f32 = 6.0;
const PROGRESS_H: f32 = 14.0;
const LINK_H: f32 = 18.0;
const CLEAR_ALL: &str = "Clear all";
const SNOOZE: &str = "Later · 10 min";

const fn rgb(r: u8, g: u8, b: u8) -> ColorU8 {
    ColorU8::from_rgba(r, g, b, 255)
}

/// Bubble colours of a [`Theme`].
#[derive(Clone, Copy, Debug)]
struct Palette {
    /// Bubble over the dog.
    bubble: ColorU8,
    outline: ColorU8,
    /// Bubble on its own (card look).
    card: ColorU8,
    card_border: ColorU8,
    ink: ColorU8,
    body: ColorU8,
    muted: ColorU8,
    accent: ColorU8,
    accent_ink: ColorU8,
    accent_soft: ColorU8,
    track: ColorU8,
    /// Source names are lightened this much (0-1) to read on a dark bubble.
    lighten: f32,
}

fn palette(t: Theme) -> Palette {
    let classic = Palette {
        bubble: rgb(0xff, 0xff, 0xff),
        outline: OUTLINE,
        card: CARD_BG,
        card_border: CARD_BORDER,
        ink: INK,
        body: BODY,
        muted: MUTED,
        accent: ACCENT,
        accent_ink: ACCENT_INK,
        accent_soft: ACCENT_SOFT,
        track: TRACK,
        lighten: 0.0,
    };
    match t {
        Theme::Classic => classic,
        Theme::Dark => Palette {
            bubble: rgb(0x2b, 0x2a, 0x33),
            outline: rgb(0x0d, 0x0c, 0x10),
            card: rgb(0x2b, 0x2a, 0x33),
            card_border: rgb(0x45, 0x43, 0x50),
            ink: rgb(0xf3, 0xef, 0xe8),
            body: rgb(0xcf, 0xca, 0xd6),
            muted: rgb(0x9a, 0x95, 0xa4),
            accent_ink: rgb(0xf2, 0xb7, 0x65),
            accent_soft: rgb(0x4a, 0x3c, 0x2c),
            track: rgb(0x45, 0x43, 0x50),
            lighten: 0.4,
            ..classic
        },
        Theme::Sky => Palette {
            bubble: rgb(0xee, 0xf6, 0xff),
            outline: rgb(0x2d, 0x4f, 0x7c),
            card: rgb(0xee, 0xf6, 0xff),
            card_border: rgb(0xc9, 0xdc, 0xf2),
            accent: rgb(0x2f, 0x7a, 0xd6),
            accent_ink: rgb(0x1d, 0x55, 0x9c),
            accent_soft: rgb(0xd6, 0xe7, 0xfb),
            track: rgb(0xd5, 0xe1, 0xee),
            ..classic
        },
        Theme::Mint => Palette {
            bubble: rgb(0xee, 0xfb, 0xf3),
            outline: rgb(0x2b, 0x6b, 0x4b),
            card: rgb(0xee, 0xfb, 0xf3),
            card_border: rgb(0xc8, 0xe9, 0xd5),
            accent: rgb(0x2f, 0x9a, 0x64),
            accent_ink: rgb(0x1f, 0x6e, 0x46),
            accent_soft: rgb(0xd3, 0xf1, 0xe0),
            track: rgb(0xd4, 0xe8, 0xdc),
            ..classic
        },
    }
}

fn mix(a: ColorU8, b: ColorU8, t: f32) -> ColorU8 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    rgb(
        m(a.red(), b.red()),
        m(a.green(), b.green()),
        m(a.blue(), b.blue()),
    )
}

fn color(c: ColorU8) -> Color {
    Color::from_rgba8(c.red(), c.green(), c.blue(), c.alpha())
}

/// `ui`: DPI scale for text and the bubble. `px`: how many physical pixels one sprite pixel covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub ui: f32,
    pub px: u32,
    /// Extra multiplier for bubble text (text size from settings, 1.0 = normal).
    pub text: f32,
}

impl Scale {
    /// `size`: dog size from settings, percent (100 = 1:1 at 100% screen scale).
    /// Rounded to a whole number so pixels don't get blurry or uneven.
    pub fn new(ui: f32, size: u32) -> Self {
        Scale {
            ui,
            px: (ui * size as f32 / 100.0).round().max(1.0) as u32,
            text: 1.0,
        }
    }

    pub fn with_text(mut self, text: f32) -> Self {
        self.text = text;
        self
    }

    /// Total scale used for the bubble and text.
    fn bubble(&self) -> f32 {
        self.ui * self.text
    }
}

/// Text to show in the bubble.
#[derive(Default)]
pub struct Bubble<'a> {
    pub title: &'a str,
    pub body: &'a str,
    /// Number of other notifications waiting in the queue.
    pub pending: usize,
    /// Small label above the title (who sent it).
    pub source: Option<&'a str>,
    /// Answer buttons under the text.
    pub choices: &'a [String],
    /// Progress bar under the text: (value 0-1 or `None` for indeterminate, state).
    pub progress: Option<(Option<f32>, ProgressState)>,
    /// Shows "Click to open" under the text.
    pub link: bool,
    /// Shows "Later" (snooze) under the text.
    pub snooze: bool,
    /// Shows "Reply…": the question may be answered in words.
    pub reply: bool,
    /// Seconds since the bubble appeared (animates the indeterminate bar).
    pub time: f32,
}

/// Regions of the drawn frame needed for hit testing (physical pixels, relative to the window).
#[derive(Clone, Debug, Default)]
pub struct Hits {
    pub bubble: Option<Rect>,
    /// Answer buttons, in order.
    pub choices: Vec<Rect>,
    /// "Clear all" next to "+N more".
    pub clear: Option<Rect>,
    /// "Later": shown again in a while.
    pub snooze: Option<Rect>,
    /// "Reply…": answer in words.
    pub reply: Option<Rect>,
}

pub struct Renderer {
    /// The dog as drawn (`assets/dog.png`).
    original: Pixmap,
    /// Where the collar is in `original` (`assets/collar.png`).
    collar: Pixmap,
    /// The dog in the chosen colours.
    atlas: Pixmap,
    items: Pixmap,
    outfit: Outfit,
    colors: (Fur, Collar),
    pal: Palette,
    /// Lowercase source name -> label colour (settings).
    source_colors: Vec<(String, ColorU8)>,
    /// Height of each animation's highest point above the ground, with what it wears
    /// (sprite pixels).
    heights: [u32; Anim::ALL.len()],
    text: Option<Text>,
    /// Pinned on the collar for the day (special days).
    pin: Option<Pin>,
    /// Goes up whenever the dog's colours change (the tray icon is drawn again).
    look_gen: u32,
}

/// Text layout: system fonts, shaping for complex scripts (Arabic, Hindi...),
/// right-to-left text and automatic font fallback for missing characters (CJK, emoji...).
struct Text {
    fonts: FontSystem,
    cache: SwashCache,
    buffer: Buffer,
    /// Content and scale of the last bubble text layout; not laid out again if unchanged.
    laid_out: Option<(String, String, usize, u32, u32)>,
    /// System UI font family used for all text.
    family: String,
    /// x of the leftmost glyph in the layout. Right-to-left lines are aligned to the right of the box;
    /// when drawing they are shifted left by this much.
    left: f32,
}

impl Renderer {
    /// `fonts`: whether to load system fonts (can be disabled in tests and previews).
    /// `extra_fonts`: additional font files (e.g. if the system has no fonts).
    pub fn new(system_fonts: bool, extra_fonts: Vec<Vec<u8>>) -> Self {
        let atlas = Pixmap::decode_png(ATLAS_PNG).expect("assets/dog.png");
        let items = Pixmap::decode_png(ITEMS_PNG).expect("assets/items.png");
        let heights = Anim::ALL.map(|a| sprite_height(&atlas, a, &Outfit::default()));
        let sources = extra_fonts
            .into_iter()
            .map(|d| cosmic_text::fontdb::Source::Binary(std::sync::Arc::new(d)));
        let mut fonts = if system_fonts {
            FontSystem::new_with_fonts(sources)
        } else {
            let mut db = cosmic_text::fontdb::Database::new();
            for s in sources {
                db.load_font_source(s);
            }
            FontSystem::new_with_locale_and_db("en-US".into(), db)
        };
        let text = (!fonts.db().is_empty()).then(|| {
            let mut buffer = Buffer::new(&mut fonts, Metrics::new(13.0, 17.0));
            // Snap glyphs to whole pixels so thresholded edges stay clean.
            buffer.set_hinting(Hinting::Enabled);
            Text {
                fonts,
                cache: SwashCache::new(),
                buffer,
                laid_out: None,
                family: String::new(),
                left: 0.0,
            }
        });
        let mut text = text;
        if let Some(t) = text.as_mut() {
            let db = t.fonts.db();
            t.family = SYSTEM_FAMILIES
                .into_iter()
                .find(|f| {
                    db.faces()
                        .any(|face| face.families.iter().any(|(n, _)| n == f))
                })
                .or_else(|| db.faces().next().map(|f| f.families[0].0.as_str()))
                .unwrap_or("sans-serif")
                .to_string();
        }
        Renderer {
            original: atlas.clone(),
            collar: Pixmap::decode_png(COLLAR_PNG).expect("assets/collar.png"),
            atlas,
            items,
            outfit: Outfit::default(),
            colors: (Fur::Golden, Collar::Red),
            pal: palette(Theme::Classic),
            source_colors: Vec::new(),
            heights,
            text,
            pin: None,
            look_gen: 0,
        }
    }

    /// Changes whenever the dog's colours do: shells redraw the tray icon when it changes.
    pub fn look_gen(&self) -> u32 {
        self.look_gen
    }

    /// What is pinned on the collar today (a ribbon, a flag), if anything.
    pub fn set_pin(&mut self, pin: Option<Pin>) {
        self.pin = pin;
    }

    /// Colours, what the dog wears and the bubble theme. Cheap when nothing changed.
    pub fn set_look(
        &mut self,
        fur: Fur,
        collar: Collar,
        outfit: Outfit,
        theme: Theme,
        source_colors: Vec<(String, ColorU8)>,
    ) {
        if self.colors != (fur, collar) {
            self.colors = (fur, collar);
            self.atlas = self.original.clone();
            look::recolor(&mut self.atlas, &self.collar, fur, collar);
            self.look_gen = self.look_gen.wrapping_add(1);
        }
        if self.outfit != outfit {
            self.outfit = outfit;
            self.heights = Anim::ALL.map(|a| sprite_height(&self.atlas, a, &outfit));
        }
        self.pal = palette(theme);
        self.source_colors = source_colors;
        // Colours are part of the cached layout.
        if let Some(t) = self.text.as_mut() {
            t.laid_out = None;
        }
    }

    pub fn outfit(&self) -> Outfit {
        self.outfit
    }

    /// Size of the toy's window: a square that fits it turned any quarter turn (physical).
    pub fn toy_size(&self, item: Item, px: u32) -> u32 {
        let (_, _, w, h) = item.sprite().poses[0].rect;
        w.max(h) * px
    }

    /// Draws the toy turned `quarter` quarter turns, centred, resting on the bottom edge.
    pub fn draw_toy(&self, pm: &mut Pixmap, item: Item, quarter: u8, px: u32) {
        pm.fill(Color::TRANSPARENT);
        let (rx, ry, w, h) = item.sprite().poses[0].rect;
        let turned = quarter % 2 == 1;
        let (tw, th) = if turned { (h, w) } else { (w, h) };
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let x0 = (pw - (tw * px) as i32) / 2;
        let y0 = ph - (th * px) as i32;
        let src = self.items.pixels();
        let iw = self.items.width();
        let dst = pm.pixels_mut();
        for ty in 0..th {
            for tx in 0..tw {
                // Pixel of the turned item -> pixel of the item.
                let (sx, sy) = match quarter % 4 {
                    0 => (tx, ty),
                    1 => (ty, h - 1 - tx),
                    2 => (w - 1 - tx, h - 1 - ty),
                    _ => (w - 1 - ty, tx),
                };
                let p = src[((ry + sy) * iw + rx + sx) as usize];
                if p.alpha() == 0 {
                    continue;
                }
                let (bx, by) = (x0 + (tx * px) as i32, y0 + (ty * px) as i32);
                for y in by.max(0)..(by + px as i32).min(ph) {
                    for x in bx.max(0)..(bx + px as i32).min(pw) {
                        let d = &mut dst[(y * pw + x) as usize];
                        *d = over(p, *d);
                    }
                }
            }
        }
    }

    /// The dog and what it wears (and carries), feet point at (`ax`, `ground`).
    fn blit_dog(&self, pm: &mut Pixmap, f: &DogFrame, ax: i32, ground: i32, px: i32) {
        blit_sprite(pm, &self.atlas, f, ax, ground, px);
        let toy = self.outfit.toy.filter(|_| f.carry);
        for item in self.outfit.worn().chain(toy) {
            let (s, (tx, ty)) = item_target(item.sprite(), f.anim, f.frame);
            let (rx, ry, w, h) = s.rect;
            blit_region(
                pm,
                &self.items,
                (rx, ry, w, h),
                (tx - s.pin.0, ty - s.pin.1),
                f.facing < 0.0,
                (ax, ground, px),
            );
        }
        if let Some(pin) = self.pin {
            draw_pin(pm, pin, f, (ax, ground, px));
        }
    }

    /// Window size needed for the given content (physical pixels).
    pub fn size(&mut self, anim: Anim, bubble: Option<&Bubble>, s: Scale) -> (u32, u32) {
        let px = s.px as f32;
        let dog = self.heights[anim.row() as usize] as f32 * px;
        let extra = match bubble {
            Some(b) => {
                let (_, rows) = self.choice_layout(b, s.bubble(), BUBBLE_MAX_TEXT);
                let (_, h) = self.layout_text(b, s.bubble(), BUBBLE_MAX_TEXT);
                h + extras_height(b, rows.len()) * s.bubble()
                    + (BUBBLE_PAD * 2.0 + BUBBLE_TAIL + BUBBLE_GAP + 4.0) * s.bubble()
            }
            None => HEADROOM * s.ui,
        };
        let w = (WIDTH * s.bubble()).max(((CELL.0 + 2 * ITEM_SPILL) * s.px) as f32 + 8.0);
        let h = Self::below(s) + dog + extra;
        (w.ceil() as u32, h.ceil() as u32)
    }

    fn below(s: Scale) -> f32 {
        ((CELL.1 - ANCHOR.1) * s.px) as f32
    }

    /// Distance of the ground line from the top of the window (physical pixels).
    pub fn ground_offset(height: u32, s: Scale) -> f32 {
        height as f32 - Self::below(s)
    }

    /// Draws the frame. `anchor_x`: x of the dog's feet point inside the window (physical).
    pub fn draw(
        &mut self,
        pixmap: &mut Pixmap,
        anchor_x: f32,
        frame: &DogFrame,
        bubble: Option<&Bubble>,
        s: Scale,
    ) -> Hits {
        pixmap.fill(Color::TRANSPARENT);
        let ground = Self::ground_offset(pixmap.height(), s);
        let ax = anchor_x.round() as i32;
        self.blit_dog(pixmap, frame, ax, ground.round() as i32, s.px as i32);

        if frame.anim == Anim::Sleep {
            self.draw_zzz(pixmap, anchor_x, ground, frame, s);
        }

        let mut hits = Hits::default();
        if let Some(b) = bubble {
            let top = ground - (self.heights[frame.anim.row() as usize] * s.px) as f32;
            let at = BubbleAt::Above {
                x: anchor_x,
                y: top,
            };
            if let Some(h) = self.draw_bubble(pixmap, at, b, s.bubble(), false) {
                hits = h;
            }
        }
        hits
    }

    /// Size of a bubble drawn on its own (no dog), tail on top (physical pixels).
    pub fn bubble_size(&mut self, b: &Bubble, s: Scale) -> (u32, u32) {
        let sc = s.bubble();
        let (w, h) = self.bubble_box(b, sc, CARD_MAX_TEXT);
        let shadow = CARD_SHADOW * sc;
        (
            (w + 2.0 * shadow).ceil() as u32,
            (h + BUBBLE_TAIL * sc + 2.0 * shadow).ceil() as u32,
        )
    }

    /// Empty room (for the shadow) between the edge of a bubble-on-its-own window and the tip
    /// of its tail (physical pixels).
    pub fn alone_inset(&self, s: Scale) -> f32 {
        (CARD_SHADOW * s.bubble()).round()
    }

    /// Draws a bubble on its own with its tail pointing at `tail_x` (physical, window-relative):
    /// up from the top edge (a menu bar above, macOS) or, with `tail_down`, down from the
    /// bottom edge (a taskbar below, Windows). The window is sized by [`Renderer::bubble_size`].
    pub fn draw_bubble_alone(
        &mut self,
        pixmap: &mut Pixmap,
        tail_x: f32,
        tail_down: bool,
        b: &Bubble,
        s: Scale,
    ) -> Hits {
        pixmap.fill(Color::TRANSPARENT);
        let sc = s.bubble();
        let shadow = CARD_SHADOW * sc;
        let at = if tail_down {
            BubbleAt::Above {
                x: tail_x,
                y: pixmap.height() as f32 - shadow + BUBBLE_GAP * sc,
            }
        } else {
            BubbleAt::Below {
                x: tail_x,
                y: shadow + BUBBLE_TAIL * sc,
                left: shadow,
            }
        };
        self.draw_bubble(pixmap, at, b, sc, true)
            .unwrap_or_default()
    }

    /// Tray icon: the sitting or the sleeping dog, straight RGBA; `unread` adds a red dash.
    pub fn icon_rgba(&self, size: u32, sleeping: bool, unread: bool) -> Vec<u8> {
        self.icon_pixmap(size, sleeping, unread)
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect()
    }

    /// Tray icon as a premultiplied pixmap: the sitting or the sleeping dog, with a thin red
    /// dash in the top-right corner when something was missed.
    pub fn icon_pixmap(&self, size: u32, sleeping: bool, unread: bool) -> Pixmap {
        let anim = if sleeping { Anim::Sleep } else { Anim::Sit };
        let (x0, y0, x1, y1) = opaque_box(&self.atlas, anim, 0);
        let (w, h) = ((x1 - x0) as f32, (y1 - y0) as f32);
        let k = size as f32 / w.max(h);
        let mut pm = Pixmap::new(size, size).expect("icon size");
        // Only this frame: the whole atlas would show the row above in the room over a
        // wide, low pose (the sleeping dog).
        let Some(frame) = self.atlas.clone_rect(
            tiny_skia::IntRect::from_xywh(x0 as i32, y0 as i32, x1 - x0, y1 - y0).expect("frame"),
        ) else {
            return pm;
        };
        let t = Transform::from_translate((size as f32 - w * k) / 2.0, size as f32 - h * k)
            .pre_scale(k, k);
        let paint = tiny_skia::PixmapPaint {
            quality: tiny_skia::FilterQuality::Bicubic,
            ..Default::default()
        };
        pm.draw_pixmap(0, 0, frame.as_ref(), &paint, t, None);
        if unread {
            // A thin red dash in the top-right corner, with a light edge for dark menu bars.
            let s = size as f32;
            let (w, h) = (s * 0.34, (s * 0.09).max(2.0));
            let (x, y) = (s - w - s * 0.03, s * 0.05);
            let edge = (s * 0.035).max(1.0);
            for (r, c) in [
                (
                    Rect::from_xywh(x - edge, y - edge, w + 2.0 * edge, h + 2.0 * edge),
                    Color::WHITE,
                ),
                (Rect::from_xywh(x, y, w, h), color(FAIL)),
            ] {
                let Some(r) = r else { continue };
                let rad = r.height() / 2.0;
                let mut pb = PathBuilder::new();
                pb.move_to(r.left() + rad, r.top());
                pb.line_to(r.right() - rad, r.top());
                pb.quad_to(r.right(), r.top(), r.right(), r.top() + rad);
                pb.quad_to(r.right(), r.bottom(), r.right() - rad, r.bottom());
                pb.line_to(r.left() + rad, r.bottom());
                pb.quad_to(r.left(), r.bottom(), r.left(), r.top() + rad);
                pb.quad_to(r.left(), r.top(), r.left() + rad, r.top());
                pb.close();
                if let Some(p) = pb.finish() {
                    pm.fill_path(
                        &p,
                        &self::paint(c),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
        }
        pm
    }

    fn draw_zzz(&mut self, pm: &mut Pixmap, ax: f32, ground: f32, frame: &DogFrame, s: Scale) {
        let muted = self.pal.muted;
        let Some(text) = &mut self.text else { return };
        let px = s.px as f32;
        let head_x = ax + 50.0 * px * frame.facing;
        let head_y = ground - self.heights[Anim::Sleep.row() as usize] as f32 * px;
        for i in 0..3 {
            let p = (frame.time * 0.35 + i as f32 / 3.0).fract();
            let x = head_x + p * 16.0 * s.ui * frame.facing;
            let y = head_y - (14.0 + p * 18.0) * s.ui;
            let alpha = ((p * PI).sin() * 0.9 * 255.0) as u8;
            let size = (9.0 + p * 6.0) * s.ui;
            let c = cosmic_text::Color::rgba(muted.red(), muted.green(), muted.blue(), alpha);
            let attrs = Attrs::new()
                .family(Family::Name(&text.family))
                .weight(Weight::BOLD);
            text.buffer.set_metrics(Metrics::new(size, size * 1.2));
            text.buffer.set_size(None, None);
            text.buffer.set_text("z", &attrs, Shaping::Basic, None);
            text.laid_out = None;
            text.left = 0.0;
            text.draw(pm, x, y, c);
        }
    }

    /// Lays out the bubble text and returns (width, height) (physical pixels).
    fn layout_text(&mut self, b: &Bubble, scale: f32, max: f32) -> (f32, f32) {
        let Some(text) = &mut self.text else {
            return (40.0 * scale, 14.0 * scale);
        };
        let key = (
            format!("{}\u{1}{}", b.source.unwrap_or(""), b.title),
            b.body.to_string(),
            b.pending,
            scale.to_bits(),
            max.to_bits(),
        );
        if text.laid_out.as_ref() != Some(&key) {
            let pal = self.pal;
            let src_color = b.source.map_or(pal.accent_ink, |s| {
                let c = look::source_color(s, &self.source_colors);
                mix(c, rgb(0xff, 0xff, 0xff), pal.lighten)
            });
            let base = Attrs::new()
                .family(Family::Name(&text.family))
                .color(cc(pal.ink));
            let source = base
                .clone()
                .color(cc(src_color))
                .weight(Weight::SEMIBOLD)
                .metrics(Metrics::new(11.0 * scale, 16.0 * scale));
            let title = base
                .clone()
                .weight(Weight::SEMIBOLD)
                .metrics(Metrics::new(13.5 * scale, 18.0 * scale));
            let body = base
                .clone()
                .color(cc(pal.body))
                .metrics(Metrics::new(13.0 * scale, 18.0 * scale));
            let more = base
                .clone()
                .color(cc(pal.muted))
                .metrics(Metrics::new(11.5 * scale, 17.0 * scale));
            let body_text: String = b.body.chars().take(400).collect();
            let more_text = t("+{n} more").replace("{n}", &b.pending.to_string());
            let source_text = b.source.unwrap_or_default().to_string();
            let mut spans: Vec<(&str, Attrs)> = Vec::new();
            for (t, a) in [
                (source_text.as_str(), source),
                (b.title, title),
                (body_text.as_str(), body),
                (more_text.as_str(), more),
            ] {
                let skip = t.is_empty() || (std::ptr::eq(t, more_text.as_str()) && b.pending == 0);
                if skip {
                    continue;
                }
                if !spans.is_empty() {
                    spans.push(("\n", a.clone()));
                }
                spans.push((t, a));
            }
            text.buffer
                .set_metrics(Metrics::new(14.0 * scale, 19.0 * scale));
            text.buffer.set_wrap(Wrap::WordOrGlyph);
            text.buffer.set_size(Some(max * scale), None);
            text.buffer
                .set_rich_text(spans, &base, Shaping::Advanced, None);
            text.buffer.shape_until_scroll(&mut text.fonts, false);
            text.laid_out = Some(key);
        }
        let (mut x0, mut x1, mut h) = (f32::MAX, 0.0f32, 0.0f32);
        for run in text.buffer.layout_runs() {
            for g in run.glyphs {
                x0 = x0.min(g.x);
                x1 = x1.max(g.x + g.w);
            }
            h = h.max(run.line_top + run.line_height);
        }
        text.left = if x0 == f32::MAX { 0.0 } else { x0.floor() };
        ((x1 - text.left).ceil().max(30.0 * scale), h.ceil())
    }

    /// Outer size of the bubble body (without tail), physical pixels.
    fn bubble_box(&mut self, b: &Bubble, scale: f32, max: f32) -> (f32, f32) {
        let (_, rows) = self.choice_layout(b, scale, max);
        let (mut tw, th) = self.layout_text(b, scale, max);
        if let Some(widest) = rows.iter().map(|r| r.1).reduce(f32::max) {
            tw = tw.max(widest);
        }
        if b.progress.is_some() || b.link {
            tw = tw.max(160.0 * scale);
        }
        if b.pending > 0 || b.snooze || b.reply {
            let mut need: f32 = if b.snooze { 190.0 } else { 90.0 };
            if b.reply {
                need += 80.0;
            }
            tw = tw.max(need * scale);
        }
        let pad = BUBBLE_PAD * scale;
        (
            tw + pad * 2.0,
            th + extras_height(b, rows.len()) * scale + pad * 2.0,
        )
    }

    fn draw_bubble(
        &mut self,
        pm: &mut Pixmap,
        at: BubbleAt,
        b: &Bubble,
        scale: f32,
        card: bool,
    ) -> Option<Hits> {
        let pal = self.pal;
        let max = if card { CARD_MAX_TEXT } else { BUBBLE_MAX_TEXT };
        let (widths, rows) = self.choice_layout(b, scale, max);
        let label = |c: ColorU8| TextStyle {
            size: 13.0 * scale,
            bold: true,
            color: c,
        };
        let (w, h) = self.bubble_box(b, scale, max);
        let (_, th) = self.layout_text(b, scale, max);
        let pad = BUBBLE_PAD * scale;
        let tw = w - pad * 2.0;
        let margin = 2.0 * scale;
        let (ax, left, top, tail_up) = match at {
            BubbleAt::Above { x, y } => {
                let bottom = y - (BUBBLE_GAP + BUBBLE_TAIL) * scale;
                let left =
                    (x - w / 2.0).clamp(margin, (pm.width() as f32 - w - margin).max(margin));
                (x, left, bottom - h, false)
            }
            BubbleAt::Below { x, y, left } => (x, left, y, true),
        };
        let rect = Rect::from_xywh(left, top, w, h)?;

        let tail_x = ax.clamp(left + 14.0 * scale, left + w - 14.0 * scale);
        let path = bubble_path(
            rect,
            if card { 12.0 } else { 9.0 } * scale,
            tail_x,
            BUBBLE_TAIL * scale,
            7.0 * scale,
            tail_up,
        )?;

        if card {
            // Soft, layered shadow like a native popover.
            for (dy, a) in [(1.0, 0.05), (2.0, 0.05), (4.0, 0.04)] {
                let mut shade = Color::BLACK;
                shade.set_alpha(a);
                let t = Transform::from_translate(0.0, dy * scale);
                pm.fill_path(&path, &paint(shade), FillRule::Winding, t, None);
            }
        } else {
            let shadow_t = Transform::from_translate(0.0, 2.0 * scale);
            let mut shade = Color::BLACK;
            shade.set_alpha(0.18);
            pm.fill_path(&path, &paint(shade), FillRule::Winding, shadow_t, None);
        }
        pm.fill_path(
            &path,
            &paint(color(if card { pal.card } else { pal.bubble })),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let stroke = Stroke {
            width: scale.round().max(1.0),
            ..Stroke::default()
        };
        pm.stroke_path(
            &path,
            &paint(color(if card { pal.card_border } else { pal.outline })),
            &stroke,
            Transform::identity(),
            None,
        );

        if let Some(text) = &mut self.text {
            let x = left + pad - text.left;
            text.draw(pm, x, top + pad, cc(pal.ink));
        }

        let x = left + pad;
        let mut y = top + pad + th;
        if let Some((value, state)) = b.progress {
            y += 6.0 * scale;
            let bar_h = 6.0 * scale;
            let track = Rect::from_xywh(x, y + 2.0 * scale, tw, bar_h)?;
            fill_rrect(pm, track, bar_h / 2.0, pal.track);
            let fill_c = match state {
                ProgressState::Running => pal.accent,
                ProgressState::Done => OK,
                ProgressState::Failed => FAIL,
            };
            let (from, to) = match (state, value) {
                (ProgressState::Running, None) => {
                    // Indeterminate: a third of the bar sweeping across.
                    let p = (b.time * 0.7).fract();
                    let s = -0.33 + p * 1.33;
                    (s.max(0.0), (s + 0.33).min(1.0))
                }
                (ProgressState::Running, Some(v)) => (0.0, v.clamp(0.0, 1.0)),
                _ => (0.0, 1.0),
            };
            if to > from
                && let Some(r) = Rect::from_xywh(
                    x + tw * from,
                    track.top(),
                    (tw * (to - from)).max(bar_h),
                    bar_h,
                )
            {
                fill_rrect(pm, r, bar_h / 2.0, fill_c);
            }
            y += (PROGRESS_H - 6.0 + 6.0) * scale;
        }
        if b.link {
            y += 4.0 * scale;
            let st = TextStyle {
                size: 12.0 * scale,
                bold: false,
                color: pal.accent_ink,
            };
            self.draw_text(pm, x, y, t("Click to open ↗"), st);
            y += (LINK_H - 4.0) * scale;
        }
        let (mut clear, mut snooze, mut reply) = (None, None, None);
        if b.pending > 0 || b.snooze || b.reply {
            y += 4.0 * scale;
            let st = TextStyle {
                size: 12.0 * scale,
                bold: true,
                color: pal.accent_ink,
            };
            // A little slack around the text so it is easy to hit.
            let slack = 4.0 * scale;
            let mut lx = x;
            if b.reply {
                let label = t("Reply…");
                let (lw, lh) = self.measure(label, st);
                self.draw_text(pm, lx, y, label, st);
                reply = Rect::from_xywh(lx - slack, y - slack, lw + 2.0 * slack, lh + 2.0 * slack);
                lx += lw + 16.0 * scale;
            }
            if b.pending > 0 {
                let clear_all = t(CLEAR_ALL);
                let (lw, lh) = self.measure(clear_all, st);
                self.draw_text(pm, lx, y, clear_all, st);
                clear = Rect::from_xywh(lx - slack, y - slack, lw + 2.0 * slack, lh + 2.0 * slack);
                lx += lw + 16.0 * scale;
            }
            if b.snooze {
                let st = TextStyle {
                    color: pal.muted,
                    ..st
                };
                let later = t(SNOOZE);
                let (lw, lh) = self.measure(later, st);
                self.draw_text(pm, lx, y, later, st);
                snooze = Rect::from_xywh(lx - slack, y - slack, lw + 2.0 * slack, lh + 2.0 * slack);
            }
            y += (LINK_H - 4.0) * scale;
        }
        let mut buttons = Vec::new();
        if !rows.is_empty() {
            y += CHOICE_GAP * scale;
            let mut i = 0;
            for (count, _) in &rows {
                let mut bx = x;
                for _ in 0..*count {
                    let r = Rect::from_xywh(bx, y, widths[i], CHOICE_H * scale)?;
                    let first = i == 0;
                    fill_rrect(
                        pm,
                        r,
                        8.0 * scale,
                        if first { pal.accent } else { pal.accent_soft },
                    );
                    let st = label(if first {
                        rgb(0xff, 0xff, 0xff)
                    } else {
                        pal.accent_ink
                    });
                    let (lw, lh) = self.measure(&b.choices[i], st);
                    self.draw_text(
                        pm,
                        (r.left() + (r.width() - lw) / 2.0).round(),
                        (r.top() + (r.height() - lh) / 2.0).round(),
                        &b.choices[i],
                        st,
                    );
                    buttons.push(r);
                    bx += widths[i] + CHOICE_GAP * scale;
                    i += 1;
                }
                y += (CHOICE_H + CHOICE_GAP) * scale;
            }
        }
        // The single-line text calls above replaced the bubble layout.
        if let Some(t) = self.text.as_mut() {
            t.laid_out = None;
        }

        let full = if tail_up {
            Rect::from_ltrb(left, top - BUBBLE_TAIL * scale, left + w, top + h)?
        } else {
            Rect::from_ltrb(left, top, left + w, top + h + BUBBLE_TAIL * scale)?
        };
        Some(Hits {
            bubble: Some(full),
            choices: buttons,
            clear,
            snooze,
            reply,
        })
    }

    /// Answer button widths and how they wrap into rows (physical pixels).
    fn choice_layout(&mut self, b: &Bubble, scale: f32, max: f32) -> (Vec<f32>, Vec<(usize, f32)>) {
        let st = TextStyle {
            size: 13.0 * scale,
            bold: true,
            color: INK,
        };
        let widths: Vec<f32> = b
            .choices
            .iter()
            .map(|c| self.measure(c, st).0 + 22.0 * scale)
            .collect();
        if let Some(t) = self.text.as_mut() {
            t.laid_out = None;
        }
        let rows = choice_rows(&widths, max * scale, CHOICE_GAP * scale);
        (widths, rows)
    }
}

/// Where a bubble goes: above the dog (tail down to `x`), or on its own hanging below
/// the tray icon (tail up to `x`, body's top-left at `left`, `y`).
#[derive(Clone, Copy)]
enum BubbleAt {
    Above { x: f32, y: f32 },
    Below { x: f32, y: f32, left: f32 },
}

/// Splits answer buttons into rows that fit `max` wide: (buttons in the row, row width).
fn choice_rows(widths: &[f32], max: f32, gap: f32) -> Vec<(usize, f32)> {
    let mut rows: Vec<(usize, f32)> = Vec::new();
    for &w in widths {
        match rows.last_mut() {
            Some((n, rw)) if *rw + gap + w <= max => {
                *n += 1;
                *rw += gap + w;
            }
            _ => rows.push((1, w)),
        }
    }
    rows
}

/// Logical height of what is drawn under the bubble text, at text scale 1.
fn extras_height(b: &Bubble, choice_rows: usize) -> f32 {
    let mut h = 0.0;
    if b.progress.is_some() {
        h += PROGRESS_H + 6.0;
    }
    if b.link {
        h += LINK_H;
    }
    if b.pending > 0 || b.snooze || b.reply {
        h += LINK_H;
    }
    if choice_rows > 0 {
        h += CHOICE_GAP + choice_rows as f32 * (CHOICE_H + CHOICE_GAP);
    }
    h
}

fn fill_rrect(pm: &mut Pixmap, r: Rect, rad: f32, c: ColorU8) {
    let rad = rad.min(r.width() / 2.0).min(r.height() / 2.0);
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    let k = 0.5523 * rad;
    let mut pb = PathBuilder::new();
    pb.move_to(l + rad, t);
    pb.line_to(rt - rad, t);
    pb.cubic_to(rt - rad + k, t, rt, t + rad - k, rt, t + rad);
    pb.line_to(rt, b - rad);
    pb.cubic_to(rt, b - rad + k, rt - rad + k, b, rt - rad, b);
    pb.line_to(l + rad, b);
    pb.cubic_to(l + rad - k, b, l, b - rad + k, l, b - rad);
    pb.line_to(l, t + rad);
    pb.cubic_to(l, t + rad - k, l + rad - k, t, l + rad, t);
    pb.close();
    if let Some(p) = pb.finish() {
        pm.fill_path(
            &p,
            &paint(color(c)),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Single-line text style for other surfaces such as the panel.
#[derive(Clone, Copy, Debug)]
pub struct TextStyle {
    /// Physical pixels.
    pub size: f32,
    pub bold: bool,
    pub color: ColorU8,
}

impl Renderer {
    /// Measures a line of text: (width, height), physical pixels.
    pub fn measure(&mut self, text: &str, style: TextStyle) -> (f32, f32) {
        let Some(t) = self.text.as_mut() else {
            return (
                text.chars().count() as f32 * style.size * 0.55,
                style.size * 1.3,
            );
        };
        t.layout_line(text, style);
        let mut w = 0.0f32;
        for run in t.buffer.layout_runs() {
            w = w.max(run.line_w);
        }
        (w.ceil(), (style.size * 1.3).ceil())
    }

    /// Draws a line of text with its top-left corner at (x, y); returns its width.
    pub fn draw_text(
        &mut self,
        pm: &mut Pixmap,
        x: f32,
        y: f32,
        text: &str,
        style: TextStyle,
    ) -> f32 {
        let (w, _) = self.measure(text, style);
        if let Some(t) = self.text.as_mut() {
            t.left = 0.0;
            t.draw(pm, x, y, cc(style.color));
        }
        w
    }

    /// Draws a frame of the dog at `px` scale, with its feet point at (`x`, `ground`), wearing
    /// its outfit.
    pub fn draw_dog(
        &self,
        pm: &mut Pixmap,
        anim: Anim,
        frame: usize,
        x: i32,
        ground: i32,
        px: i32,
    ) {
        self.blit_dog(pm, &DogFrame::still(anim, frame), x, ground, px);
    }

    /// Draws the dog fitted into a `size`-pixel square with its top-left corner at (x, y).
    /// Downscaling uses a box filter; edge pixels are thresholded so they stay sharp.
    pub fn draw_dog_icon(&self, pm: &mut Pixmap, anim: Anim, x: i32, y: i32, size: u32) {
        let (x0, y0, x1, y1) = opaque_box(&self.atlas, anim, 0);
        let (w, h) = (x1 - x0, y1 - y0);
        let k = size as f32 / w.max(h) as f32;
        let (ow, oh) = (
            ((w as f32) * k).round() as u32,
            ((h as f32) * k).round() as u32,
        );
        let (ox, oy) = (x + (size - ow) as i32 / 2, y + (size - oh) as i32);
        let src = self.atlas.pixels();
        let aw = self.atlas.width();
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let dst = pm.pixels_mut();
        for dy in 0..oh {
            for dx in 0..ow {
                // Source area: the atlas pixels covered by this target pixel.
                let sx0 = x0 + (dx as f32 / k) as u32;
                let sy0 = y0 + (dy as f32 / k) as u32;
                let sx1 = (x0 + ((dx + 1) as f32 / k).ceil() as u32).clamp(sx0 + 1, x1);
                let sy1 = (y0 + ((dy + 1) as f32 / k).ceil() as u32).clamp(sy0 + 1, y1);
                let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
                for sy in sy0..sy1 {
                    for sx in sx0..sx1 {
                        let p = src[(sy * aw + sx) as usize];
                        r += p.red() as u32;
                        g += p.green() as u32;
                        b += p.blue() as u32;
                        a += p.alpha() as u32;
                        n += 1;
                    }
                }
                if n == 0 || a / n < 128 {
                    continue;
                }
                // Convert the premultiplied average to a fully opaque color.
                let c = |v: u32| (v * 255 / a.max(1)).min(255) as u8;
                let (tx, ty) = (ox + dx as i32, oy + dy as i32);
                if tx < 0 || ty < 0 || tx >= pw || ty >= ph {
                    continue;
                }
                if let Some(p) = PremultipliedColorU8::from_rgba(c(r), c(g), c(b), 255) {
                    dst[(ty * pw + tx) as usize] = p;
                }
            }
        }
    }

    /// Height of an animation above the ground (sprite pixels).
    pub fn dog_height(&self, anim: Anim) -> u32 {
        self.heights[anim.row() as usize]
    }
}

impl Text {
    fn layout_line(&mut self, text: &str, style: TextStyle) {
        let family = self.family.clone();
        let size = style.size;
        let weight = if style.bold {
            Weight::BOLD
        } else {
            Weight::NORMAL
        };
        let attrs = Attrs::new()
            .family(Family::Name(&family))
            .weight(weight)
            .color(cc(style.color));
        self.buffer
            .set_metrics(Metrics::new(size, (size * 1.3).ceil()));
        self.buffer.set_wrap(Wrap::None);
        self.buffer.set_size(None, None);
        self.buffer.set_text(text, &attrs, Shaping::Advanced, None);
        self.buffer.shape_until_scroll(&mut self.fonts, false);
        self.laid_out = None;
    }

    /// Draws the laid-out text with its top-left corner at (x, y).
    fn draw(&mut self, pm: &mut Pixmap, x: f32, y: f32, color: cosmic_text::Color) {
        let (ox, oy) = (x.round() as i32, y.round() as i32);
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let pixels = pm.pixels_mut();
        self.buffer.draw(
            &mut self.fonts,
            &mut self.cache,
            color,
            |gx, gy, w, h, c| {
                let [r, g, b, a] = c.as_rgba();
                if a == 0 {
                    return;
                }
                let Some(src) = ColorU8::from_rgba(r, g, b, a).premultiply().into() else {
                    return;
                };
                for yy in (oy + gy).max(0)..(oy + gy + h as i32).min(ph) {
                    for xx in (ox + gx).max(0)..(ox + gx + w as i32).min(pw) {
                        let d = &mut pixels[(yy * pw + xx) as usize];
                        *d = over(src, *d);
                    }
                }
            },
        );
    }
}

fn cc(c: ColorU8) -> cosmic_text::Color {
    cosmic_text::Color::rgba(c.red(), c.green(), c.blue(), c.alpha())
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn bubble_path(
    r: Rect,
    rad: f32,
    tail_x: f32,
    tail_h: f32,
    tail_w: f32,
    tail_up: bool,
) -> Option<tiny_skia::Path> {
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    let k = 0.5523 * rad;
    let mut pb = PathBuilder::new();
    pb.move_to(l + rad, t);
    if tail_up {
        pb.line_to(tail_x - tail_w, t);
        pb.line_to(tail_x, t - tail_h);
        pb.line_to(tail_x + tail_w, t);
    }
    pb.line_to(rt - rad, t);
    pb.cubic_to(rt - rad + k, t, rt, t + rad - k, rt, t + rad);
    pb.line_to(rt, b - rad);
    pb.cubic_to(rt, b - rad + k, rt - rad + k, b, rt - rad, b);
    if !tail_up {
        pb.line_to(tail_x + tail_w, b);
        pb.line_to(tail_x, b + tail_h);
        pb.line_to(tail_x - tail_w, b);
    }
    pb.line_to(l + rad, b);
    pb.cubic_to(l + rad - k, b, l, b - rad + k, l, b - rad);
    pb.line_to(l, t + rad);
    pb.cubic_to(l, t + rad - k, l + rad - k, t, l + rad, t);
    pb.close();
    pb.finish()
}

/// Whether something is drawn at (`x`, `y`) of `pm` or within `reach` pixels of it: clicks
/// there are for us; elsewhere they go through to what is behind the window.
pub fn drawn_near(pm: &Pixmap, x: i32, y: i32, reach: i32) -> bool {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let (px, py) = (x + dx, y + dy);
            if px >= 0
                && py >= 0
                && px < w
                && py < h
                && pm
                    .pixel(px as u32, py as u32)
                    .is_some_and(|p| p.alpha() > 40)
            {
                return true;
            }
        }
    }
    false
}

/// Bounding box of a cell's opaque pixels (atlas coordinates, right/bottom exclusive).
fn opaque_box(atlas: &Pixmap, anim: Anim, frame: usize) -> (u32, u32, u32, u32) {
    let (cx, cy) = (frame as u32 * CELL.0, anim.row() * CELL.1);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in cy..cy + CELL.1 {
        for x in cx..cx + CELL.0 {
            if atlas.pixel(x, y).is_some_and(|p| p.alpha() > 0) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x0, y0, x1, y1)
}

/// Height above the ground of the highest point across all frames of an animation, hats
/// included.
fn sprite_height(atlas: &Pixmap, anim: Anim, outfit: &Outfit) -> u32 {
    (0..anim.frames())
        .map(|f| {
            let (_, y0, _, _) = opaque_box(atlas, anim, f);
            let body = (anim.row() * CELL.1 + ANCHOR.1).saturating_sub(y0) as i32;
            outfit
                .worn()
                .map(|i| {
                    let (s, (_, ty)) = item_target(i.sprite(), anim, f);
                    -(ty - s.pin.1)
                })
                .fold(body, i32::max)
                .max(0) as u32
        })
        .max()
        .unwrap_or(ANCHOR.1)
}

/// Which picture of an item frame `frame` of `anim` uses, and where its pin goes (relative to
/// the feet point, sprite pixels). Worn items have a picture per action (per frame for happy),
/// fitted on its first frame, and follow the head from there; toys follow it from idle's.
fn item_target(s: look::ItemSprite, anim: Anim, frame: usize) -> (look::ItemPose, (i32, i32)) {
    let cell = anim.cell(frame);
    let (pose, from) = if !s.worn {
        (0, 0)
    } else if anim == Anim::Happy {
        let name = format!("happy{frame}");
        let i = look::POSES.iter().position(|p| *p == name).unwrap_or(0);
        (i, cell)
    } else {
        (anim.row() as usize, anim.cell(0))
    };
    let p = s.poses[pose];
    let (hx, hy) = HEAD[cell];
    let (fx, fy) = HEAD[from];
    (p, (p.at.0 + (hx - fx) as i32, p.at.1 + (hy - fy) as i32))
}

/// Draws `rect` of `src` with its top-left at `at` (sprite pixels from the feet point),
/// mirrored around the feet point when `flip`. `(ax, ground, px)` as in [`blit_sprite`].
fn blit_region(
    pm: &mut Pixmap,
    src: &Pixmap,
    (rx, ry, w, h): (u32, u32, u32, u32),
    at: (i32, i32),
    flip: bool,
    (ax, ground, px): (i32, i32, i32),
) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let sw = src.width();
    let s = src.pixels();
    let dst = pm.pixels_mut();
    for sy in 0..h {
        let y0 = ground + (at.1 + sy as i32) * px;
        for sx in 0..w {
            let p = s[((ry + sy) * sw + rx + sx) as usize];
            if p.alpha() == 0 {
                continue;
            }
            let dx = at.0 + sx as i32;
            let x0 = if flip {
                ax - (dx + 1) * px
            } else {
                ax + dx * px
            };
            for y in y0.max(0)..(y0 + px).min(ph) {
                for x in x0.max(0)..(x0 + px).min(pw) {
                    let d = &mut dst[(y * pw + x) as usize];
                    *d = over(p, *d);
                }
            }
        }
    }
}

/// Draws a sprite frame at `px` scale with nearest-neighbor; if `facing < 0`, mirrors it around the feet point.
pub fn blit_sprite(pm: &mut Pixmap, atlas: &Pixmap, f: &DogFrame, ax: i32, ground: i32, px: i32) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let (cx, cy) = (f.frame as u32 * CELL.0, f.anim.row() * CELL.1);
    let flip = f.facing < 0.0;
    let src = atlas.pixels();
    let aw = atlas.width();
    let dst = pm.pixels_mut();
    for sy in 0..CELL.1 {
        let y0 = ground + (sy as i32 - ANCHOR.1 as i32) * px;
        for sx in 0..CELL.0 {
            let p = src[((cy + sy) * aw + cx + sx) as usize];
            if p.alpha() == 0 {
                continue;
            }
            let dx = sx as i32 - ANCHOR.0 as i32;
            let x0 = if flip {
                ax - (dx + 1) * px
            } else {
                ax + dx * px
            };
            for y in y0.max(0)..(y0 + px).min(ph) {
                for x in x0.max(0)..(x0 + px).min(pw) {
                    let d = &mut dst[(y * pw + x) as usize];
                    *d = over(p, *d);
                }
            }
        }
    }
}

/// A pin on the collar, beside the tag (`NECK`), on the side towards the back.
fn draw_pin(pm: &mut Pixmap, pin: Pin, f: &DogFrame, (ax, ground, px): (i32, i32, i32)) {
    let (rows, colors) = pin.pixels();
    let (nx, ny) = NECK[f.anim.cell(f.frame)];
    let w = rows[0].len() as i32;
    // On the collar behind the tag (the tag hangs at the neck's centre), the top at the band.
    let (left, top) = (nx as i32 - w - 4, ny as i32 - 3);
    let flip = f.facing < 0.0;
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let dst = pm.pixels_mut();
    for (ry, row) in rows.iter().enumerate() {
        for (rx, ch) in row.chars().enumerate() {
            let Some((_, c)) = colors.iter().find(|(k, _)| *k == ch) else {
                continue;
            };
            let Some(p) = PremultipliedColorU8::from_rgba(c[0], c[1], c[2], c[3]) else {
                continue;
            };
            // Mirrored with the dog, but the picture itself is never turned around.
            let col = if flip { w - 1 - rx as i32 } else { rx as i32 };
            let dx = left + col;
            let x0 = if flip {
                ax - (dx + 1) * px
            } else {
                ax + dx * px
            };
            let y0 = ground + (top + ry as i32) * px;
            for y in y0.max(0)..(y0 + px).min(ph) {
                for x in x0.max(0)..(x0 + px).min(pw) {
                    dst[(y * pw + x) as usize] = p;
                }
            }
        }
    }
}

fn over(s: PremultipliedColorU8, d: PremultipliedColorU8) -> PremultipliedColorU8 {
    if s.alpha() == 255 {
        return s;
    }
    let inv = 255 - s.alpha() as u32;
    let m = |a: u8, b: u8| (a as u32 + (b as u32 * inv + 127) / 255).min(255) as u8;
    let a = m(s.alpha(), d.alpha());
    PremultipliedColorU8::from_rgba(
        m(s.red(), d.red()).min(a),
        m(s.green(), d.green()).min(a),
        m(s.blue(), d.blue()).min(a),
        a,
    )
    .unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleeping_icon_has_nothing_above_the_dog() {
        let r = Renderer::new(false, Vec::new());
        for size in [32, 44, 64] {
            let pm = r.icon_pixmap(size, true, false);
            let top = pm.pixels()[..(size * size / 3) as usize]
                .iter()
                .filter(|p| p.alpha() > 0)
                .count();
            assert_eq!(top, 0, "{size} px: something drawn above the sleeping dog");
        }
    }
}
