//! Pixel scenes (`docs/spec/ui.md` §1, ADR-0011): what a screen draws in pixels on the panel,
//! as a list of primitives (text in proportional fonts, icons, rounded boxes, circles, lines,
//! a dither). A scene is data: screens build it, tests read it, and [`draw`] paints it into
//! any 1-bit target.
//!
//! Fonts and icons come from U8g2 through `u8g2-fonts` (`NOTICE`): Helvetica from the X11
//! fonts (Adobe/DEC licence), Spleen for big digits (BSD-2-Clause), Open Iconic for icons
//! (SIL OFL). Characters a font lacks are skipped.

use alloc::string::String;
use alloc::vec::Vec;

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{
    Circle, CornerRadii, Line, PrimitiveStyle, Rectangle, RoundedRectangle,
};
use u8g2_fonts::types::{FontColor, HorizontalAlignment, VerticalPosition};
use u8g2_fonts::{FontRenderer, fonts};

/// Text fonts: Helvetica regular and bold by pixel size, and big digits for clocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    R8,
    B8,
    R10,
    B10,
    R12,
    B12,
    B14,
    /// Spleen 16×32, for clocks.
    Digits,
}

impl Font {
    fn renderer(self) -> &'static FontRenderer {
        static R8: FontRenderer =
            FontRenderer::new::<fonts::u8g2_font_helvR08_te>().with_ignore_unknown_chars(true);
        static B8: FontRenderer =
            FontRenderer::new::<fonts::u8g2_font_helvB08_te>().with_ignore_unknown_chars(true);
        static R10: FontRenderer =
            FontRenderer::new::<fonts::u8g2_font_helvR10_te>().with_ignore_unknown_chars(true);
        static B10: FontRenderer =
            FontRenderer::new::<fonts::u8g2_font_helvB10_te>().with_ignore_unknown_chars(true);
        static R12: FontRenderer =
            FontRenderer::new::<fonts::u8g2_font_helvR12_te>().with_ignore_unknown_chars(true);
        static B12: FontRenderer =
            FontRenderer::new::<fonts::u8g2_font_helvB12_te>().with_ignore_unknown_chars(true);
        static B14: FontRenderer =
            FontRenderer::new::<fonts::u8g2_font_helvB14_te>().with_ignore_unknown_chars(true);
        static DIGITS: FontRenderer =
            FontRenderer::new::<fonts::u8g2_font_spleen16x32_mf>().with_ignore_unknown_chars(true);
        match self {
            Font::R8 => &R8,
            Font::B8 => &B8,
            Font::R10 => &R10,
            Font::B10 => &B10,
            Font::R12 => &R12,
            Font::B12 => &B12,
            Font::B14 => &B14,
            Font::Digits => &DIGITS,
        }
    }

    /// Pixels from one baseline to the next.
    pub fn line_height(self) -> i16 {
        match self {
            Font::R8 | Font::B8 => 10,
            Font::R10 | Font::B10 => 13,
            Font::R12 | Font::B12 => 15,
            Font::B14 => 17,
            Font::Digits => 32,
        }
    }

    /// Pixels above the baseline taken by capitals.
    pub fn ascent(self) -> i16 {
        i16::from(self.renderer().get_ascent())
    }
}

/// Icons (Open Iconic glyph codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Mail,
    Chat,
    Bluetooth,
    BatteryFull,
    BatteryEmpty,
    Charging,
    Pin,
    People,
    Person,
    Settings,
    Tools,
    Check,
    Cross,
    Pencil,
    Broadcast,
    Signal,
    Star,
    Trash,
    Route,
    Qr,
    Clock,
    Sun,
    Bell,
    Lock,
    Plus,
    Warning,
    Refresh,
    Search,
    Map,
    List,
    Monitor,
    Power,
    Eye,
    Graph,
    Pulse,
}

