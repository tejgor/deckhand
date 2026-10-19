//! Port of ui.ts THEME (and Ink's colorize.js) on ratatui colors.
//!
//! The palette is ANSI named colors so it follows the user's terminal theme. Ink colors by chalk name; ratatui names
//! the 16 ANSI colors differently: chalk `white` (SGR 37) is ratatui `Gray`, chalk `gray` (`blackBright`, SGR 90) is
//! `DarkGray`, chalk `whiteBright` (SGR 97) is `White`, and every other `<name>Bright` is `Light<Name>`.
//! [`deckhand_core::display::THEME`] keeps the Ink names (core helpers return colors as names); [`color`] maps them.

use ratatui::style::Color;

/// TS: ink colorize.js for a foreground: the ratatui color of an Ink color name (`magenta`, `gray`, ...), hex
/// (`#rrggbb`, `#rgb`), `rgb(r, g, b)` or `ansi256(n)`. Unknown names draw uncolored (`None`), as chalk leaves them.
pub fn color(name: &str) -> Option<Color> {
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

/// TS: ui.ts THEME, as ratatui colors.
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
}

/// TS: ui.ts THEME.
pub const THEME: Theme = Theme {
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
};

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_core::display::THEME as INK_THEME;

    #[test]
    fn theme_matches_the_ink_names() {
        let ink = INK_THEME;
        let pairs = [
            (ink.accent, THEME.accent),
            (ink.accent_soft, THEME.accent_soft),
            (ink.active, THEME.active),
            (ink.muted, THEME.muted),
            (ink.border, THEME.border),
            (ink.border_active, THEME.border_active),
            (ink.border_danger, THEME.border_danger),
            (ink.success, THEME.success),
            (ink.warn, THEME.warn),
            (ink.error, THEME.error),
        ];
        for (name, mapped) in pairs {
            assert_eq!(color(name), Some(mapped), "{name}");
        }
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
