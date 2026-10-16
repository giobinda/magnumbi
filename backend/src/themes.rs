//! Themes: named, ID'd looks for the whole UI and its charts.
//!
//! A dashboard can pin a theme by ID (`theme: midnight`) so it looks the same for every viewer.
//! Built-in themes are FROZEN: an ID always renders the same way. To change a look, add a theme
//! with a new ID in project/themes/*.yaml (built-in IDs are reserved).

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

static HEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#[0-9a-fA-F]{6}$").unwrap());
static THEME_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9-]{1,40}$").unwrap());
const SYSTEM_FONT: &str = r#"Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif"#;

// validated categorical palettes
const REF_LIGHT: [&str; 8] = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#4a3aa7", "#e34948"];
const REF_DARK: [&str; 8] = ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#008300", "#9085e9", "#e66767"];
// every slot >= 3:1 on white and >= 8 adjacent CVD ΔE
const HIGH_CONTRAST: [&str; 7] = ["#256abf", "#d24e1c", "#0e8a5f", "#5b4bc4", "#a87400", "#c94a7a", "#007000"];

// Aloha: an aloha shirt on a Honolulu afternoon (lagoon, hibiscus, pineapple, ocean, plumeria,
// palm, sunset, orchid); every slot >= 3.9:1 on its surface
const ALOHA: [&str; 8] = ["#0a8a9e", "#d62839", "#a87800", "#1f5fa8", "#c9327d", "#2b8a3e", "#d4580a", "#7a3fa0"];
// Aloha night: the same, after sunset; every slot >= 5.7:1 on its surface
const ALOHA_NIGHT: [&str; 8] = ["#2ec4b6", "#ff6b6b", "#ffb703", "#5aa9ff", "#ff70a6", "#8ac926", "#fb8b24", "#b39cff"];

pub const DEFAULT_LIGHT: &str = "aloha";
pub const DEFAULT_DARK: &str = "aloha-night";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeColors {
    pub bg: String,
    pub surface: String,
    pub surface_2: String,
    pub border: String,
    pub text: String,
    pub text_muted: String,
    pub accent: String,
    pub accent_soft: String,
    pub good: String,
    pub bad: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub id: String,
    pub label: String,
    /// light | dark
    pub mode: String,
    pub colors: ThemeColors,
    /// categorical series colours, in order
    pub palette: Vec<String>,
    #[serde(default = "system_font")]
    pub font: String,
    #[serde(default = "default_radius")]
    pub radius: u8,
    #[serde(default)]
    pub builtin: bool,
}

fn system_font() -> String {
    SYSTEM_FONT.into()
}
fn default_radius() -> u8 {
    10
}

