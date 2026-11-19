//! QR codes in Shelf images: finds and reads them on one background thread and posts what
//! they say to the UI thread (Event::Qr). Also tells a link, a Wi-Fi login and plain text apart.

use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use std::time::Duration;

use windows::core::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;

use crate::events::{post, Event};

/// Bigger images are scaled down to this many pixels on their longest side before the search
/// (a whole 4K screen is read as it is: shrinking it loses a small code shown at 100% zoom).
const MAX_SIDE: u32 = 4096;

/// (path, whether a notice should say what was found)
static TX: Mutex<Option<Sender<(String, bool)>>> = Mutex::new(None);

/// Look for QR codes in the image at `path`; the answer comes back as Event::Qr(path, .., notify).
pub fn request(path: &str, notify: bool) {
    let Ok(mut g) = TX.lock() else { return };
    if g.is_none() {
        let (tx, rx) = channel::<(String, bool)>();
        let ok = std::thread::Builder::new()
            .name("qr".into())
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }
                let wic: Option<IWICImagingFactory> =
                    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok() };
                while let Ok((p, notify)) = rx.recv() {
                    let mut codes = None;
                    if let Some(f) = wic.as_ref() {
                        // a screenshot that just appeared may still be being written
                        for i in 0..4 {
                            codes = unsafe { grey(f, &p) }.map(|(w, h, px)| scan(w, h, &px));
                            if codes.is_some() || !notify || i == 3 {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(600));
                        }
                    }
                    post(Event::Qr(p, codes.unwrap_or_default(), notify));
                }
            })
            .is_ok();
        if !ok {
            return;
        }
        *g = Some(tx);
    }
    if let Some(tx) = g.as_ref() {
        let _ = tx.send((path.to_string(), notify));
    }
}

/// Testing aid (VEYRA_DEMO=qr...): read the codes right here, on the calling thread.
pub fn scan_now(path: &str) -> Vec<String> {
    unsafe {
        let wic: Option<IWICImagingFactory> = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok();
        wic.and_then(|f| grey(&f, path)).map(|(w, h, px)| scan(w, h, &px)).unwrap_or_default()
    }
}

