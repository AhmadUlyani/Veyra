//! The Veyra logo, drawn with Direct2D at any size. Deliberately minimal and flat:
//! a near-black squircle, a black notch hanging from its top edge (the island itself),
//! and a solid off-white V. One tiny accent dot, no gradients.
//! Used for the tray icon, the Start-menu shortcut icon and the .exe icon.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const TILE: u32 = 0x1A1A1F;
const RIM: u32 = 0x2C2C33;
const NOTCH: u32 = 0x000000;
const MARK: u32 = 0xF2F2F5;
const ACCENT: u32 = 0x5EEAD4;

fn col(hex: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 255) as f32 / 255.0,
        g: ((hex >> 8) & 255) as f32 / 255.0,
        b: (hex & 255) as f32 / 255.0,
        a,
    }
}

fn pt(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}

unsafe fn draw(f: &ID2D1Factory, rt: &ID2D1RenderTarget, s: f32) -> Result<()> {
    let small = s < 40.0;
    let u = |v: f32| v * s;
    rt.Clear(Some(&col(0, 0.0)));

    // flat squircle tile with a hairline rim
    let tile = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F { left: u(0.04), top: u(0.04), right: u(0.96), bottom: u(0.96) },
        radiusX: u(0.235),
        radiusY: u(0.235),
    };
    rt.FillRoundedRectangle(&tile, &rt.CreateSolidColorBrush(&col(TILE, 1.0), None)?);
    if !small {
        rt.DrawRoundedRectangle(&tile, &rt.CreateSolidColorBrush(&col(RIM, 1.0), None)?, u(0.012), None);
    }

    // the notch: hangs from the tile's top edge, only its bottom corners are rounded
    let (nx0, nx1) = if small { (0.27, 0.73) } else { (0.31, 0.69) };
    let nh = if small { 0.20 } else { 0.17 };
    let r = u(nh * 0.55);
    let notch = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F { left: u(nx0), top: u(0.04) - r, right: u(nx1), bottom: u(0.04 + nh) },
        radiusX: r,
        radiusY: r,
    };
    // clip to the tile so the part above its top edge disappears
    let geo: ID2D1Geometry = f.CreateRoundedRectangleGeometry(&tile)?.cast()?;
    let layer = rt.CreateLayer(None)?;
    let lp = D2D1_LAYER_PARAMETERS {
        contentBounds: D2D_RECT_F { left: 0.0, top: 0.0, right: s, bottom: s },
        geometricMask: std::mem::ManuallyDrop::new(Some(geo)),
        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
        maskTransform: windows::Foundation::Numerics::Matrix3x2::identity(),
        opacity: 1.0,
        opacityBrush: std::mem::ManuallyDrop::new(None),
        layerOptions: D2D1_LAYER_OPTIONS_NONE,
    };
    rt.PushLayer(&lp, &layer);
    let _ = std::mem::ManuallyDrop::into_inner(lp.geometricMask);
    rt.FillRoundedRectangle(&notch, &rt.CreateSolidColorBrush(&col(NOTCH, 1.0), None)?);
    rt.PopLayer();
    if !small {
        let cy = u(0.04 + nh * 0.5);
        rt.FillEllipse(
            &D2D1_ELLIPSE { point: pt(u(nx1 - 0.065), cy), radiusX: u(0.022), radiusY: u(0.022) },
            &rt.CreateSolidColorBrush(&col(ACCENT, 1.0), None)?,
        );
    }

    // the V, solid
    let round = f.CreateStrokeStyle(
        &D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            dashCap: D2D1_CAP_STYLE_ROUND,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            miterLimit: 10.0,
            dashStyle: D2D1_DASH_STYLE_SOLID,
            dashOffset: 0.0,
        },
        None,
    )?;
    let path = f.CreatePathGeometry()?;
    let sink = path.Open()?;
    let (top, bot) = if small { (0.43, 0.80) } else { (0.40, 0.78) };
    sink.BeginFigure(pt(u(0.30), u(top)), D2D1_FIGURE_BEGIN_HOLLOW);
    sink.AddLine(pt(u(0.5), u(bot)));
    sink.AddLine(pt(u(0.70), u(top)));
    sink.EndFigure(D2D1_FIGURE_END_OPEN);
    sink.Close()?;
    let width = if small { u(0.135) } else { u(0.11) };
    rt.DrawGeometry(&path, &rt.CreateSolidColorBrush(&col(MARK, 1.0), None)?, width, &round);
    Ok(())
}

/// Render the logo; returns straight (non-premultiplied) BGRA pixels, top-down.
pub fn render(size: u32) -> Option<Vec<u8>> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let bmp = wic.CreateBitmap(size, size, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad).ok()?;
        let f: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let rt = f.CreateWicBitmapRenderTarget(&bmp, &props).ok()?;
        rt.BeginDraw();
        let ok = draw(&f, &rt, size as f32).is_ok();
        rt.EndDraw(None, None).ok()?;
        if !ok {
            return None;
        }
        let mut px = vec![0u8; (size * size * 4) as usize];
        bmp.CopyPixels(std::ptr::null(), size * 4, &mut px).ok()?;
        for p in px.chunks_exact_mut(4) {
            let a = p[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        Some(px)
    }
}

/// Build a multi-resolution .ico file (32-bit DIB entries).
pub fn ico_bytes() -> Vec<u8> {
    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 96, 128, 256];
    let mut images: Vec<(u32, Vec<u8>)> = Vec::new();
    for &s in &sizes {
        let Some(px) = render(s) else { continue };
        let mask_row = s.div_ceil(32) * 4;
        let mut d = Vec::new();
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&(s as i32).to_le_bytes());
        d.extend_from_slice(&((s * 2) as i32).to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&32u16.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&(s * s * 4 + mask_row * s).to_le_bytes());
        d.extend_from_slice(&[0u8; 16]);
        for y in (0..s).rev() {
            let row = (y * s * 4) as usize;
            d.extend_from_slice(&px[row..row + (s * 4) as usize]);
        }
        d.extend(std::iter::repeat(0u8).take((mask_row * s) as usize));
        images.push((s, d));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (s, d) in &images {
        let wh = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[wh, wh, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(d.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += d.len() as u32;
    }
    for (_, d) in &images {
        out.extend_from_slice(d);
    }
    out
}

/// HICON of the logo at `size` px (for the tray).
pub fn hicon(size: u32) -> HICON {
    let Some(px) = render(size) else { return HICON::default() };
    unsafe {
        let n = size as i32;
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: n,
                biHeight: -n,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let hdc = GetDC(HWND::default());
        let mut bits = std::ptr::null_mut();
        let Ok(color) = CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0) else {
            ReleaseDC(HWND::default(), hdc);
            return HICON::default();
        };
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        let mask = CreateBitmap(n, n, 1, 1, None);
        let ii = ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&ii).unwrap_or_default();
        ReleaseDC(HWND::default(), hdc);
        let _ = DeleteObject(color);
        let _ = DeleteObject(mask);
        icon
    }
}
