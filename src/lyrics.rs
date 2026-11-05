//! Synced lyrics from LRCLIB (https://lrclib.net), a free, open lyrics database.

use windows::core::*;
use windows::Foundation::Uri;
use windows::Web::Http::{HttpClient, HttpStatusCode};
use windows::Win32::System::WinRT::*;

use crate::events::{post, Event};

#[derive(Clone)]
pub struct Line {
    pub t: f64,
    pub text: String,
}

pub fn fetch(key: u64, title: String, artist: String, album: String, dur: f64) {
    std::thread::spawn(move || {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let r = find(&title, &artist, &album, dur);
        post(Event::Lyrics(key, r));
    });
}

fn esc(s: &str) -> String {
    Uri::EscapeComponent(&HSTRING::from(s)).map(|h| h.to_string()).unwrap_or_default()
}

fn http_get(client: &HttpClient, url: &str) -> Option<String> {
    let uri = Uri::CreateUri(&HSTRING::from(url)).ok()?;
    let resp = client.GetAsync(&uri).ok()?.get().ok()?;
    if resp.StatusCode().ok()? != HttpStatusCode::Ok {
        return None;
    }
    Some(resp.Content().ok()?.ReadAsStringAsync().ok()?.get().ok()?.to_string())
}

const NOISE: [&str; 14] = [
    "official", "video", "lyric", "audio", "visualizer", "visualiser", "mv", "m/v", "hd", "4k", "remaster", "live",
    "color coded", "performance",
];

pub fn strip_brackets(s: &str, all: bool) -> String {
    let mut out = String::new();
    let mut depth = 0;
    let mut buf = String::new();
    for ch in s.chars() {
        match ch {
            '(' | '[' | '【' => {
                if depth == 0 {
                    buf.clear();
                }
                depth += 1;
                buf.push(ch);
            }
            ')' | ']' | '】' if depth > 0 => {
                buf.push(ch);
                depth -= 1;
                if depth == 0 {
                    let l = buf.to_lowercase();
                    if !all && !NOISE.iter().any(|n| l.contains(n)) {
                        out.push_str(&buf);
                    }
                    buf.clear();
                }
            }
            _ if depth > 0 => buf.push(ch),
            _ => out.push(ch),
        }
    }
    let out = out.replace('|', " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn clean_artist(a: &str) -> String {
    let a = a.trim();
    let a = a.strip_suffix(" - Topic").unwrap_or(a);
    let a = a.strip_suffix("VEVO").unwrap_or(a);
    a.trim().to_string()
}

fn candidates(title: &str, artist: &str) -> Vec<(String, String)> {
    let t = strip_brackets(title, false);
    let a = clean_artist(artist);
    let mut v = vec![(t.clone(), a.clone())];
    // YouTube style "Artist - Title"
    if let Some((l, r)) = t.split_once(" - ") {
        v.push((r.trim().to_string(), l.trim().to_string()));
    }
    let bare = strip_brackets(title, true);
    let bare = bare.split(" feat.").next().unwrap_or(&bare).split(" ft.").next().unwrap_or(&bare).trim().to_string();
    if bare != t {
        v.push((bare, a));
    }
    v
}

fn pick(json: &serde_json::Value, dur: f64) -> Option<String> {
    let arr = json.as_array()?;
    let mut best: Option<(f64, String)> = None;
    for it in arr {
        let Some(s) = it.get("syncedLyrics").and_then(|v| v.as_str()) else { continue };
        if s.trim().is_empty() {
            continue;
        }
        let d = it.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let score = if dur > 0.0 && d > 0.0 { (d - dur).abs() } else { 30.0 };
        if best.as_ref().map(|b| score < b.0).unwrap_or(true) {
            best = Some((score, s.to_string()));
        }
    }
    best.map(|b| b.1)
}

fn find(title: &str, artist: &str, album: &str, dur: f64) -> Option<Vec<Line>> {
    let client = HttpClient::new().ok()?;
    if let Ok(h) = client.DefaultRequestHeaders() {
        if let Ok(ua) = h.UserAgent() {
            let _ = ua.TryParseAdd(&HSTRING::from("Veyra/0.1 (Windows dynamic island)"));
        }
    }
    let base = "https://lrclib.net/api";
    for (i, (t, a)) in candidates(title, artist).into_iter().enumerate() {
        if t.is_empty() {
            continue;
        }
        if i == 0 && !a.is_empty() {
            let mut url = format!("{base}/get?track_name={}&artist_name={}", esc(&t), esc(&a));
            if !album.is_empty() {
                url.push_str(&format!("&album_name={}", esc(album)));
            }
            if dur > 0.0 {
                url.push_str(&format!("&duration={}", dur.round() as i64));
            }
            if let Some(body) = http_get(&client, &url) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                    if let Some(s) = v.get("syncedLyrics").and_then(|x| x.as_str()) {
                        let l = parse_lrc(s);
                        if !l.is_empty() {
                            return Some(l);
                        }
                    }
                }
            }
        }
        let url = if a.is_empty() {
            format!("{base}/search?q={}", esc(&t))
        } else {
            format!("{base}/search?track_name={}&artist_name={}", esc(&t), esc(&a))
        };
        if let Some(body) = http_get(&client, &url) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                if let Some(s) = pick(&v, dur) {
                    let l = parse_lrc(&s);
                    if !l.is_empty() {
                        return Some(l);
                    }
                }
            }
        }
    }
    // last resort: free-text search
    let q = format!("{} {}", strip_brackets(title, true), clean_artist(artist));
    let body = http_get(&client, &format!("{base}/search?q={}", esc(q.trim())))?;
    let v = serde_json::from_str::<serde_json::Value>(&body).ok()?;
    let l = parse_lrc(&pick(&v, dur)?);
    (!l.is_empty()).then_some(l)
}

pub fn parse_lrc(s: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for raw in s.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(r) = rest.strip_prefix('[') {
            let Some(end) = r.find(']') else { break };
            let tag = &r[..end];
            rest = &r[end + 1..];
            if let Some((m, sec)) = tag.split_once(':') {
                if let (Ok(m), Ok(sec)) = (m.trim().parse::<f64>(), sec.trim().replace(':', ".").parse::<f64>()) {
                    times.push(m * 60.0 + sec);
                }
            }
        }
        let text = rest.trim();
        let text = if text.is_empty() { "♪" } else { text };
        for t in times {
            out.push(Line { t, text: text.to_string() });
        }
    }
    out.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    out
}
