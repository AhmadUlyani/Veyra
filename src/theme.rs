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
    /// The same three, used while the island is open (expanded). Equal to the above unless
    /// theme.conf sets `expanded_*`.
    pub bg_open: Vec<(f32, C)>,
    pub fg_open: C,
    pub text_contrast_open: f32,
    /// Regular buttons (None = translucent, follows `fg`).
    pub button: Option<C>,
    pub button_hover: Option<C>,
    pub button_text: Option<C>,
    /// The selected button (open tab). None = a step stronger than `button`.
    pub button_active: Option<C>,
    /// Main filled buttons (Start focus, + Bottle, add task, Deselect). None = each feature's color.
    pub button_accent: Option<C>,
    /// Regular icons (None = the text color).
    pub icon: Option<C>,
    /// Icons of the open tab / switched-on toggles (None = each feature's color).
    pub icon_active: Option<C>,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            bg: vec![(0.0, C(0.0, 0.0, 0.0, 1.0))],
            fg: C(1.0, 1.0, 1.0, 1.0),
            text_contrast: 0.0,
            bg_open: vec![(0.0, C(0.0, 0.0, 0.0, 1.0))],
            fg_open: C(1.0, 1.0, 1.0, 1.0),
            text_contrast_open: 0.0,
            button: None,
            button_hover: None,
            button_text: None,
            button_active: None,
            button_accent: None,
            icon: None,
            icon_active: None,
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

# --- Background while open --------------------------------------
# Colors used while the island is open (clicked). The island fades
# between the two when it opens and closes.
# same     = keep the background above
# solid    = one color (uses \"expanded_solid\")
# gradient = several colors (uses \"expanded_gradient\", same format as above)
# Example (black outside, blue gradient inside):
#   mode = solid
#   solid = #000000
#   expanded_mode = gradient
#   expanded_gradient = #0A1A4A 0, #1E4FD8 0.6, #4A8BFF 1
expanded_mode = same
expanded_solid = #000000
expanded_gradient = #0A1A4A 0, #1E4FD8 0.6, #4A8BFF 1

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

# Text while open: same = as above; auto = picked for the open background;
# or a color. expanded_text_contrast takes the same values as text_contrast
# (plus \"same\").
expanded_text = auto
expanded_text_contrast = auto

# --- Buttons ----------------------------------------------------
# auto = translucent, follows the text color
button = auto
button_hover = auto
button_text = auto

# Selected button (the open tab in the top bar)
# auto = a little stronger than \"button\" (button_hover does not change it)
button_active = auto

# Main colored buttons (Start focus, + Bottle, add task, Deselect)
# auto = keep each feature's own color
button_accent = auto

# --- Icons ------------------------------------------------------
# Icon colors, set apart from the text.
# icon        = regular icons (tabs, play / next, + and -, undo, ...)
#               auto = same color as the text
# icon_active = icon of the open tab and of switched-on toggles (lyrics)
#               auto = each feature's own color
icon = auto
icon_active = auto
";

/// Options added after the first release: (where the block starts in `DEFAULT_FILE`, what
/// follows it). Older files get the options they do not set yet appended.
const ADDED: &[(&str, &str)] = &[
    ("# --- Background while open", "\n# --- Text"),
    ("# Text while open", "\n# --- Buttons"),
    ("# Selected button", "\n# Main colored"),
    ("# --- Icons", ""),
];

