//! Styles for what the editor draws (PLAN Stage 7c; 2.11: the default
//! theme is configuration).
//!
//! A theme maps *scope names* to [`Style`]s. The names a renderer uses are
//! the fields of [`UiStyles`] (`ui.text`, `ui.selection`, …); Stage 9 adds
//! syntax scopes. Two layers: the default config's `[theme]` and the
//! user's; a user entry replaces the default entry of the same name. The
//! layers are resolved into a [`UiStyles`] when either changes, so drawing
//! reads fields, not a map.
//!
//! ```toml
//! [theme]
//! "ui.selection" = { bg = "#264f78" }
//! "ui.statusline" = { fg = "black", bg = "white", modifiers = ["bold"] }
//! "ui.search" = "underline"          # modifiers alone
//! ```

use std::collections::BTreeMap;
use std::fmt;

/// A terminal color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Color {
    /// The terminal's own foreground or background.
    #[default]
    Reset,
    /// One of the 16 ANSI colors (0–7 normal, 8–15 bright).
    Ansi(u8),
    /// The 256-color palette.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

const ANSI_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright-black",
    "bright-red",
    "bright-green",
    "bright-yellow",
    "bright-blue",
    "bright-magenta",
    "bright-cyan",
    "bright-white",
];

impl Color {
    /// `default`, an ANSI name (`red`, `bright-blue`, also `gray` for
    /// bright black), a palette index `0`–`255`, or `#rrggbb`.
    pub fn parse(s: &str) -> Result<Color, String> {
        let s = s.trim();
        if s == "default" || s == "reset" {
            return Ok(Color::Reset);
        }
        if s == "gray" || s == "grey" {
            return Ok(Color::Ansi(8));
        }
        if let Some(i) = ANSI_NAMES.iter().position(|n| *n == s) {
            return Ok(Color::Ansi(i as u8));
        }
        if let Some(hex) = s.strip_prefix('#') {
            if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
                return Ok(Color::Rgb(byte(0), byte(2), byte(4)));
            }
            return Err(format!("bad color {s:?} (expected #rrggbb)"));
        }
        s.parse::<u8>()
            .map(Color::Indexed)
            .map_err(|_| format!("unknown color {s:?}"))
    }
}

/// Text attributes, as a set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Attributes(u8);

impl Attributes {
    pub const NONE: Attributes = Attributes(0);
    pub const BOLD: Attributes = Attributes(1);
    pub const DIM: Attributes = Attributes(2);
    pub const ITALIC: Attributes = Attributes(4);
    pub const UNDERLINE: Attributes = Attributes(8);
    pub const REVERSE: Attributes = Attributes(16);
    pub const STRIKE: Attributes = Attributes(32);

    const NAMES: [(&'static str, Attributes); 6] = [
        ("bold", Attributes::BOLD),
        ("dim", Attributes::DIM),
        ("italic", Attributes::ITALIC),
        ("underline", Attributes::UNDERLINE),
        ("reverse", Attributes::REVERSE),
        ("strike", Attributes::STRIKE),
    ];

    pub fn contains(self, other: Attributes) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// `bold`, `dim`, `italic`, `underline`, `reverse`, or `strike`.
    pub fn parse(s: &str) -> Result<Attributes, String> {
        Attributes::NAMES
            .iter()
            .find(|(n, _)| *n == s.trim())
            .map(|&(_, a)| a)
            .ok_or_else(|| format!("unknown modifier {s:?}"))
    }
}

impl std::ops::BitOr for Attributes {
    type Output = Attributes;
    fn bitor(self, rhs: Attributes) -> Attributes {
        Attributes(self.0 | rhs.0)
    }
}

/// Foreground, background, and attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub attributes: Attributes,
}

impl Style {
    /// `over` drawn on top of `self`: its colors where it sets them, and
    /// both sets of attributes.
    pub fn patch(self, over: Style) -> Style {
        Style {
            fg: if over.fg == Color::Reset {
                self.fg
            } else {
                over.fg
            },
            bg: if over.bg == Color::Reset {
                self.bg
            } else {
                over.bg
            },
            attributes: self.attributes | over.attributes,
        }
    }

    /// A style from config: a table `{ fg, bg, modifiers = [...] }`, or a
    /// string of space-separated modifiers.
    pub(crate) fn from_toml(value: &toml::Value) -> Result<Style, String> {
        let mut style = Style::default();
        match value {
            toml::Value::String(s) => {
                for word in s.split_whitespace() {
                    style.attributes = style.attributes | Attributes::parse(word)?;
                }
            }
            toml::Value::Table(t) => {
                for (key, v) in t {
                    match (key.as_str(), v) {
                        ("fg", toml::Value::String(c)) => style.fg = Color::parse(c)?,
                        ("bg", toml::Value::String(c)) => style.bg = Color::parse(c)?,
                        ("modifiers", toml::Value::Array(list)) => {
                            for m in list {
                                let toml::Value::String(m) = m else {
                                    return Err("modifiers are strings".into());
                                };
                                style.attributes = style.attributes | Attributes::parse(m)?;
                            }
                        }
                        (key, _) => {
                            return Err(format!(
                                "unexpected {key:?} (expected fg, bg, or modifiers)"
                            ));
                        }
                    }
                }
            }
            other => {
                return Err(format!(
                    "expected a table or a string, got {}",
                    other.type_str()
                ));
            }
        }
        Ok(style)
    }
}

