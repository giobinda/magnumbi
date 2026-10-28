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
// green, amber and pink deepened (same hue) to reach 3:1 on white once Magnum light became the default
const REF_LIGHT: [&str; 8] = ["#2a78d6", "#eb6834", "#1aa674", "#c48500", "#e56a98", "#008300", "#4a3aa7", "#e34948"];
const REF_DARK: [&str; 8] = ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#008300", "#9085e9", "#e66767"];
// every slot >= 3:1 on white and >= 8 adjacent CVD ΔE
const HIGH_CONTRAST: [&str; 7] = ["#256abf", "#d24e1c", "#0e8a5f", "#5b4bc4", "#a87400", "#c94a7a", "#007000"];

// Aloha: an aloha shirt on a Honolulu afternoon (lagoon, hibiscus, pineapple, ocean, plumeria,
// palm, sunset, orchid); every slot >= 3.9:1 on its surface
const ALOHA: [&str; 8] = ["#0a8a9e", "#d62839", "#a87800", "#1f5fa8", "#c9327d", "#2b8a3e", "#d4580a", "#7a3fa0"];
// Aloha night: the same, after sunset; every slot >= 5.7:1 on its surface
const ALOHA_NIGHT: [&str; 8] = ["#2ec4b6", "#ff6b6b", "#ffb703", "#5aa9ff", "#ff70a6", "#8ac926", "#fb8b24", "#b39cff"];

/// Magnum, calm and neutral, by default (a tester found Aloha heavy); Aloha stays, to choose
pub const DEFAULT_LIGHT: &str = "magnum-light";
pub const DEFAULT_DARK: &str = "magnum-dark";

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

/// An organisation's own themes (kept in its ScramDB) and its default light and dark theme.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OrgThemes {
    #[serde(default)]
    pub themes: Vec<Theme>,
    /// a theme id (the organisation's or a project one); none: the platform's default
    #[serde(default)]
    pub default_light: Option<String>,
    #[serde(default)]
    pub default_dark: Option<String>,
}

pub const MAX_ORG_THEMES: usize = 20;

impl OrgThemes {
    /// Checks (and normalises) the themes against the project's (`project`: built-in and project
    /// files, whose ids are reserved).
    pub fn validate(&mut self, project: &[Theme]) -> Result<(), String> {
        if self.themes.len() > MAX_ORG_THEMES {
            return Err(format!("at most {MAX_ORG_THEMES} themes"));
        }
        let mut seen: Vec<String> = vec![];
        for t in &mut self.themes {
            t.validate().map_err(|e| format!("theme {}: {e}", t.id))?;
            if project.iter().any(|p| p.id == t.id) {
                return Err(format!("theme id {:?} is built in or a project theme: pick another", t.id));
            }
            if seen.contains(&t.id) {
                return Err(format!("theme {:?} is there twice", t.id));
            }
            if t.label.trim().is_empty() || t.label.len() > 60 {
                return Err(format!("theme {}: a label up to 60 characters", t.id));
            }
            seen.push(t.id.clone());
        }
        for (default, mode) in [(&self.default_light, "light"), (&self.default_dark, "dark")] {
            let Some(id) = default else { continue };
            let t = self.themes.iter().chain(project).find(|t| &t.id == id);
            let t = t.ok_or_else(|| format!("default {mode} theme {id:?} doesn't exist"))?;
            if t.mode != mode {
                return Err(format!("{id:?} is a {} theme, not a {mode} one", t.mode));
            }
        }
        Ok(())
    }
}

/// Black or white, whichever reads better on `hex` (text on the accent colour).
fn on_colour(hex: &str) -> &'static str {
    let lum = {
        let c: Vec<f64> = [1, 3, 5]
            .iter()
            .map(|i| f64::from(u8::from_str_radix(hex.get(*i..*i + 2).unwrap_or("00"), 16).unwrap_or(0)) / 255.0)
            .map(|x| if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) })
            .collect();
        0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
    };
    if 1.05 / (lum + 0.05) >= (lum + 0.05) / 0.05 { "#ffffff" } else { "#000000" }
}

