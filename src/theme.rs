//! User-editable UI colors, read from `%APPDATA%\Veyra\theme.conf`.
//!
//! A plain `key = value` file; lines starting with `#` or `;` are comments.
//! See `DEFAULT_FILE` for every option. The file is created on first run (an older
//! `theme.json` is converted) and reloaded automatically when it changes.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::gfx::C;

#[derive(Clone, PartialEq, Debug)]
pub struct Theme {
    /// Background stops, (position from the bottom 0..1, color). One stop = solid.
    pub bg: Vec<(f32, C)>,
    /// Color that replaces white in text, icons and translucent surfaces.
    pub fg: C,
    /// How much to firm up faded (secondary) text and icons: 0 = as designed, 1 = fully solid.
    pub text_contrast: f32,
    /// Regular buttons (None = translucent, follows `fg`).
    pub button: Option<C>,
    pub button_hover: Option<C>,
    pub button_text: Option<C>,
    /// Main filled buttons (Start focus, + Bottle, add task, Deselect). None = each feature's color.
    pub button_accent: Option<C>,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            bg: vec![(0.0, C(0.0, 0.0, 0.0, 1.0))],
            fg: C(1.0, 1.0, 1.0, 1.0),
            text_contrast: 0.0,
            button: None,
            button_hover: None,
            button_text: None,
            button_accent: None,
        }
    }
}

const DEFAULT_FILE: &str = "\
# Veyra UI colors
# Save this file and Veyra updates within a second.
# Colors: #RGB, #RRGGBB or #RRGGBBAA (last two digits = transparency).
# Lines starting with # are comments.

# --- Background -------------------------------------------------
# solid    = one color (uses \"solid\")
# gradient = several colors (uses \"gradient\")
mode = solid
solid = #000000

# Gradient colors from the BOTTOM of the island (0) to the TOP (1):
#   color position, color position, ...
# Positions are optional; without them the colors are spread evenly.
# Example (pink, light at the top):
gradient = #F07A8A 0, #F2B5BC 0.45, #F2DCE0 0.75

# --- Text -------------------------------------------------------
# auto = white on dark backgrounds, dark on light ones
text = auto

# How strong the faded text is (hints, labels, inactive icons):
#   auto   = normal on dark backgrounds, stronger on light ones
#   normal = as designed
#   high   = clearly stronger
#   max    = all text fully solid
#   or a number from 0 (normal) to 100 (max)
text_contrast = auto

# --- Buttons ----------------------------------------------------
# auto = translucent, follows the text color
button = auto
button_hover = auto
button_text = auto

# Main colored buttons (Start focus, + Bottle, add task, Deselect)
# auto = keep each feature's own color
button_accent = auto
";

fn dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Veyra"))
}

pub fn path() -> Option<PathBuf> {
    Some(dir()?.join("theme.conf"))
}

/// Writes the default file if there is none yet (converting an older theme.json), and returns its path.
pub fn ensure_file() -> Option<PathBuf> {
    let p = path()?;
    if !p.exists() {
        let _ = std::fs::create_dir_all(dir()?);
        let old = dir()?.join("theme.json");
        let text = std::fs::read_to_string(&old).ok().and_then(|s| from_json(&s)).unwrap_or_else(|| DEFAULT_FILE.to_string());
        if std::fs::write(&p, text).is_ok() && old.exists() {
            let _ = std::fs::rename(&old, dir()?.join("theme.json.old"));
        }
    }
    Some(p)
}

pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(path()?).ok()?.modified().ok()
}

/// Loads the theme; anything missing or invalid falls back to the default.
pub fn load() -> Theme {
    path().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| parse(&s)).unwrap_or_default()
}

/// `key = value` pairs (keys lower-cased), comments and blank lines skipped.
fn pairs(s: &str) -> Vec<(String, String)> {
    s.trim_start_matches('\u{feff}')
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') || l.starts_with(';') {
                return None;
            }
            let (k, v) = l.split_once('=').or_else(|| l.split_once(':'))?;
            Some((k.trim().to_ascii_lowercase(), v.trim().trim_matches('"').to_string()))
        })
        .collect()
}