impl Icon {
    fn code(self) -> u32 {
        match self {
            Icon::Mail => 156,
            Icon::Chat => 135,
            Icon::Bluetooth => 94,
            Icon::BatteryFull => 91,
            Icon::BatteryEmpty => 90,
            Icon::Charging => 96,
            Icon::Pin => 209,
            Icon::People => 228,
            Icon::Person => 229,
            Icon::Settings => 129,
            Icon::Tools => 282,
            Icon::Check => 115,
            Icon::Cross => 283,
            Icon::Pencil => 227,
            Icon::Broadcast => 84,
            Icon::Signal => 253,
            Icon::Star => 258,
            Icon::Trash => 271,
            Icon::Route => 201,
            Icon::Qr => 178,
            Icon::Clock => 123,
            Icon::Sun => 259,
            Icon::Bell => 93,
            Icon::Lock => 202,
            Icon::Plus => 234,
            Icon::Warning => 280,
            Icon::Refresh => 243,
            Icon::Search => 207,
            Icon::Map => 208,
            Icon::List => 199,
            Icon::Monitor => 222,
            Icon::Power => 235,
            Icon::Eye => 165,
            Icon::Graph => 176,
            Icon::Pulse => 238,
        }
    }
}

/// Icon size: 8 or 16 pixels square.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSize {
    S8,
    S16,
}

impl IconSize {
    pub fn px(self) -> i16 {
        match self {
            IconSize::S8 => 8,
            IconSize::S16 => 16,
        }
    }

    fn renderer(self) -> &'static FontRenderer {
        static S8: FontRenderer = FontRenderer::new::<fonts::u8g2_font_open_iconic_all_1x_t>();
        static S16: FontRenderer = FontRenderer::new::<fonts::u8g2_font_open_iconic_all_2x_t>();
        match self {
            IconSize::S8 => &S8,
            IconSize::S16 => &S16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// One primitive. Coordinates are pixels from the scene's top-left corner; `ink` false draws
/// white (on a black box).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// Text on its baseline at `y`, anchored at `x` by `align`.
    Text {
        x: i16,
        y: i16,
        font: Font,
        align: Align,
        ink: bool,
        text: String,
    },
    /// An icon, its top-left corner at (`x`, `y`).
    Icon {
        x: i16,
        y: i16,
        icon: Icon,
        size: IconSize,
        ink: bool,
    },
    /// A box with rounded corners (radius 0: square), filled and/or outlined.
    Rect {
        x: i16,
        y: i16,
        w: u16,
        h: u16,
        radius: u8,
        fill: Option<bool>,
        stroke: Option<bool>,
    },
    /// A disc or a ring of diameter `d` centred on (`cx`, `cy`).
    Circle {
        cx: i16,
        cy: i16,
        d: u16,
        filled: bool,
        ink: bool,
    },
    Line {
        x0: i16,
        y0: i16,
        x1: i16,
        y1: i16,
        ink: bool,
    },
    /// Every other pixel turned white: what is under it fades.
    Dither { x: i16, y: i16, w: u16, h: u16 },
}

/// A list of primitives, drawn in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scene {
    pub items: Vec<Item>,
}

