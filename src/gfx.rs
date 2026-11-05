//! Thin Direct2D / DirectWrite wrapper rendering into a DIB that is pushed to a
//! per-pixel-alpha layered window with UpdateLayeredWindow. Uses the software
//! rasterizer on purpose: it avoids loading GPU drivers and keeps RAM low.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::mem::ManuallyDrop;

use windows::core::*;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::util::{wide, R};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct C(pub f32, pub f32, pub f32, pub f32);

impl C {
    pub const fn hex(h: u32) -> C {
        C(((h >> 16) & 255) as f32 / 255.0, ((h >> 8) & 255) as f32 / 255.0, (h & 255) as f32 / 255.0, 1.0)
    }
    pub const fn a(self, a: f32) -> C {
        C(self.0, self.1, self.2, self.3 * a)
    }
    pub fn mix(self, o: C, t: f32) -> C {
        C(
            self.0 + (o.0 - self.0) * t,
            self.1 + (o.1 - self.1) * t,
            self.2 + (o.2 - self.2) * t,
            self.3 + (o.3 - self.3) * t,
        )
    }
    fn d2d(self) -> D2D1_COLOR_F {
        D2D1_COLOR_F { r: self.0, g: self.1, b: self.2, a: self.3 }
    }
}

pub const WHITE: C = C(1.0, 1.0, 1.0, 1.0);
pub const BLACK: C = C(0.0, 0.0, 0.0, 1.0);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Al {
    L,
    C,
    R,
}

type FmtKey = (u32, u32, Al, bool, bool);

pub struct Gfx {
    factory: ID2D1Factory,
    dw: IDWriteFactory,
    rt: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    layers: Vec<ID2D1Layer>,
    depth: Cell<usize>,
    round: ID2D1StrokeStyle,
    dash: ID2D1StrokeStyle,
    fmts: RefCell<HashMap<FmtKey, IDWriteTextFormat>>,
    text_font: Vec<u16>,
    icon_font: Vec<u16>,
    memdc: HDC,
    bits: *mut u8,
    dib: HBITMAP,
    cap: (i32, i32),
    pub scale: f32,
}

fn pt(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}

fn rr(r: R, rad: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: rad, radiusY: rad }
}