/// The image as 8-bit grey pixels (scaled down when it is very big).
unsafe fn grey(f: &IWICImagingFactory, path: &str) -> Option<(u32, u32, Vec<u8>)> {
    let w = HSTRING::from(path);
    let dec = f.CreateDecoderFromFilename(&w, None, windows::Win32::Foundation::GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
    let frame = dec.GetFrame(0).ok()?;
    let (mut iw, mut ih) = (0u32, 0u32);
    frame.GetSize(&mut iw, &mut ih).ok()?;
    if iw == 0 || ih == 0 {
        return None;
    }
    let big = iw.max(ih);
    let (ow, oh) = if big > MAX_SIDE {
        (((iw as u64 * MAX_SIDE as u64) / big as u64).max(1) as u32, ((ih as u64 * MAX_SIDE as u64) / big as u64).max(1) as u32)
    } else {
        (iw, ih)
    };
    let src: IWICBitmapSource = if (ow, oh) != (iw, ih) {
        let sc = f.CreateBitmapScaler().ok()?;
        sc.Initialize(&frame, ow, oh, WICBitmapInterpolationModeFant).ok()?;
        sc.cast().ok()?
    } else {
        frame.cast().ok()?
    };
    let conv = f.CreateFormatConverter().ok()?;
    conv.Initialize(&src, &GUID_WICPixelFormat8bppGray, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
    let mut px = vec![0u8; (ow * oh) as usize];
    conv.CopyPixels(std::ptr::null(), ow, &mut px).ok()?;
    Some((ow, oh, px))
}

/// What the QR codes in a grey image say, each once, in the order found.
pub fn scan(w: u32, h: u32, px: &[u8]) -> Vec<String> {
    let (w, h) = (w as usize, h as usize);
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| px[y * w + x]);
    let mut out: Vec<String> = Vec::new();
    for g in img.detect_grids() {
        if let Ok((_, s)) = g.decode() {
            let s = s.trim_matches(|c: char| c == '\0').to_string();
            if !s.trim().is_empty() && !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

/// What a QR code holds.
#[derive(Debug, PartialEq)]
pub enum Kind {
    /// a web address that can be opened in the browser
    Link(String),
    /// a Wi-Fi login: network name and password (open networks have none)
    Wifi(String, Option<String>),
    Text(String),
}

pub fn kind(s: &str) -> Kind {
    let t = s.trim();
    let low = t.to_ascii_lowercase();
    if (low.starts_with("https://") || low.starts_with("http://")) && !t.contains(char::is_whitespace) && t.len() > 8 {
        return Kind::Link(t.to_string());
    }
    if low.starts_with("wifi:") {
        if let Some((ssid, pass)) = wifi(&t[5..]) {
            return Kind::Wifi(ssid, pass);
        }
    }
    Kind::Text(t.to_string())
}

/// `S:name;T:WPA;P:secret;;` (any order; `\` escapes `;`, `,`, `:`, `"` and `\`).
fn wifi(body: &str) -> Option<(String, Option<String>)> {
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    for ch in body.chars() {
        if esc {
            cur.push(ch);
            esc = false;
        } else if ch == '\\' {
            esc = true;
        } else if ch == ';' {
            if let Some((k, v)) = cur.split_once(':') {
                fields.push((k.trim().to_ascii_uppercase(), v.to_string()));
            }
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    if let Some((k, v)) = cur.split_once(':') {
        fields.push((k.trim().to_ascii_uppercase(), v.to_string()));
    }
    let get = |key: &str| fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    let ssid = get("S").filter(|s| !s.is_empty())?;
    let open = get("T").map(|t| t.eq_ignore_ascii_case("nopass")).unwrap_or(false);
    let pass = get("P").filter(|p| !p.is_empty() && !open);
    Some((ssid, pass))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_kinds_apart() {
        assert_eq!(kind("https://example.com/a?b=1"), Kind::Link("https://example.com/a?b=1".into()));
        assert_eq!(kind("  HTTP://x.org "), Kind::Link("HTTP://x.org".into()));
        assert_eq!(kind("https://"), Kind::Text("https://".into()));
        assert_eq!(kind("visit https://x.org today"), Kind::Text("visit https://x.org today".into()));
        assert_eq!(kind("WIFI:T:WPA;S:Home Net;P:pa\\;ss;;"), Kind::Wifi("Home Net".into(), Some("pa;ss".into())));
        assert_eq!(kind("WIFI:S:Cafe;T:nopass;P:;;"), Kind::Wifi("Cafe".into(), None));
        assert_eq!(kind("wifi:P:x;;"), Kind::Text("wifi:P:x;;".into()));
        assert_eq!(kind("hello"), Kind::Text("hello".into()));
    }

    /// Draws QR codes the way a screenshot shows them (a white margin, big modules, a grey
    /// page around) and reads them back.
    fn draw(codes: &[&str], scale: usize) -> (u32, u32, Vec<u8>) {
        let mats: Vec<(usize, Vec<bool>)> = codes
            .iter()
            .map(|s| {
                let q = qrcode::QrCode::new(s.as_bytes()).unwrap();
                (q.width(), q.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect())
            })
            .collect();
        let cell = mats.iter().map(|(n, _)| (n + 8) * scale).max().unwrap();
        let (w, h) = (cell * codes.len() + 40, cell + 40);
        let mut px = vec![200u8; w * h];
        for (i, (n, bits)) in mats.iter().enumerate() {
            let (ox, oy) = (20 + i * cell, 20);
            for y in 0..(n + 8) * scale {
                for x in 0..(n + 8) * scale {
                    let (mx, my) = ((x / scale) as isize - 4, (y / scale) as isize - 4);
                    let dark = mx >= 0 && my >= 0 && (mx as usize) < *n && (my as usize) < *n && bits[my as usize * n + mx as usize];
                    px[(oy + y) * w + ox + x] = if dark { 20 } else { 250 };
                }
            }
        }
        (w as u32, h as u32, px)
    }

    #[test]
    fn reads_drawn_codes() {
        let (w, h, px) = draw(&["https://example.com/menu?table=7"], 5);
        assert_eq!(scan(w, h, &px), vec!["https://example.com/menu?table=7".to_string()]);
        let (w, h, px) = draw(&["WIFI:T:WPA;S:Home;P:secret123;;", "hello there"], 4);
        let got = scan(w, h, &px);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got.contains(&"hello there".to_string()));
        let blank = vec![255u8; 300 * 200];
        assert!(scan(300, 200, &blank).is_empty());
    }
}