/// A theme as CSS design tokens (--mbi-*) plus helper classes in Magnum BI's style, for a
/// company's own site or design system; `from_css` reads the tokens back.
pub fn to_css(t: &Theme) -> String {
    let c = &t.colors;
    let label = t.label.replace("*/", "* /");
    let mut out = format!(
        "/* Magnum BI theme: {label} ({}), {}.\n   Design tokens for your own site or design system, e.g. color: var(--mbi-accent).\n   \
         Edit the values and paste this back into Magnum BI (Themes > Import CSS) to update the theme. */\n:root {{\n",
        t.id, t.mode
    );
    let tokens: [(&str, &str); 12] = [
        ("mode", &t.mode),
        ("bg", &c.bg),
        ("surface", &c.surface),
        ("surface-2", &c.surface_2),
        ("border", &c.border),
        ("text", &c.text),
        ("text-muted", &c.text_muted),
        ("accent", &c.accent),
        ("accent-soft", &c.accent_soft),
        ("on-accent", on_colour(&c.accent)),
        ("good", &c.good),
        ("bad", &c.bad),
    ];
    for (k, v) in tokens {
        out.push_str(&format!("  --mbi-{k}: {v};\n"));
    }
    out.push_str(&format!("  --mbi-radius: {}px;\n  --mbi-font: {};\n", t.radius, t.font));
    for (i, p) in t.palette.iter().enumerate() {
        out.push_str(&format!("  --mbi-chart-{}: {p};\n", i + 1));
    }
    out.push_str(
        "}\n\n/* Helper classes in Magnum BI's style */\n\
         .mbi-page { background: var(--mbi-bg); color: var(--mbi-text); font-family: var(--mbi-font); }\n\
         .mbi-card { background: var(--mbi-surface); border: 1px solid var(--mbi-border); border-radius: var(--mbi-radius); padding: 14px 16px; }\n\
         .mbi-muted { color: var(--mbi-text-muted); }\n\
         .mbi-link { color: var(--mbi-accent); }\n\
         .mbi-button { background: var(--mbi-surface); color: var(--mbi-text); border: 1px solid var(--mbi-border); border-radius: var(--mbi-radius); padding: 6px 11px; font: inherit; }\n\
         .mbi-button-primary { background: var(--mbi-accent); color: var(--mbi-on-accent); border-color: var(--mbi-accent); }\n\
         .mbi-badge { background: var(--mbi-accent-soft); color: var(--mbi-accent); border-radius: 999px; padding: 2px 8px; }\n\
         .mbi-kpi { font-size: 28px; font-weight: 700; color: var(--mbi-text); }\n\
         .mbi-up { color: var(--mbi-good); }\n\
         .mbi-down { color: var(--mbi-bad); }\n",
    );
    out
}

static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"--mbi-([a-z0-9-]+)\s*:\s*([^;}]*)").unwrap());

