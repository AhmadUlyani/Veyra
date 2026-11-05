use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::System::SystemInformation::GetLocalTime;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> R {
        R { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x && p.0 < self.right() && p.1 >= self.y && p.1 < self.bottom()
    }
    pub fn inset(&self, dx: f32, dy: f32) -> R {
        R::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
    pub fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.right(), bottom: self.bottom() }
    }
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Local wall-clock time. `secs` is a "naive local epoch" (seconds since 1970-01-01 local),
/// good enough for scheduling reminders without dealing with time zones.
#[allow(dead_code)]
pub struct Now {
    pub y: i64,
    pub mo: i64,
    pub d: i64,
    pub h: i64,
    pub mi: i64,
    pub secs: i64,
}

impl Now {
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.y, self.mo, self.d)
    }
    pub fn day(&self) -> i64 {
        self.secs.div_euclid(86400)
    }
    /// "Tue, 3 Nov 2026"
    pub fn date_label(&self) -> String {
        const WD: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"]; // 1970-01-01 was a Thursday
        const MO: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let wd = WD[self.day().rem_euclid(7) as usize];
        let mo = MO[(self.mo.clamp(1, 12) - 1) as usize];
        format!("{wd}, {} {mo} {}", self.d, self.y)
    }
}

pub fn now() -> Now {
    let st = unsafe { GetLocalTime() };
    let (y, mo, d) = (st.wYear as i64, st.wMonth as i64, st.wDay as i64);
    let (h, mi, s) = (st.wHour as i64, st.wMinute as i64, st.wSecond as i64);
    let secs = days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + s;
    Now { y, mo, d, h, mi, secs }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn hm(secs: i64) -> String {
    let s = secs.rem_euclid(86400);
    format!("{:02}:{:02}", s / 3600, (s / 60) % 60)
}

/// "14:30", "Tmrw 09:00", "+3d 10:00", "Yday 18:00"
pub fn due_label(due: i64, today: i64) -> String {
    let dd = due.div_euclid(86400) - today;
    match dd {
        0 => hm(due),
        1 => format!("Tmrw {}", hm(due)),
        -1 => format!("Yday {}", hm(due)),
        n if n > 1 => format!("+{}d {}", n, hm(due)),
        n => format!("{}d {}", n, hm(due)),
    }
}

pub fn mmss(secs: f64) -> String {
    let s = secs.max(0.0).round() as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}