/// The styles a renderer draws with, resolved from the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct UiStyles {
    /// Document text (`ui.text`).
    pub text: Style,
    /// Selected text (`ui.selection`), and the primary selection's
    /// (`ui.selection.primary`, else `ui.selection`).
    pub selection: Style,
    pub primary_selection: Style,
    /// A secondary cursor (`ui.cursor`).
    pub cursor: Style,
    /// Search matches (`ui.search`).
    pub search: Style,
    /// The status line (`ui.statusline`).
    pub statusline: Style,
    /// The prompt line (`ui.prompt`).
    pub prompt: Style,
    /// Lines between split views (`ui.separator`).
    pub separator: Style,
}

/// The theme: the default and user layers, resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Theme {
    defaults: BTreeMap<String, Style>,
    user: BTreeMap<String, Style>,
    ui: UiStyles,
}

/// The scope names [`UiStyles`] reads.
pub const UI_SCOPES: [&str; 8] = [
    "ui.text",
    "ui.selection",
    "ui.selection.primary",
    "ui.cursor",
    "ui.search",
    "ui.statusline",
    "ui.prompt",
    "ui.separator",
];

impl Theme {
    /// The resolved styles.
    pub fn ui(&self) -> &UiStyles {
        &self.ui
    }

    /// The style of `scope`: the user's entry, else the default's.
    pub fn get(&self, scope: &str) -> Option<Style> {
        self.user
            .get(scope)
            .or_else(|| self.defaults.get(scope))
            .copied()
    }

    /// Replaces the default layer (`user`: the user layer) as a whole.
    pub(crate) fn set_layer(&mut self, user: bool, styles: BTreeMap<String, Style>) {
        if user {
            self.user = styles;
        } else {
            self.defaults = styles;
        }
        self.resolve();
    }

    fn resolve(&mut self) {
        let get = |name| self.get(name).unwrap_or_default();
        let selection = get("ui.selection");
        self.ui = UiStyles {
            text: get("ui.text"),
            selection,
            primary_selection: self.get("ui.selection.primary").unwrap_or(selection),
            cursor: get("ui.cursor"),
            search: get("ui.search"),
            statusline: get("ui.statusline"),
            prompt: get("ui.prompt"),
            separator: get("ui.separator"),
        };
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Color::Reset => f.write_str("default"),
            Color::Ansi(i) => f.write_str(ANSI_NAMES[usize::from(*i) % 16]),
            Color::Indexed(i) => write!(f, "{i}"),
            Color::Rgb(r, g, b) => write!(f, "#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_parse() {
        assert_eq!(Color::parse("default"), Ok(Color::Reset));
        assert_eq!(Color::parse("red"), Ok(Color::Ansi(1)));
        assert_eq!(Color::parse("bright-blue"), Ok(Color::Ansi(12)));
        assert_eq!(Color::parse("gray"), Ok(Color::Ansi(8)));
        assert_eq!(Color::parse("208"), Ok(Color::Indexed(208)));
        assert_eq!(Color::parse("#264F78"), Ok(Color::Rgb(0x26, 0x4f, 0x78)));
        for bad in ["#12345", "#gggggg", "256", "chartreuse", ""] {
            assert!(Color::parse(bad).is_err(), "{bad:?}");
        }
        for c in [
            Color::Reset,
            Color::Ansi(9),
            Color::Indexed(7),
            Color::Rgb(1, 2, 3),
        ] {
            assert_eq!(Color::parse(&c.to_string()), Ok(c));
        }
    }

    #[test]
    fn styles_parse_and_patch() {
        let v: toml::Value = toml::from_str::<toml::Table>(
            "a = { fg = \"red\", bg = \"#000000\", modifiers = [\"bold\", \"underline\"] }\nb = \"reverse dim\"",
        )
        .unwrap()
        .into();
        let a = Style::from_toml(&v["a"]).unwrap();
        assert_eq!(a.fg, Color::Ansi(1));
        assert_eq!(a.bg, Color::Rgb(0, 0, 0));
        assert!(
            a.attributes
                .contains(Attributes::BOLD | Attributes::UNDERLINE)
        );
        let b = Style::from_toml(&v["b"]).unwrap();
        assert!(b.attributes.contains(Attributes::REVERSE | Attributes::DIM));
        let p = a.patch(Style {
            fg: Color::Reset,
            bg: Color::Ansi(4),
            attributes: Attributes::ITALIC,
        });
        assert_eq!((p.fg, p.bg), (Color::Ansi(1), Color::Ansi(4)));
        assert!(p.attributes.contains(Attributes::BOLD | Attributes::ITALIC));
        for bad in ["blink", "{ fg = 3 }", "{ color = \"red\" }", "[1]"] {
            let v: toml::Value = toml::from_str::<toml::Table>(&format!(
                "x = {}",
                if bad.starts_with('{') || bad.starts_with('[') {
                    bad.to_owned()
                } else {
                    format!("{bad:?}")
                }
            ))
            .unwrap()
            .into();
            assert!(Style::from_toml(&v["x"]).is_err(), "{bad}");
        }
    }

    #[test]
    fn user_entries_replace_defaults_and_primary_falls_back() {
        let mut theme = Theme::default();
        let reverse = Style {
            attributes: Attributes::REVERSE,
            ..Style::default()
        };
        theme.set_layer(false, [("ui.selection".to_owned(), reverse)].into());
        assert_eq!(theme.ui().primary_selection, reverse);
        let blue = Style {
            bg: Color::Ansi(4),
            ..Style::default()
        };
        theme.set_layer(true, [("ui.selection".to_owned(), blue)].into());
        assert_eq!(theme.ui().selection, blue);
        assert_eq!(theme.ui().primary_selection, blue);
        theme.set_layer(true, BTreeMap::new());
        assert_eq!(theme.ui().selection, reverse);
    }
}