/// A theme from CSS design tokens (as `to_css` writes them): tokens not given come from `base`;
/// everything else in the CSS is ignored. The result is validated like any theme.
pub fn from_css(css: &str, base: &Theme) -> Result<Theme, String> {
    let mut t = base.clone();
    let mut chart: Vec<(u32, String)> = vec![];
    let mut found = 0;
    for cap in TOKEN.captures_iter(css) {
        let (name, value) = (&cap[1], cap[2].trim().to_string());
        found += 1;
        let c = &mut t.colors;
        match name {
            "mode" => t.mode = value,
            "bg" => c.bg = value,
            "surface" => c.surface = value,
            "surface-2" => c.surface_2 = value,
            "border" => c.border = value,
            "text" => c.text = value,
            "text-muted" => c.text_muted = value,
            "accent" => c.accent = value,
            "accent-soft" => c.accent_soft = value,
            "good" => c.good = value,
            "bad" => c.bad = value,
            "font" => t.font = value,
            "radius" => {
                t.radius =
                    value.trim_end_matches("px").trim().parse().map_err(|_| format!("--mbi-radius: {value:?}"))?;
            }
            n if n.starts_with("chart-") => {
                let i = n["chart-".len()..].parse().map_err(|_| format!("--mbi-{n}: chart-1, chart-2, ..."))?;
                chart.push((i, value));
            }
            _ => found -= 1, // on-accent (derived) and anything else
        }
    }
    if found == 0 {
        return Err("no Magnum BI tokens (--mbi-...) found".into());
    }
    if !chart.is_empty() {
        chart.sort_by_key(|(i, _)| *i);
        t.palette = chart.into_iter().map(|(_, v)| v).collect();
    }
    t.validate()?;
    Ok(t)
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
            "aloha",
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
            "aloha-night",
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

    fn acme() -> Theme {
        let mut t = builtin().into_iter().find(|t| t.id == "slate").unwrap();
        t.id = "acme".into();
        t.label = "ACME */ blue".into();
        t.colors.accent = "#0055a5".into();
        t.font = "\"Acme Sans\", sans-serif".into();
        t.radius = 8;
        t.builtin = false;
        t
    }

    #[test]
    fn css_tokens_out_and_back() {
        let css = to_css(&acme());
        assert!(css.contains("--mbi-accent: #0055a5;"), "{css}");
        assert!(css.contains("--mbi-radius: 8px;") && css.contains("--mbi-font: \"Acme Sans\", sans-serif;"), "{css}");
        assert!(css.contains("--mbi-chart-1: ") && css.contains("--mbi-on-accent: #ffffff;"), "{css}");
        assert!(css.contains(".mbi-card") && css.contains("var(--mbi-surface)"), "helper classes for the company site");
        assert!(!css.contains("ACME */"), "a label can't end the comment early");

        // the design team edits a token and pastes it back
        let edited = css
            .replace("--mbi-accent: #0055a5;", "--mbi-accent: #FF8A00;")
            .replace("--mbi-radius: 8px;", "--mbi-radius: 4px;");
        let back = from_css(&edited, &builtin()[0]).unwrap();
        assert_eq!(back.colors.accent, "#ff8a00");
        assert_eq!(back.radius, 4);
        assert_eq!((back.mode.as_str(), back.palette.len()), ("light", acme().palette.len()));
        assert_eq!(back.colors.surface, acme().colors.surface);

        // only some tokens: the rest from the starting theme
        let base = &builtin()[0];
        let partial = from_css(
            ":root { --mbi-accent: #123456; --mbi-chart-1: #111111; --mbi-chart-2: #222222; --mbi-chart-3: #333333; }",
            base,
        )
        .unwrap();
        assert_eq!((partial.colors.accent.as_str(), partial.colors.bg.as_str()), ("#123456", base.colors.bg.as_str()));
        assert_eq!(partial.palette, ["#111111", "#222222", "#333333"]);
        // anything that isn't a token value is refused
        for bad in [
            ":root { --mbi-accent: red; }",
            ":root { --mbi-accent: #12345; }",
            ":root { --mbi-font: url(https://evil.example/f.woff); }",
            ":root { --mbi-radius: 99px; }",
            ":root { --mbi-mode: sepia; }",
        ] {
            assert!(from_css(bad, base).is_err(), "{bad}");
        }
        assert!(from_css("no tokens here", base).unwrap_err().contains("--mbi-"));
        // the rest of the CSS (our helper classes, their own rules) is ignored, never used
        let with_rules = format!("{css}\nbody {{ background: url(https://elsewhere.example/x.png) }}");
        assert_eq!(from_css(&with_rules, base).unwrap().colors.accent, "#0055a5");
    }

    #[test]
    fn organisations_themes_are_checked() {
        let project = builtin();
        let doc = |themes: Vec<Theme>, light: Option<&str>, dark: Option<&str>| OrgThemes {
            themes,
            default_light: light.map(String::from),
            default_dark: dark.map(String::from),
        };
        let mut ok = doc(vec![acme()], Some("acme"), Some("aloha-night"));
        ok.validate(&project).unwrap();
        let mut dark_acme = acme();
        dark_acme.id = "acme-dark".into();
        for (mut bad, why) in [
            (doc(vec![acme(), acme()], None, None), "twice"),
            (doc(vec![Theme { id: "aloha".into(), ..acme() }], None, None), "built in"),
            (doc(vec![acme()], Some("nope"), None), "nope"),
            (doc(vec![acme()], None, Some("acme")), "dark"),
            (
                doc(vec![Theme { colors: ThemeColors { bg: "red".into(), ..acme().colors }, ..acme() }], None, None),
                "colour",
            ),
            (doc(vec![dark_acme; 21], None, None), "20"),
        ] {
            let e = bad.validate(&project).unwrap_err();
            assert!(e.contains(why), "{why}: {e}");
        }
    }

    #[test]
    fn the_defaults_are_magnum_and_readable() {
        let all = builtin();
        // calmer than Aloha (a tester found it heavy); Aloha stays, to choose
        assert_eq!((DEFAULT_LIGHT, DEFAULT_DARK), ("magnum-light", "magnum-dark"));
        assert!(all.iter().any(|t| t.id == "aloha") && all.iter().any(|t| t.id == "aloha-night"));
        for (id, mode) in [(DEFAULT_LIGHT, "light"), (DEFAULT_DARK, "dark")] {
            let t = all.iter().find(|t| t.id == id).unwrap_or_else(|| panic!("{id} is built in"));
            assert!(t.mode == mode, "{id}");
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
