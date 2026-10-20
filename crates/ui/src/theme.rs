//! Port of ui.ts THEME (and Ink's colorize.js) on ratatui colors, in one of two modes ([`ThemeMode`]).
//!
//! Terminal mode keeps ui.ts's palette: ANSI named colors, so it follows the user's terminal theme. Ink colors by chalk
//! name; ratatui names the 16 ANSI colors differently: chalk `white` (SGR 37) is ratatui `Gray`, chalk `gray`
//! (`blackBright`, SGR 90) is `DarkGray`, chalk `whiteBright` (SGR 97) is `White`, and every other `<name>Bright` is
//! `Light<Name>`. Dusk mode draws the design system's palette ([`DUSK`]) in 24-bit color on its own ground instead:
//! the same Ink names map to its tokens, and [`apply_dusk`] resolves what a terminal would otherwise decide (default
//! colors, inverse, dim). [`deckhand_core::display::THEME`] keeps the Ink names (core helpers return colors as names);
//! [`color`] maps them.

use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};

use deckhand_core::display::{DUSK, Rgb, ThemeMode};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

/// Whether dusk mode is on (terminal mode until [`set_mode`] says otherwise).
static DUSK_ON: AtomicBool = AtomicBool::new(false);

/// Chooses the theme every later draw uses (the UI sets it at startup from the `theme` setting).
pub fn set_mode(mode: ThemeMode) {
    DUSK_ON.store(mode == ThemeMode::Dusk, Ordering::Relaxed);
}

/// The theme in use.
pub fn mode() -> ThemeMode {
    if DUSK_ON.load(Ordering::Relaxed) {
        ThemeMode::Dusk
    } else {
        ThemeMode::Terminal
    }
}

/// A palette color as a ratatui color.
pub const fn rgb(color: Rgb) -> Color {
    Color::Rgb(color.0, color.1, color.2)
}

/// TS: ink colorize.js for a foreground: the ratatui color of an Ink color name (`magenta`, `gray`, ...), hex
/// (`#rrggbb`, `#rgb`), `rgb(r, g, b)` or `ansi256(n)`. Unknown names draw uncolored (`None`), as chalk leaves them.
/// In dusk mode the theme's names map to the dusk palette instead ([`dusk_color`]).
pub fn color(name: &str) -> Option<Color> {
    if mode() == ThemeMode::Dusk
        && let Some(color) = dusk_color(name)
    {
        return Some(color);
    }
    ansi_color(name)
}

/// The dusk token for an Ink color name the theme uses (`magenta`, `gray`, ...); `None` for the rest (`blue`, hex,
/// ...), which keep their [`ansi_color`].
pub fn dusk_color(name: &str) -> Option<Color> {
    Some(rgb(match name {
        "magenta" => DUSK.magenta,
        "magentaBright" => DUSK.magenta_bright,
        "cyan" => DUSK.cyan,
        "green" => DUSK.green,
        "yellow" => DUSK.yellow,
        "red" => DUSK.red,
        "gray" | "grey" | "blackBright" | "white" => DUSK.muted,
        "whiteBright" => DUSK.ink,
        _ => return None,
    }))
}

/// Ink colorize.js on the ANSI palette (terminal mode's [`color`]).
pub fn ansi_color(name: &str) -> Option<Color> {
    Some(match name {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::Gray,
        "gray" | "grey" | "blackBright" => Color::DarkGray,
        "redBright" => Color::LightRed,
        "greenBright" => Color::LightGreen,
        "yellowBright" => Color::LightYellow,
        "blueBright" => Color::LightBlue,
        "magentaBright" => Color::LightMagenta,
        "cyanBright" => Color::LightCyan,
        "whiteBright" => Color::White,
        _ => return parse_color_function(name),
    })
}