impl Gfx {
    pub fn new(scale: f32) -> Result<Gfx> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0 * scale,
                dpiY: 96.0 * scale,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = factory.CreateDCRenderTarget(&props)?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = rt.CreateSolidColorBrush(&WHITE.d2d(), None)?;
            let mut layers = Vec::new();
            for _ in 0..4 {
                layers.push(rt.CreateLayer(None)?);
            }
            let mut sp = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
            };
            let round = factory.CreateStrokeStyle(&sp, None)?;
            sp.dashStyle = D2D1_DASH_STYLE_DASH;
            let dash = factory.CreateStrokeStyle(&sp, None)?;

            let mut coll: Option<IDWriteFontCollection> = None;
            let _ = dw.GetSystemFontCollection(&mut coll, BOOL::from(false));
            let has = |name: &str| -> bool {
                let Some(c) = &coll else { return false };
                let w = wide(name);
                let mut i = 0u32;
                let mut ex = BOOL(0);
                c.FindFamilyName(PCWSTR(w.as_ptr()), &mut i, &mut ex).is_ok() && ex.as_bool()
            };
            let text_font = if has("Segoe UI Variable Text") { "Segoe UI Variable Text" } else { "Segoe UI" };
            let icon_font = if has("Segoe Fluent Icons") { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" };

            let memdc = CreateCompatibleDC(HDC::default());
            Ok(Gfx {
                factory,
                dw,
                rt,
                brush,
                layers,
                depth: Cell::new(0),
                round,
                dash,
                fmts: RefCell::new(HashMap::new()),
                text_font: wide(text_font),
                icon_font: wide(icon_font),
                memdc,
                bits: std::ptr::null_mut(),
                dib: HBITMAP::default(),
                cap: (0, 0),
                scale,
            })
        }
    }

    /// Write the top-left w×h of the frame buffer as a 32-bit BMP (premultiplied BGRA).
    pub fn dump_bmp(&self, path: &str, w: i32, h: i32) {
        if self.bits.is_null() || w <= 0 || h <= 0 || w > self.cap.0 || h > self.cap.1 {
            return;
        }
        let mut out = Vec::with_capacity(54 + (w * h * 4) as usize);
        let size = 54 + (w * h * 4) as u32;
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&(-h).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&[0u8; 24]);
        let _ = unsafe { windows::Win32::Graphics::Gdi::GdiFlush() };
        for y in 0..h {
            let row = unsafe { std::slice::from_raw_parts(self.bits.add((y * self.cap.0 * 4) as usize), (w * 4) as usize) };
            out.extend_from_slice(row);
        }
        let _ = std::fs::write(path, out);
    }

    pub fn set_scale(&mut self, s: f32) {
        self.scale = s;
        unsafe { self.rt.SetDpi(96.0 * s, 96.0 * s) };
    }

    fn ensure_dib(&mut self, w: i32, h: i32) -> bool {
        if w <= self.cap.0 && h <= self.cap.1 && !self.dib.is_invalid() {
            return true;
        }
        let nw = w.max(self.cap.0);
        let nh = h.max(self.cap.1);
        unsafe {
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: nw,
                    biHeight: -nh,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(dib) = CreateDIBSection(self.memdc, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0) else {
                return false;
            };
            SelectObject(self.memdc, dib);
            if !self.dib.is_invalid() {
                let _ = DeleteObject(self.dib);
            }
            self.dib = dib;
            self.bits = bits as *mut u8;
            self.cap = (nw, nh);
        }
        true
    }

    pub fn begin(&mut self, w: i32, h: i32) -> bool {
        if !self.ensure_dib(w, h) {
            return false;
        }
        let rc = RECT { left: 0, top: 0, right: w, bottom: h };
        unsafe {
            if self.rt.BindDC(self.memdc, &rc).is_err() {
                return false;
            }
            self.rt.BeginDraw();
            self.rt.SetTransform(&Matrix3x2::identity());
            self.rt.Clear(Some(&C(0.0, 0.0, 0.0, 0.0).d2d()));
        }
        self.depth.set(0);
        true
    }

    pub fn end(&self, hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            while self.depth.get() > 0 {
                self.pop();
            }
            let _ = self.rt.EndDraw(None, None);
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                hwnd,
                HDC::default(),
                Some(&POINT { x, y }),
                Some(&SIZE { cx: w, cy: h }),
                self.memdc,
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }
    }

    /// Offset everything drawn afterwards (used for sliding page transitions).
    pub fn translate(&self, dx: f32, dy: f32) {
        unsafe { self.rt.SetTransform(&Matrix3x2::translation(dx, dy)) }
    }

    fn b(&self, c: C) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&c.d2d()) };
        &self.brush
    }

    pub fn rrect(&self, r: R, rad: f32, c: C) {
        unsafe { self.rt.FillRoundedRectangle(&rr(r, rad), self.b(c)) }
    }

    pub fn rect(&self, r: R, c: C) {
        unsafe { self.rt.FillRectangle(&r.d2d(), self.b(c)) }
    }

    pub fn rrect_stroke(&self, r: R, rad: f32, c: C, w: f32, dashed: bool) {
        let st = if dashed { &self.dash } else { &self.round };
        unsafe { self.rt.DrawRoundedRectangle(&rr(r, rad), self.b(c), w, st) }
    }

    pub fn circle(&self, cx: f32, cy: f32, rad: f32, c: C) {
        let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: rad, radiusY: rad };
        unsafe { self.rt.FillEllipse(&e, self.b(c)) }
    }

    pub fn circle_stroke(&self, cx: f32, cy: f32, rad: f32, c: C, w: f32) {
        let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: rad, radiusY: rad };
        unsafe { self.rt.DrawEllipse(&e, self.b(c), w, &self.round) }
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, w: f32, c: C) {
        unsafe { self.rt.DrawLine(pt(x0, y0), pt(x1, y1), self.b(c), w, &self.round) }
    }

    /// Progress arc starting at 12 o'clock, clockwise.
    pub fn arc(&self, cx: f32, cy: f32, rad: f32, frac: f32, w: f32, c: C) {
        if frac <= 0.002 {
            return;
        }
        if frac >= 0.998 {
            self.circle_stroke(cx, cy, rad, c, w);
            return;
        }
        let a0 = -std::f32::consts::FRAC_PI_2;
        let a1 = a0 + frac * std::f32::consts::TAU;
        unsafe {
            let Ok(geo) = self.factory.CreatePathGeometry() else { return };
            let Ok(sink) = geo.Open() else { return };
            sink.BeginFigure(pt(cx + rad * a0.cos(), cy + rad * a0.sin()), D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: pt(cx + rad * a1.cos(), cy + rad * a1.sin()),
                size: D2D_SIZE_F { width: rad, height: rad },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if frac > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            if sink.Close().is_ok() {
                self.rt.DrawGeometry(&geo, self.b(c), w, &self.round);
            }
        }
    }

    /// Water drop shape filling `r`.
    pub fn drop_geo(&self, r: R) -> Option<ID2D1Geometry> {
        let cx = r.cx();
        let (top, w, h) = (r.y, r.w, r.h);
        let yb = top + h * 0.62;
        unsafe {
            let geo = self.factory.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            sink.BeginFigure(pt(cx, top), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: pt(cx + w * 0.12, top + h * 0.2),
                point2: pt(cx + w * 0.5, top + h * 0.36),
                point3: pt(cx + w * 0.5, yb),
            });
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: pt(cx - w * 0.5, yb),
                size: D2D_SIZE_F { width: w * 0.5, height: h * 0.38 },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_SMALL,
            });
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: pt(cx - w * 0.5, top + h * 0.36),
                point2: pt(cx - w * 0.12, top + h * 0.2),
                point3: pt(cx, top),
            });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close().ok()?;
            geo.cast().ok()
        }
    }

    /// Concave fillet next to a top-attached shape: a `f`×`f` square at the top edge
    /// with a quarter circle cut out, so the notch flows smoothly into the screen edge.
    /// `x` is the shape's left edge (left = true) or right edge (left = false).
    pub fn shoulder(&self, x: f32, y: f32, f: f32, left: bool, c: C) {
        if f < 0.5 {
            return;
        }
        let o = if left { x - f } else { x + f };
        unsafe {
            let Ok(geo) = self.factory.CreatePathGeometry() else { return };
            let Ok(sink) = geo.Open() else { return };
            sink.BeginFigure(pt(o, y), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(pt(x, y));
            sink.AddLine(pt(x, y + f));
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: pt(o, y),
                size: D2D_SIZE_F { width: f, height: f },
                rotationAngle: 0.0,
                sweepDirection: if left { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_CLOCKWISE },
                arcSize: D2D1_ARC_SIZE_SMALL,
            });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            if sink.Close().is_ok() {
                self.rt.FillGeometry(&geo, self.b(c), None);
            }
        }
    }

    pub fn fill_geo(&self, g: &ID2D1Geometry, c: C) {
        unsafe { self.rt.FillGeometry(g, self.b(c), None) }
    }

    pub fn stroke_geo(&self, g: &ID2D1Geometry, c: C, w: f32) {
        unsafe { self.rt.DrawGeometry(g, self.b(c), w, &self.round) }
    }

    fn fmt(&self, size: f32, weight: u32, al: Al, icon: bool, wrap: bool) -> Option<IDWriteTextFormat> {
        let key = ((size * 10.0) as u32, weight, al, icon, wrap);
        if let Some(f) = self.fmts.borrow().get(&key) {
            return Some(f.clone());
        }
        unsafe {
            let fam = if icon { &self.icon_font } else { &self.text_font };
            let f = self
                .dw
                .CreateTextFormat(
                    PCWSTR(fam.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT(weight as i32),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-us"),
                )
                .ok()?;
            let _ = f.SetTextAlignment(match al {
                Al::L => DWRITE_TEXT_ALIGNMENT_LEADING,
                Al::C => DWRITE_TEXT_ALIGNMENT_CENTER,
                Al::R => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            let _ = f.SetParagraphAlignment(if wrap {
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR
            } else {
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER
            });
            if !wrap {
                let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            }
            let sign = self.dw.CreateEllipsisTrimmingSign(&f).ok();
            let t = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            };
            let _ = f.SetTrimming(&t, sign.as_ref());
            self.fmts.borrow_mut().insert(key, f.clone());
            Some(f)
        }
    }

    fn draw_text(&self, s: &str, r: R, f: &IDWriteTextFormat, c: C) {
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            self.rt.DrawText(&w, f, &r.d2d(), self.b(c), D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    pub fn text(&self, s: &str, r: R, size: f32, weight: u32, c: C, al: Al) {
        if let Some(f) = self.fmt(size, weight, al, false, false) {
            self.draw_text(s, r, &f, c);
        }
    }

    pub fn text_wrap(&self, s: &str, r: R, size: f32, weight: u32, c: C, al: Al) {
        if let Some(f) = self.fmt(size, weight, al, false, true) {
            self.draw_text(s, r, &f, c);
        }
    }

    pub fn icon(&self, ch: char, r: R, size: f32, c: C) {
        if let Some(f) = self.fmt(size, 400, Al::C, true, false) {
            let mut b = [0u8; 4];
            self.draw_text(ch.encode_utf8(&mut b), r, &f, c);
        }
    }

    pub fn measure(&self, s: &str, size: f32, weight: u32) -> f32 {
        let Some(f) = self.fmt(size, weight, Al::L, false, false) else { return 0.0 };
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            if let Ok(l) = self.dw.CreateTextLayout(&w, &f, 10000.0, 100.0) {
                let mut m = DWRITE_TEXT_METRICS::default();
                if l.GetMetrics(&mut m).is_ok() {
                    return m.widthIncludingTrailingWhitespace;
                }
            }
        }
        0.0
    }

    pub fn create_bitmap(&self, w: u32, h: u32, px: &[u8]) -> Option<ID2D1Bitmap> {
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        unsafe {
            self.rt
                .CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(px.as_ptr() as *const _), w * 4, &props)
                .ok()
        }
    }

    pub fn bitmap(&self, b: &ID2D1Bitmap, r: R, rad: f32) {
        self.push_rr(r, rad, 1.0);
        unsafe {
            self.rt.DrawBitmap(b, Some(&r.d2d()), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
        }
        self.pop();
    }

    pub fn push_geo(&self, geo: &ID2D1Geometry, opacity: f32) {
        let d = self.depth.get();
        if d >= self.layers.len() {
            return;
        }
        let p = D2D1_LAYER_PARAMETERS {
            contentBounds: D2D_RECT_F { left: -1e5, top: -1e5, right: 1e5, bottom: 1e5 },
            geometricMask: ManuallyDrop::new(Some(geo.clone())),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity,
            opacityBrush: ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        unsafe { self.rt.PushLayer(&p, &self.layers[d]) };
        let _ = ManuallyDrop::into_inner(p.geometricMask);
        self.depth.set(d + 1);
    }

    pub fn push_rr(&self, r: R, rad: f32, opacity: f32) {
        unsafe {
            if let Ok(g) = self.factory.CreateRoundedRectangleGeometry(&rr(r, rad)) {
                if let Ok(g) = g.cast::<ID2D1Geometry>() {
                    self.push_geo(&g, opacity);
                    return;
                }
            }
        }
        // keep push/pop balanced even on failure
        self.depth.set(self.depth.get());
    }

    pub fn pop(&self) {
        let d = self.depth.get();
        if d > 0 {
            unsafe { self.rt.PopLayer() };
            self.depth.set(d - 1);
        }
    }
}

impl Drop for Gfx {
    fn drop(&mut self) {
        unsafe {
            if !self.dib.is_invalid() {
                let _ = DeleteObject(self.dib);
            }
            let _ = DeleteDC(self.memdc);
        }
    }
}