/// `None` for "auto" / empty / invalid.
fn opt_color(v: Option<&String>) -> Option<C> {
    let v = v?.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("auto") {
        return None;
    }
    color(v)
}

pub fn parse(s: &str) -> Theme {
    let kv = pairs(s);
    // the last occurrence of a key wins
    let get = |k: &str| kv.iter().rev().find(|(key, _)| key == k).map(|(_, v)| v);
    let mut bg = Vec::new();
    if get("mode").map(|m| m.eq_ignore_ascii_case("gradient")).unwrap_or(false) {
        bg = parse_gradient(get("gradient").map(|s| s.as_str()).unwrap_or(""));
    }
    if bg.is_empty() {
        bg.push((0.0, opt_color(get("solid")).unwrap_or(C(0.0, 0.0, 0.0, 1.0))));
    }
    let fg = opt_color(get("text")).unwrap_or_else(|| auto_fg(&bg));
    let light_bg = luma(bg.iter().map(|s| s.1)) > 0.6;
    let text_contrast = match get("text_contrast").map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("normal") | Some("low") => 0.0,
        Some("high") => 0.6,
        Some("max") => 1.0,
        Some(n) if n.trim_end_matches('%').parse::<f32>().is_ok() => (n.trim_end_matches('%').parse::<f32>().unwrap() / 100.0).clamp(0.0, 1.0),
        // auto (or anything else)
        _ => {
            if light_bg {
                0.45
            } else {
                0.0
            }
        }
    };
    Theme {
        bg,
        fg,
        text_contrast,
        button: opt_color(get("button")),
        button_hover: opt_color(get("button_hover")),
        button_text: opt_color(get("button_text")),
        button_accent: opt_color(get("button_accent")),
    }
}

