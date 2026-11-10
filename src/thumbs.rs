//! Shelf image previews: decodes image files into small thumbnails on one background
//! thread and posts them to the UI thread (Event::Thumb).

use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;

use windows::core::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;

use crate::events::{post, Event};

/// Thumbnail size in pixels (about 2x the tile's preview box, so it stays sharp on HiDPI).
pub const TW: u32 = 128;
pub const TH: u32 = 96;

pub struct Thumb {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

static TX: Mutex<Option<Sender<String>>> = Mutex::new(None);

pub fn is_image(path: &str) -> bool {
    matches!(
        std::path::Path::new(path).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "jfif" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico" | "heic")
    )
}

/// Ask for a thumbnail of `path`; the answer comes back as Event::Thumb(path, ..).
pub fn request(path: &str) {
    let Ok(mut g) = TX.lock() else { return };
    if g.is_none() {
        let (tx, rx) = channel::<String>();
        let ok = std::thread::Builder::new()
            .name("thumbs".into())
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }
                let wic: Option<IWICImagingFactory> =
                    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok() };
                while let Ok(p) = rx.recv() {
                    let t = wic.as_ref().and_then(|f| unsafe { decode(f, &p) });
                    post(Event::Thumb(p, t));
                }
            })
            .is_ok();
        if !ok {
            return;
        }
        *g = Some(tx);
    }
    if let Some(tx) = g.as_ref() {
        let _ = tx.send(path.to_string());
    }
}

unsafe fn decode(f: &IWICImagingFactory, path: &str) -> Option<Thumb> {
    let w = HSTRING::from(path);
    let dec = f.CreateDecoderFromFilename(&w, None, GENERIC_READ_ACCESS, WICDecodeMetadataCacheOnDemand).ok()?;
    let frame = dec.GetFrame(0).ok()?;
    let (mut iw, mut ih) = (0u32, 0u32);
    frame.GetSize(&mut iw, &mut ih).ok()?;
    if iw == 0 || ih == 0 {
        return None;
    }
    // fill the preview box: crop the middle of the image to its aspect ratio
    let (cw, ch) = if iw as u64 * TH as u64 > ih as u64 * TW as u64 {
        (((ih as u64 * TW as u64) / TH as u64).max(1) as u32, ih)
    } else {
        (iw, ((iw as u64 * TH as u64) / TW as u64).max(1) as u32)
    };
    let clip = f.CreateBitmapClipper().ok()?;
    let rc = WICRect { X: ((iw - cw) / 2) as i32, Y: ((ih - ch) / 2) as i32, Width: cw as i32, Height: ch as i32 };
    clip.Initialize(&frame, &rc).ok()?;
    // tiny images are not blown up past their size
    let (ow, oh) = if cw < TW { (cw, ch) } else { (TW, TH) };
    let sc = f.CreateBitmapScaler().ok()?;
    sc.Initialize(&clip, ow, oh, WICBitmapInterpolationModeFant).ok()?;
    let conv = f.CreateFormatConverter().ok()?;
    conv.Initialize(&sc, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)
        .ok()?;
    let mut px = vec![0u8; (ow * oh * 4) as usize];
    conv.CopyPixels(std::ptr::null(), ow * 4, &mut px).ok()?;
    Some(Thumb { w: ow, h: oh, px })
}

const GENERIC_READ_ACCESS: windows::Win32::Foundation::GENERIC_ACCESS_RIGHTS = windows::Win32::Foundation::GENERIC_READ;