/// `#rrggbb` / `#rgb` (chalk.hex), `rgb(r, g, b)` (colorize.js rgbRegex) and `ansi256(n)` (ansiRegex).
fn parse_color_function(name: &str) -> Option<Color> {
    if let Some(hex) = name.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .map(|c| c.to_digit(16).and_then(|value| u8::try_from(value).ok()))
            .collect::<Option<_>>()?;
        return match digits.as_slice() {
            [r, g, b] => Some(Color::Rgb(r * 17, g * 17, b * 17)),
            [r1, r2, g1, g2, b1, b2] => Some(Color::Rgb(r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2)),
            _ => None,
        };
    }
    if let Some(inner) = name.strip_prefix("ansi256(").and_then(|rest| rest.strip_suffix(')')) {
        return inner.trim().parse::<u8>().ok().map(Color::Indexed);
    }
    if let Some(inner) = name.strip_prefix("rgb(").and_then(|rest| rest.strip_suffix(')')) {
        let parts: Vec<u8> = inner
            .split(',')
            .map(|part| part.trim().parse::<u8>().ok())
            .collect::<Option<_>>()?;
        if let [r, g, b] = parts.as_slice() {
            return Some(Color::Rgb(*r, *g, *b));
        }
    }
    None
}

/// TS: ui.ts THEME, as ratatui colors, plus what dusk mode paints itself (`None` in terminal mode: the terminal's own).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub accent: Color,
    pub accent_soft: Color,
    pub active: Color,
    pub muted: Color,
    pub border: Color,
    pub border_active: Color,
    pub border_danger: Color,
    pub success: Color,
    pub warn: Color,
    pub error: Color,
    /// Key hints (the footer), a step quieter than muted.
    pub hint: Color,
    /// The background under everything.
    pub ground: Option<Color>,
    /// The background inside bordered boxes.
    pub box_fill: Option<Color>,
    /// Text without a color of its own.
    pub text: Option<Color>,
    /// Bold text without a color of its own, and the fill of inverse text without one (the cursor).
    pub ink: Option<Color>,
    /// Text on a fill (the inverse selection bar, the cursor).
    pub ink_dark: Option<Color>,
}

/// TS: ui.ts THEME.
pub const TERMINAL_THEME: Theme = Theme {
    accent: Color::Magenta,
    accent_soft: Color::LightMagenta,
    active: Color::Cyan,
    muted: Color::DarkGray,
    border: Color::DarkGray,
    border_active: Color::Magenta,
    border_danger: Color::Red,
    success: Color::Green,
    warn: Color::Yellow,
    error: Color::Red,
    hint: Color::DarkGray,
    ground: None,
    box_fill: None,
    text: None,
    ink: None,
    ink_dark: None,
};

/// ui.ts THEME's roles on the dusk palette. Muted text is the `muted` token (dim text fades it to about `dim`, see
/// [`apply_dusk`]), key hints are `dim` and borders `edge`.
pub const DUSK_THEME: Theme = Theme {
    accent: rgb(DUSK.magenta),
    accent_soft: rgb(DUSK.magenta_bright),
    active: rgb(DUSK.cyan),
    muted: rgb(DUSK.muted),
    border: rgb(DUSK.edge),
    border_active: rgb(DUSK.magenta),
    border_danger: rgb(DUSK.red),
    success: rgb(DUSK.green),
    warn: rgb(DUSK.yellow),
    error: rgb(DUSK.red),
    hint: rgb(DUSK.dim),
    ground: Some(rgb(DUSK.term)),
    box_fill: Some(rgb(DUSK.box_fill)),
    // Plain text in muted, as the design system draws terminal content; ink only for emphasis.
    text: Some(rgb(DUSK.muted)),
    ink: Some(rgb(DUSK.ink)),
    ink_dark: Some(rgb(DUSK.ink_dark)),
};

/// The theme of the current [`mode`] (`THEME.accent`, ...).
pub struct CurrentTheme;

impl Deref for CurrentTheme {
    type Target = Theme;

    fn deref(&self) -> &Theme {
        match mode() {
            ThemeMode::Dusk => &DUSK_THEME,
            ThemeMode::Terminal => &TERMINAL_THEME,
        }
    }
}