/// The `ADDED` blocks with every option `cur` already sets left out (so an appended default
/// never overrides the user's own value), ready to append. Blocks with nothing left are skipped.
fn missing_blocks(cur: &str) -> String {
    let have = pairs(cur);
    let set = |line: &str| pairs(line).first().map(|(k, _)| have.iter().any(|(h, _)| h == k));
    let mut out = Vec::new();
    for (from, to) in ADDED {
        let Some(i) = DEFAULT_FILE.find(from) else { continue };
        let end = if to.is_empty() { Some(DEFAULT_FILE.len() - i) } else { DEFAULT_FILE[i..].find(to) };
        let Some(e) = end else { continue };
        let lines: Vec<&str> = DEFAULT_FILE[i..i + e].trim_end().lines().filter(|l| set(l) != Some(true)).collect();
        // only comments left: the user already has every option of this block
        if lines.iter().any(|l| set(l) == Some(false)) {
            out.push(lines.join("\n"));
        }
    }
    out.join("\n\n")
}

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
    } else if let Ok(cur) = std::fs::read_to_string(&p) {
        // older file without some newer options: append them (the user's settings stay)
        let add = missing_blocks(&cur);
        if !add.is_empty() {
            let _ = std::fs::write(&p, format!("{}\n\n{}\n", cur.trim_end(), add));
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
    let is = |k: &str, v: &str| get(k).map(|m| m.trim().eq_ignore_ascii_case(v)).unwrap_or(false);

    let bg = background(is("mode", "gradient"), get("gradient"), get("solid"));
    let fg = opt_color(get("text")).unwrap_or_else(|| auto_fg(&bg));
    let text_contrast = contrast(get("text_contrast"), &bg);

    // open (expanded) colors: "same" (or nothing) keeps the ones above
    let open_differs = is("expanded_mode", "solid") || is("expanded_mode", "gradient");
    let bg_open = if open_differs {
        background(is("expanded_mode", "gradient"), get("expanded_gradient"), get("expanded_solid"))
    } else {
        bg.clone()
    };
    let fg_open = match get("expanded_text") {
        Some(v) if v.trim().eq_ignore_ascii_case("same") => fg,
        v => opt_color(v).unwrap_or_else(|| if open_differs { auto_fg(&bg_open) } else { fg }),
    };
    let text_contrast_open = match get("expanded_text_contrast") {
        Some(v) if v.trim().eq_ignore_ascii_case("same") => text_contrast,
        Some(v) if !(v.trim().is_empty() || v.trim().eq_ignore_ascii_case("auto")) => contrast(Some(v), &bg_open),
        _ if open_differs => contrast(None, &bg_open),
        _ => text_contrast,
    };
    Theme {
        bg,
        fg,
        text_contrast,
        bg_open,
        fg_open,
        text_contrast_open,
        button: opt_color(get("button")),
        button_hover: opt_color(get("button_hover")),
        button_text: opt_color(get("button_text")),
        button_active: opt_color(get("button_active")),
        button_accent: opt_color(get("button_accent")),
        icon: opt_color(get("icon")),
        icon_active: opt_color(get("icon_active")),
    }
}

/// Gradient stops (when `gradient` is set and valid) or one solid stop (black by default).
fn background(gradient: bool, grad: Option<&String>, solid: Option<&String>) -> Vec<(f32, C)> {
    let mut bg = Vec::new();
    if gradient {
        bg = parse_gradient(grad.map(|s| s.as_str()).unwrap_or(""));
    }
    if bg.is_empty() {
        bg.push((0.0, opt_color(solid).unwrap_or(C(0.0, 0.0, 0.0, 1.0))));
    }
    bg
}

/// `text_contrast` value; "auto" (or anything unknown) firms text up on light backgrounds.
fn contrast(v: Option<&String>, bg: &[(f32, C)]) -> f32 {
    match v.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("normal") | Some("low") => 0.0,
        Some("high") => 0.6,
        Some("max") => 1.0,
        Some(n) if n.trim_end_matches('%').parse::<f32>().is_ok() => (n.trim_end_matches('%').parse::<f32>().unwrap() / 100.0).clamp(0.0, 1.0),
        _ => {
            if luma(bg.iter().map(|s| s.1)) > 0.6 {
                0.45
            } else {
                0.0
            }
        }
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
    fn expanded_colors() {
        // not set: open looks like closed
        let t = parse("mode = gradient\ngradient = #F07A8A, #F2DCE0\ntext = #ff0000");
        assert_eq!((t.bg_open.clone(), t.fg_open), (t.bg.clone(), t.fg));
        // black outside, blue gradient inside
        let t = parse(DEFAULT_FILE.replace("expanded_mode = same", "expanded_mode = gradient").as_str());
        assert_eq!(t.bg, vec![(0.0, C(0.0, 0.0, 0.0, 1.0))]);
        assert_eq!(t.bg_open.len(), 3);
        assert_eq!(t.bg_open[2].1, color("#4A8BFF").unwrap());
        // light open background: dark text inside, white outside
        let t = parse("expanded_mode = solid\nexpanded_solid = #F2DCE0");
        assert!(t.fg_open.0 < 0.5 && t.fg.0 > 0.5);
        assert!(t.text_contrast_open > 0.0 && t.text_contrast == 0.0);
        let t = parse("text = #00ff00\nexpanded_mode = solid\nexpanded_solid = #F2DCE0\nexpanded_text = same\nexpanded_text_contrast = max");
        assert_eq!(t.fg_open, C(0.0, 1.0, 0.0, 1.0));
        assert_eq!(t.text_contrast_open, 1.0);
    }

    #[test]
    fn appended_blocks() {
        // a first-release file gets every newer block
        let sec = missing_blocks("solid = #222222\nbutton = auto");
        for k in ["expanded_mode = same", "expanded_text_contrast = auto", "button_active = auto", "icon = auto", "icon_active = auto"] {
            assert!(sec.contains(k), "{k}");
        }
        let t = parse(&format!("solid = #222222\n{}", sec.replace("expanded_mode = same", "expanded_mode = gradient")));
        assert_eq!(t.bg[0].1, color("#222222").unwrap());
        assert_eq!(t.bg_open.len(), 3);
        // only what is missing, and nothing for the full default file
        let sec = missing_blocks("expanded_mode = same\nexpanded_text = auto");
        let keys: Vec<String> = pairs(&sec).into_iter().map(|(k, _)| k).collect();
        assert!(!keys.contains(&"expanded_mode".into()) && !keys.contains(&"expanded_text".into()));
        for k in ["expanded_solid", "expanded_text_contrast", "button_active", "icon_active"] {
            assert!(keys.contains(&k.to_string()), "{k}");
        }
        assert_eq!(missing_blocks(DEFAULT_FILE), "");
        // a value the user already set is never appended again (it would win, being last)
        let cur = "solid = #000\nicon_active = #FF8800";
        let sec = missing_blocks(cur);
        assert!(sec.contains("icon = auto") && !pairs(&sec).iter().any(|(k, _)| k == "icon_active"));
        assert_eq!(parse(&format!("{cur}\n{sec}")).icon_active, color("#FF8800"));
    }

    #[test]
    fn icons() {
        let t = parse("icon = #FFCC00\nicon_active = auto\nbutton_active = #223344");
        assert_eq!(t.icon, color("#FFCC00"));
        assert_eq!(t.icon_active, None);
        assert_eq!(t.button_active, color("#223344"));
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