/// "#F07A8A 0, #F2DCE0 0.75" → sorted stops; positions default to even spacing.
fn parse_gradient(s: &str) -> Vec<(f32, C)> {
    let items: Vec<&str> = s.split(',').map(str::trim).filter(|x| !x.is_empty()).collect();
    let n = items.len();
    let mut out: Vec<(f32, C)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| {
            let mut parts = it.split_whitespace();
            let c = color(parts.next()?)?;
            let even = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
            let at = parts.next().and_then(|p| p.trim_end_matches('%').parse::<f32>().ok()).map(|p| if p > 1.0 { p / 100.0 } else { p });
            Some((at.unwrap_or(even).clamp(0.0, 1.0), c))
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// Converts the old theme.json into the .conf format (keeps the user's colors).
fn from_json(s: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()?;
    let mut out = DEFAULT_FILE.to_string();
    let mut set = |key: &str, val: String| {
        let prefix = format!("\n{key} = ");
        if let Some(i) = out.find(&prefix) {
            let start = i + prefix.len();
            let end = out[start..].find('\n').map(|e| start + e).unwrap_or(out.len());
            out.replace_range(start..end, &val);
        }
    };
    if let Some(m) = v.get("mode").and_then(|m| m.as_str()) {
        set("mode", m.to_string());
    }
    if let Some(c) = v.get("solid").and_then(|m| m.as_str()) {
        set("solid", c.to_string());
    }
    if let Some(t) = v.get("text").and_then(|m| m.as_str()) {
        set("text", t.to_string());
    }
    if let Some(arr) = v.get("gradient").and_then(|g| g.as_array()) {
        let stops: Vec<String> = arr
            .iter()
            .filter_map(|st| match st {
                serde_json::Value::String(c) => Some(c.clone()),
                o => {
                    let c = o.get("color")?.as_str()?;
                    Some(match o.get("at").and_then(|a| a.as_f64()) {
                        Some(a) => format!("{c} {a}"),
                        None => c.to_string(),
                    })
                }
            })
            .collect();
        if !stops.is_empty() {
            set("gradient", stops.join(", "));
        }
    }
    Some(out)
}

/// White on dark backgrounds, near-black on light ones (by average luminance).
fn auto_fg(bg: &[(f32, C)]) -> C {
    if luma(bg.iter().map(|s| s.1)) > 0.6 {
        C(0.11, 0.11, 0.12, 1.0)
    } else {
        C(1.0, 1.0, 1.0, 1.0)
    }
}

pub fn luma(cs: impl Iterator<Item = C>) -> f32 {
    let (sum, n) = cs.fold((0.0, 0), |(s, n), c| (s + 0.2126 * c.0 + 0.7152 * c.1 + 0.0722 * c.2, n + 1));
    if n == 0 {
        0.0
    } else {
        sum / n as f32
    }
}

/// Parses `#RGB`, `#RRGGBB` or `#RRGGBBAA`. Pure white is nudged a hair off 1.0, because
/// the renderer treats exact white as "the theme's text color".
pub fn color(s: &str) -> Option<C> {
    let h = s.trim().trim_start_matches('#');
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|b| b as f32 / 255.0);
    let nib = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok().map(|b| (b * 17) as f32 / 255.0);
    let c = match h.len() {
        3 => C(nib(0)?, nib(1)?, nib(2)?, 1.0),
        6 => C(byte(0)?, byte(2)?, byte(4)?, 1.0),
        8 => C(byte(0)?, byte(2)?, byte(4)?, byte(6)?),
        _ => return None,
    };
    if c.0 == 1.0 && c.1 == 1.0 && c.2 == 1.0 {
        return Some(C(0.9999, 0.9999, 0.9999, c.3));
    }
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(color("#000"), Some(C(0.0, 0.0, 0.0, 1.0)));
        assert_eq!(color("FF0000"), Some(C(1.0, 0.0, 0.0, 1.0)));
        assert!(color("#fff").unwrap().0 < 1.0);
        assert!(color("#12345").is_none());
        assert!(color("#zzzzzz").is_none());
    }

    #[test]
    fn default_file_is_solid_black() {
        assert_eq!(parse(DEFAULT_FILE), Theme::default());
    }

    #[test]
    fn gradient_bottom_to_top() {
        let t = parse("mode = gradient\ngradient = #F07A8A, #F2DCE0");
        assert_eq!(t.bg.len(), 2);
        assert_eq!((t.bg[0].0, t.bg[1].0), (0.0, 1.0));
        // light pink background: dark text
        assert!(t.fg.0 < 0.5);
        let t = parse("MODE = Gradient\n# comment\ngradient = #000 0.8, #111 10%\ntext = #ff0000");
        assert_eq!(t.bg[0].0, 0.1);
        assert_eq!(t.fg, C(1.0, 0.0, 0.0, 1.0));
    }

    #[test]
    fn contrast() {
        assert_eq!(parse("text_contrast = high").text_contrast, 0.6);
        assert_eq!(parse("text_contrast = 50").text_contrast, 0.5);
        assert_eq!(parse("text_contrast = max").text_contrast, 1.0);
        // auto: normal on black, stronger on a light background
        assert_eq!(parse("").text_contrast, 0.0);
        assert!(parse("solid = #F2DCE0").text_contrast > 0.0);
    }

    #[test]
    fn buttons() {
        let t = parse("button = #333333\nbutton_hover = auto\nbutton_accent = #F07A8A");
        assert_eq!(t.button, color("#333333"));
        assert_eq!(t.button_hover, None);
        assert_eq!(t.button_accent, color("#F07A8A"));
    }

    #[test]
    fn broken_values_fall_back() {
        assert_eq!(parse("nonsense\nmode = gradient\ngradient = nope"), Theme::default());
        assert_eq!(parse("solid = #12"), Theme::default());
    }

    #[test]
    fn converts_old_json() {
        let conf = from_json(r##"{"mode":"gradient","gradient":[{"color":"#F07A8A","at":0.0},{"color":"#F2DCE0","at":0.75}],"text":"auto"}"##).unwrap();
        let t = parse(&conf);
        assert_eq!(t.bg, vec![(0.0, color("#F07A8A").unwrap()), (0.75, color("#F2DCE0").unwrap())]);
    }
}