fn color(ink: bool) -> BinaryColor {
    if ink {
        BinaryColor::On
    } else {
        BinaryColor::Off
    }
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&mut self, x: i16, y: i16, font: Font, align: Align, ink: bool, text: &str) {
        self.items.push(Item::Text {
            x,
            y,
            font,
            align,
            ink,
            text: String::from(text),
        });
    }

    pub fn icon(&mut self, x: i16, y: i16, icon: Icon, size: IconSize, ink: bool) {
        self.items.push(Item::Icon {
            x,
            y,
            icon,
            size,
            ink,
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rect(
        &mut self,
        x: i16,
        y: i16,
        w: u16,
        h: u16,
        radius: u8,
        fill: Option<bool>,
        stroke: Option<bool>,
    ) {
        self.items.push(Item::Rect {
            x,
            y,
            w,
            h,
            radius,
            fill,
            stroke,
        });
    }

    pub fn circle(&mut self, cx: i16, cy: i16, d: u16, filled: bool, ink: bool) {
        self.items.push(Item::Circle {
            cx,
            cy,
            d,
            filled,
            ink,
        });
    }

    pub fn hline(&mut self, x0: i16, x1: i16, y: i16) {
        self.items.push(Item::Line {
            x0,
            y0: y,
            x1,
            y1: y,
            ink: true,
        });
    }

    pub fn vline(&mut self, x: i16, y0: i16, y1: i16) {
        self.items.push(Item::Line {
            x0: x,
            y0,
            x1: x,
            y1,
            ink: true,
        });
    }

    pub fn dither(&mut self, x: i16, y: i16, w: u16, h: u16) {
        self.items.push(Item::Dither { x, y, w, h });
    }

    /// Everything shifted by (`dx`, `dy`).
    pub fn append(&mut self, other: Scene, dx: i16, dy: i16) {
        for mut it in other.items {
            match &mut it {
                Item::Text { x, y, .. }
                | Item::Icon { x, y, .. }
                | Item::Rect { x, y, .. }
                | Item::Dither { x, y, .. } => {
                    *x += dx;
                    *y += dy;
                }
                Item::Circle { cx, cy, .. } => {
                    *cx += dx;
                    *cy += dy;
                }
                Item::Line { x0, y0, x1, y1, .. } => {
                    *x0 += dx;
                    *x1 += dx;
                    *y0 += dy;
                    *y1 += dy;
                }
            }
            self.items.push(it);
        }
    }

    /// The texts of the scene in order, for tests.
    pub fn texts(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// Paints `scene` with its top-left corner at `origin`.
pub fn draw<D: DrawTarget<Color = BinaryColor>>(
    scene: &Scene,
    origin: Point,
    d: &mut D,
) -> Result<(), D::Error> {
    let p = |x: i16, y: i16| origin + Point::new(i32::from(x), i32::from(y));
    for item in &scene.items {
        match item {
            Item::Text {
                x,
                y,
                font,
                align,
                ink,
                text,
            } => {
                let h = match align {
                    Align::Left => HorizontalAlignment::Left,
                    Align::Center => HorizontalAlignment::Center,
                    Align::Right => HorizontalAlignment::Right,
                };
                // A font error (a glyph out of the font) only loses that text.
                let _ = font.renderer().render_aligned(
                    text.as_str(),
                    p(*x, *y),
                    VerticalPosition::Baseline,
                    h,
                    FontColor::Transparent(color(*ink)),
                    d,
                );
            }
            Item::Icon {
                x,
                y,
                icon,
                size,
                ink,
            } => {
                if let Some(ch) = char::from_u32(icon.code()) {
                    let _ = size.renderer().render(
                        ch,
                        p(*x, *y),
                        VerticalPosition::Top,
                        FontColor::Transparent(color(*ink)),
                        d,
                    );
                }
            }
            Item::Rect {
                x,
                y,
                w,
                h,
                radius,
                fill,
                stroke,
            } => {
                let r = RoundedRectangle::new(
                    Rectangle::new(p(*x, *y), Size::new(u32::from(*w), u32::from(*h))),
                    CornerRadii::new(Size::new(u32::from(*radius), u32::from(*radius))),
                );
                if let Some(f) = fill {
                    r.into_styled(PrimitiveStyle::with_fill(color(*f)))
                        .draw(d)?;
                }
                if let Some(s) = stroke {
                    r.into_styled(PrimitiveStyle::with_stroke(color(*s), 1))
                        .draw(d)?;
                }
            }
            Item::Circle {
                cx,
                cy,
                d: dia,
                filled,
                ink,
            } => {
                let c = Circle::with_center(p(*cx, *cy), u32::from(*dia));
                let style = if *filled {
                    PrimitiveStyle::with_fill(color(*ink))
                } else {
                    PrimitiveStyle::with_stroke(color(*ink), 1)
                };
                c.into_styled(style).draw(d)?;
            }
            Item::Line {
                x0,
                y0,
                x1,
                y1,
                ink,
            } => {
                Line::new(p(*x0, *y0), p(*x1, *y1))
                    .into_styled(PrimitiveStyle::with_stroke(color(*ink), 1))
                    .draw(d)?;
            }
            Item::Dither { x, y, w, h } => {
                let o = p(*x, *y);
                let pixels = (0..i32::from(*h)).flat_map(move |dy| {
                    (0..i32::from(*w))
                        .filter(move |dx| (dx + dy + o.x + o.y) % 2 == 0)
                        .map(move |dx| Pixel(o + Point::new(dx, dy), BinaryColor::Off))
                });
                d.draw_iter(pixels)?;
            }
        }
    }
    Ok(())
}

/// Width of `s` in `font`, in pixels.
pub fn width(font: Font, s: &str) -> i16 {
    font.renderer()
        .get_rendered_dimensions(s, Point::zero(), VerticalPosition::Baseline)
        .map_or(0, |d| i16::try_from(d.advance.x).unwrap_or(i16::MAX))
}

/// Width of one character: advances add up, so strings are measured in one pass.
fn char_width(font: Font, c: char) -> i16 {
    let mut b = [0u8; 4];
    width(font, c.encode_utf8(&mut b))
}

/// `s` cut to `w` pixels, ending with `...` when cut.
pub fn ellipsize(font: Font, s: &str, w: i16) -> String {
    let dots = width(font, "...");
    let mut total = 0i16;
    // Where the text can stop with the dots still fitting.
    let mut keep = 0;
    for (i, ch) in s.char_indices() {
        let cw = char_width(font, ch);
        if total.saturating_add(cw).saturating_add(dots) <= w {
            keep = i + ch.len_utf8();
        }
        total = total.saturating_add(cw);
        if total > w {
            let mut t = String::from(s.get(..keep).unwrap_or("").trim_end());
            t.push_str("...");
            return t;
        }
    }
    String::from(s)
}

/// `s` in lines of at most `w` pixels, broken at spaces; a word longer than a line is cut.
pub fn wrap(font: Font, s: &str, w: i16) -> Vec<String> {
    let space = char_width(font, ' ');
    let mut lines: Vec<String> = Vec::new();
    for para in s.split('\n') {
        let mut line = String::new();
        let mut lw = 0i16;
        for word in para.split(' ') {
            let ww = word
                .chars()
                .fold(0i16, |a, c| a.saturating_add(char_width(font, c)));
            let add = if line.is_empty() { ww } else { space + ww };
            if lw.saturating_add(add) <= w {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
                lw += add;
                continue;
            }
            if !line.is_empty() {
                lines.push(core::mem::take(&mut line));
                lw = 0;
            }
            // The word alone, cut where it no longer fits.
            for ch in word.chars() {
                let cw = char_width(font, ch);
                if lw.saturating_add(cw) > w && !line.is_empty() {
                    lines.push(core::mem::take(&mut line));
                    lw = 0;
                }
                line.push(ch);
                lw = lw.saturating_add(cw);
            }
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measuring_and_wrapping() {
        assert!(width(Font::R10, "Bob") > 0);
        assert!(width(Font::B10, "Bob") >= width(Font::R10, "Bob"));
        assert_eq!(width(Font::R10, ""), 0);
        let lines = wrap(Font::R10, "Coffee at the refuge around 4?", 120);
        assert!(lines.len() >= 2, "{lines:?}");
        assert!(lines.iter().all(|l| width(Font::R10, l) <= 120));
        assert_eq!(lines.join(" "), "Coffee at the refuge around 4?");
        let cut = ellipsize(Font::R8, "Alice: Snow on the pass, take chains", 100);
        assert!(
            cut.ends_with("...") && width(Font::R8, &cut) <= 100,
            "{cut}"
        );
        assert_eq!(ellipsize(Font::R8, "short", 100), "short");
        // A word longer than the line is cut, not lost.
        let long = wrap(Font::R10, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 60);
        assert!(long.len() > 1 && long.concat().len() == 40);
        // Advances add up: what the one-pass measuring relies on.
        for f in [
            Font::R8,
            Font::B8,
            Font::R10,
            Font::B10,
            Font::R12,
            Font::B12,
        ] {
            let s = "Snow on the pass, take chains! œuf à 4?";
            let sum: i16 = s.chars().map(|c| char_width(f, c)).sum();
            assert_eq!(sum, width(f, s), "{f:?}");
        }
        // French accents and œ are in the fonts.
        assert!(width(Font::R10, "œuf à é") > width(Font::R10, "uf  "));
    }
}