/// TS: ui.ts THEME, in the current mode.
pub static THEME: CurrentTheme = CurrentTheme;

/// How far dim text fades toward its background.
const DIM_FADE: f32 = 0.4;

/// `color` moved `amount` of the way toward `toward` (both 24-bit; otherwise `None`).
fn mix(color: Color, toward: Color, amount: f32) -> Option<Color> {
    let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (color, toward) else {
        return None;
    };
    // Rounded and clamped, so the cast cannot truncate.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let channel = |a: u8, b: u8| {
        (f32::from(a) + (f32::from(b) - f32::from(a)) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Some(Color::Rgb(channel(r1, r2), channel(g1, g2), channel(b1, b2)))
}

/// Dusk mode's last step over a drawn frame: what the terminal would otherwise decide is drawn in `theme`'s colors.
/// Default backgrounds become the ground and default text muted (bold: ink); inverse becomes a fill in the text's
/// color (default: ink) with dark text on it (so a cyan selection is a cyan bar), and dim text fades toward its
/// background. A theme without a ground (terminal mode) leaves the buffer alone.
pub fn apply_dusk(buf: &mut Buffer, area: Rect, theme: &Theme) {
    let (Some(ground), Some(text), Some(ink), Some(ink_dark)) = (theme.ground, theme.text, theme.ink, theme.ink_dark)
    else {
        return;
    };
    let area = buf.area.intersection(area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            let mut modifier = cell.modifier;
            let mut fg = match cell.fg {
                Color::Reset if modifier.intersects(Modifier::BOLD | Modifier::REVERSED) => ink,
                Color::Reset => text,
                color => color,
            };
            let mut bg = if cell.bg == Color::Reset { ground } else { cell.bg };
            if modifier.contains(Modifier::REVERSED) {
                // On the UI's own backgrounds the text on the fill is ink-dark, not the ground.
                let plain = cell.bg == Color::Reset || Some(cell.bg) == theme.box_fill;
                (fg, bg) = (if plain { ink_dark } else { bg }, fg);
                modifier.remove(Modifier::REVERSED);
            }
            if modifier.contains(Modifier::DIM)
                && let Some(faded) = mix(fg, bg, DIM_FADE)
            {
                fg = faded;
                modifier.remove(Modifier::DIM);
            }
            cell.fg = fg;
            cell.bg = bg;
            cell.modifier = modifier;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_core::display::THEME as INK_THEME;

    #[test]
    fn theme_matches_the_ink_names() {
        let ink = INK_THEME;
        let theme = TERMINAL_THEME;
        let pairs = [
            (ink.accent, theme.accent),
            (ink.accent_soft, theme.accent_soft),
            (ink.active, theme.active),
            (ink.muted, theme.muted),
            (ink.border, theme.border),
            (ink.border_active, theme.border_active),
            (ink.border_danger, theme.border_danger),
            (ink.success, theme.success),
            (ink.warn, theme.warn),
            (ink.error, theme.error),
        ];
        for (name, mapped) in pairs {
            assert_eq!(ansi_color(name), Some(mapped), "{name}");
        }
    }

    #[test]
    fn dusk_maps_the_ink_names_to_its_tokens() {
        let ink = INK_THEME;
        let theme = DUSK_THEME;
        let pairs = [
            (ink.accent, theme.accent),
            (ink.accent_soft, theme.accent_soft),
            (ink.active, theme.active),
            (ink.muted, theme.muted),
            (ink.border_active, theme.border_active),
            (ink.border_danger, theme.border_danger),
            (ink.success, theme.success),
            (ink.warn, theme.warn),
            (ink.error, theme.error),
        ];
        for (name, mapped) in pairs {
            assert_eq!(dusk_color(name), Some(mapped), "{name}");
        }
        // Borders are the edge token, a step lighter than muted text.
        assert_eq!(theme.border, rgb(DUSK.edge));
        assert_eq!(dusk_color("magenta"), Some(Color::Rgb(0xf7, 0xa8, 0xf0)));
        assert_eq!(dusk_color("blue"), None);
        assert_eq!(dusk_color("#ff8000"), None);
    }

    #[test]
    fn the_mode_is_terminal_until_set() {
        assert_eq!(mode(), ThemeMode::Terminal);
        assert_eq!(*THEME, TERMINAL_THEME);
    }

    #[test]
    fn dusk_paints_the_ground_muted_text_and_a_dark_on_cyan_selection() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 1));
        buf[(1, 0)].set_fg(DUSK_THEME.active).modifier = Modifier::REVERSED | Modifier::BOLD;
        buf[(2, 0)].modifier = Modifier::REVERSED;
        buf[(3, 0)].set_fg(DUSK_THEME.muted).modifier = Modifier::DIM;
        buf[(4, 0)].modifier = Modifier::BOLD;
        apply_dusk(&mut buf, Rect::new(0, 0, 5, 1), &DUSK_THEME);
        let ground = rgb(DUSK.term);
        assert_eq!((buf[(0, 0)].fg, buf[(0, 0)].bg), (rgb(DUSK.muted), ground));
        assert_eq!(buf[(4, 0)].fg, rgb(DUSK.ink));
        assert_eq!((buf[(1, 0)].fg, buf[(1, 0)].bg), (rgb(DUSK.ink_dark), rgb(DUSK.cyan)));
        assert_eq!(buf[(1, 0)].modifier, Modifier::BOLD);
        assert_eq!((buf[(2, 0)].fg, buf[(2, 0)].bg), (rgb(DUSK.ink_dark), rgb(DUSK.ink)));
        // Dim: 40% of the way from muted (#c3c8d0) to the ground (#363c46), close to the dim token (#848c99).
        assert_eq!(buf[(3, 0)].fg, Color::Rgb(0x8b, 0x90, 0x99));
        assert_eq!(buf[(3, 0)].modifier, Modifier::empty());
    }

    #[test]
    fn dusk_keeps_explicit_backgrounds_and_ansi_dim() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        buf[(0, 0)].set_fg(Color::Blue).modifier = Modifier::DIM;
        buf[(1, 0)].set_fg(DUSK_THEME.active).set_bg(DUSK_THEME.warn).modifier = Modifier::REVERSED;
        apply_dusk(&mut buf, Rect::new(0, 0, 4, 1), &DUSK_THEME);
        assert_eq!((buf[(0, 0)].fg, buf[(0, 0)].modifier), (Color::Blue, Modifier::DIM));
        assert_eq!((buf[(1, 0)].fg, buf[(1, 0)].bg), (DUSK_THEME.warn, DUSK_THEME.active));
    }

    #[test]
    fn terminal_mode_leaves_the_buffer_alone() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        buf[(0, 0)].modifier = Modifier::REVERSED | Modifier::DIM;
        let before = buf.clone();
        apply_dusk(&mut buf, Rect::new(0, 0, 4, 1), &TERMINAL_THEME);
        assert_eq!(buf, before);
    }

    #[test]
    fn maps_chalk_names_to_ratatui_ansi_colors() {
        assert_eq!(color("gray"), Some(Color::DarkGray));
        assert_eq!(color("white"), Some(Color::Gray));
        assert_eq!(color("whiteBright"), Some(Color::White));
        assert_eq!(color("magentaBright"), Some(Color::LightMagenta));
        assert_eq!(color("nope"), None);
        assert_eq!(color("#ff8000"), Some(Color::Rgb(255, 128, 0)));
        assert_eq!(color("#f80"), Some(Color::Rgb(255, 136, 0)));
        assert_eq!(color("rgb(1, 2,3)"), Some(Color::Rgb(1, 2, 3)));
        assert_eq!(color("ansi256(208)"), Some(Color::Indexed(208)));
        assert_eq!(color("#zzz"), None);
    }
}