impl Theme {
    /// Checks for project themes (colours and fonts are injected into CSS).
    pub fn validate(&mut self) -> Result<(), String> {
        if !THEME_ID.is_match(&self.id) {
            return Err("theme ids are lowercase letters, digits and dashes".into());
        }
        if self.mode != "light" && self.mode != "dark" {
            return Err("mode: light or dark".into());
        }
        let c = &mut self.colors;
        for v in [
            &mut c.bg,
            &mut c.surface,
            &mut c.surface_2,
            &mut c.border,
            &mut c.text,
            &mut c.text_muted,
            &mut c.accent,
            &mut c.accent_soft,
            &mut c.good,
            &mut c.bad,
        ] {
            if !HEX.is_match(v) {
                return Err(format!("{v:?} is not a #rrggbb colour"));
            }
            *v = v.to_lowercase();
        }
        if !(3..=10).contains(&self.palette.len()) || self.palette.iter().any(|p| !HEX.is_match(p)) {
            return Err("palette: 3 to 10 #rrggbb colours".into());
        }
        self.palette = self.palette.iter().map(|p| p.to_lowercase()).collect();
        if self.font.contains([';', '{', '}', '<', '>']) || self.font.contains("url(") {
            return Err("font must be a plain font-family list".into());
        }
        if self.radius > 20 {
            return Err("radius: 0 to 20".into());
        }
        self.builtin = false;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn t(id: &str, label: &str, mode: &str, palette: &[&str], radius: u8, c: [&str; 10]) -> Theme {
    Theme {
        id: id.into(),
        label: label.into(),
        mode: mode.into(),
        palette: palette.iter().map(|s| s.to_string()).collect(),
        radius,
        font: SYSTEM_FONT.into(),
        builtin: true,
        colors: ThemeColors {
            bg: c[0].into(),
            surface: c[1].into(),
            surface_2: c[2].into(),
            border: c[3].into(),
            text: c[4].into(),
            text_muted: c[5].into(),
            accent: c[6].into(),
            accent_soft: c[7].into(),
            good: c[8].into(),
            bad: c[9].into(),
        },
    }
}

pub fn builtin() -> Vec<Theme> {
    vec![
        // sand, a red Ferrari 308 for the accent, an aloha shirt for the charts
        t(
            DEFAULT_LIGHT,
            "Aloha",
            "light",
            &ALOHA,
            12,
            [
                "#fff6e9", "#fffdf8", "#fdeed6", "#efd9b8", "#2a1a12", "#6e4f3a", "#c8102e", "#fde3e1", "#13795b",
                "#b3261e",
            ],
        ),
        // a Pacific night: coral and turquoise
        t(
            DEFAULT_DARK,
            "Aloha night",
            "dark",
            &ALOHA_NIGHT,
            12,
            [
                "#0b1a2b", "#11243a", "#18304b", "#244463", "#f7efe2", "#b9c6d3", "#ff6b6b", "#3d2232", "#4cd38a",
                "#ff8a80",
            ],
        ),
        t(
            "magnum-light",
            "Magnum light",
            "light",
            &REF_LIGHT,
            10,
            [
                "#f6f7f9", "#ffffff", "#f1f3f6", "#e3e6eb", "#1d2330", "#5c6676", "#4f6bed", "#e9edff", "#006300",
                "#c4314b",
            ],
        ),
        t(
            "magnum-dark",
            "Magnum dark",
            "dark",
            &REF_DARK,
            10,
            [
                "#0f1218", "#171b23", "#1f2430", "#2b3140", "#e6e9ef", "#a3adbd", "#7b8ff5", "#232a4a", "#0ca30c",
                "#f2788c",
            ],
        ),
        t(
            "paper",
            "Paper",
            "light",
            &REF_LIGHT,
            4,
            [
                "#f4f1ea", "#fbf9f4", "#f0ebe0", "#e2dccd", "#2b2620", "#6a6256", "#b4532a", "#f3e3d8", "#006300",
                "#b3261e",
            ],
        ),
        t(
            "slate",
            "Slate",
            "light",
            &REF_LIGHT,
            6,
            [
                "#eef1f4", "#ffffff", "#f3f5f7", "#d9dee4", "#1f2933", "#56616e", "#0f766e", "#dff3f0", "#006300",
                "#c4314b",
            ],
        ),
        t(
            "midnight",
            "Midnight",
            "dark",
            &REF_DARK,
            12,
            [
                "#0b1020", "#121a2e", "#1a2540", "#26324f", "#e8ecf6", "#a2adc6", "#7aa2ff", "#1d2a4d", "#0ca30c",
                "#f2788c",
            ],
        ),
        t(
            "contrast",
            "High contrast",
            "light",
            &HIGH_CONTRAST,
            4,
            [
                "#ffffff", "#ffffff", "#efefef", "#5c5c5c", "#000000", "#333333", "#1c4fbf", "#e3ebfa", "#006300",
                "#b3261e",
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG contrast ratio of two #rrggbb colours.
    fn contrast(a: &str, b: &str) -> f64 {
        let lum = |h: &str| {
            let c: Vec<f64> = [1, 3, 5]
                .iter()
                .map(|i| f64::from(u8::from_str_radix(&h[*i..*i + 2], 16).unwrap()) / 255.0)
                .map(|x| if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) })
                .collect();
            0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn the_defaults_are_aloha_and_readable() {
        let all = builtin();
        for (id, mode) in [(DEFAULT_LIGHT, "light"), (DEFAULT_DARK, "dark")] {
            let t = all.iter().find(|t| t.id == id).unwrap_or_else(|| panic!("{id} is built in"));
            assert!(t.id.starts_with("aloha") && t.mode == mode, "{id}");
            let c = &t.colors;
            for (what, fg, bg, min) in [
                ("text", &c.text, &c.bg, 4.5),
                ("text", &c.text, &c.surface, 4.5),
                ("muted text", &c.text_muted, &c.surface, 4.5),
                ("muted text", &c.text_muted, &c.bg, 4.5),
                ("links", &c.accent, &c.surface, 4.5),
                ("good", &c.good, &c.surface, 3.0),
                ("bad", &c.bad, &c.surface, 3.0),
            ] {
                assert!(contrast(fg, bg) >= min, "{id}: {what} {fg} on {bg} is {:.1}:1", contrast(fg, bg));
            }
            assert_eq!(t.palette.len(), 8);
            for p in &t.palette {
                assert!(contrast(p, &c.surface) >= 3.0, "{id}: chart colour {p} is {:.1}:1", contrast(p, &c.surface));
            }
        }
        // the earlier defaults stay as they were (built-in themes are frozen)
        assert!(all.iter().any(|t| t.id == "magnum-light" && t.colors.accent == "#4f6bed"));
    }
}
