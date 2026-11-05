//! System watchers: volume, audio output device, network, battery, brightness,
//! Caps Lock, keyboard layout and full-screen detection.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::*;
use windows::Networking::Connectivity::*;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::*;
use windows::Win32::Globalization::*;
use windows::Win32::Media::Audio::Endpoints::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::PropertiesSystem::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::events::{post, Event};

// ---------------------------------------------------------------- audio

#[implement(IAudioEndpointVolumeCallback)]
struct VolCb;

impl IAudioEndpointVolumeCallback_Impl for VolCb_Impl {
    fn OnNotify(&self, p: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        unsafe {
            if let Some(d) = p.as_ref() {
                post(Event::Volume(d.fMasterVolume, d.bMuted.as_bool()));
            }
        }
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DevCb;

impl IMMNotificationClient_Impl for DevCb_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> Result<()> {
        if flow == eRender && role == eConsole {
            post(Event::AudioDevice);
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}

pub struct Audio {
    en: IMMDeviceEnumerator,
    dev_cb: IMMNotificationClient,
    ep: Option<IAudioEndpointVolume>,
    cb: IAudioEndpointVolumeCallback,
    pub name: String,
    id: String,
}

unsafe fn dev_name(dev: &IMMDevice) -> String {
    let Ok(store) = dev.OpenPropertyStore(STGM_READ) else { return String::new() };
    let Ok(pv) = store.GetValue(&PKEY_Device_FriendlyName) else { return String::new() };
    match PropVariantToStringAlloc(&pv) {
        Ok(p) => {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        }
        Err(_) => String::new(),
    }
}

impl Audio {
    pub fn new() -> Option<Audio> {
        unsafe {
            let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let dev_cb: IMMNotificationClient = DevCb.into();
            let _ = en.RegisterEndpointNotificationCallback(&dev_cb);
            let cb: IAudioEndpointVolumeCallback = VolCb.into();
            let mut a = Audio { en, dev_cb, ep: None, cb, name: String::new(), id: String::new() };
            a.rebind();
            Some(a)
        }
    }

    /// Re-attach to the current default output. Returns true if the device changed.
    pub fn rebind(&mut self) -> bool {
        unsafe {
            if let Some(ep) = self.ep.take() {
                let _ = ep.UnregisterControlChangeNotify(&self.cb);
            }
            let Ok(dev) = self.en.GetDefaultAudioEndpoint(eRender, eConsole) else {
                self.name.clear();
                return false;
            };
            let id = match dev.GetId() {
                Ok(p) => {
                    let s = p.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as *const _));
                    s
                }
                Err(_) => String::new(),
            };
            self.name = dev_name(&dev);
            if let Ok(ep) = dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) {
                let _ = ep.RegisterControlChangeNotify(&self.cb);
                self.ep = Some(ep);
            }
            let changed = !self.id.is_empty() && id != self.id;
            self.id = id;
            changed
        }
    }

    pub fn get(&self) -> Option<(f32, bool)> {
        let ep = self.ep.as_ref()?;
        unsafe {
            let v = ep.GetMasterVolumeLevelScalar().ok()?;
            let m = ep.GetMute().map(|b| b.as_bool()).unwrap_or(false);
            Some((v, m))
        }
    }

    pub fn set(&self, v: f32) {
        if let Some(ep) = &self.ep {
            unsafe {
                let _ = ep.SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), std::ptr::null());
                if v > 0.0 {
                    let _ = ep.SetMute(BOOL::from(false), std::ptr::null());
                }
            }
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        unsafe {
            if let Some(ep) = self.ep.take() {
                let _ = ep.UnregisterControlChangeNotify(&self.cb);
            }
            let _ = self.en.UnregisterEndpointNotificationCallback(&self.dev_cb);
        }
    }
}

// ---------------------------------------------------------------- network

/// (internet reachable, profile name, is wifi)
pub fn net_status() -> (bool, String, bool) {
    match NetworkInformation::GetInternetConnectionProfile() {
        Ok(p) => {
            let lvl = p.GetNetworkConnectivityLevel().unwrap_or(NetworkConnectivityLevel::None);
            let name = p.ProfileName().map(|h| h.to_string()).unwrap_or_default();
            let wifi = p.IsWlanConnectionProfile().unwrap_or(false);
            (lvl == NetworkConnectivityLevel::InternetAccess, name, wifi)
        }
        Err(_) => (false, String::new(), false),
    }
}

pub fn watch_network() {
    let _ = NetworkInformation::NetworkStatusChanged(&NetworkStatusChangedEventHandler::new(|_| {
        post(Event::NetChanged);
        Ok(())
    }));
}

// ---------------------------------------------------------------- power

/// (on AC power, battery percent) — None when there is no battery.
pub fn power() -> Option<(bool, u8)> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).ok()? };
    if s.BatteryFlag & 128 != 0 || s.BatteryLifePercent == 255 {
        return None;
    }
    Some((s.ACLineStatus == 1, s.BatteryLifePercent.min(100)))
}

pub const GUID_BRIGHTNESS: GUID = GUID::from_u128(0x8ffee2c6_2d01_46be_adb9_398addc5b4ff);

pub fn watch_brightness(hwnd: HWND) {
    unsafe {
        let _ = RegisterPowerSettingNotification(HANDLE(hwnd.0), &GUID_BRIGHTNESS, DEVICE_NOTIFY_WINDOW_HANDLE);
    }
}

// ---------------------------------------------------------------- keyboard

static CAPS_DOWN: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn kb_proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let k = &*(l.0 as *const KBDLLHOOKSTRUCT);
        if k.vkCode == VK_CAPITAL.0 as u32 {
            let m = w.0 as u32;
            if m == WM_KEYDOWN || m == WM_SYSKEYDOWN {
                if !CAPS_DOWN.swap(true, Ordering::Relaxed) {
                    post(Event::Caps);
                }
            } else {
                CAPS_DOWN.store(false, Ordering::Relaxed);
            }
        }
    }
    CallNextHookEx(HHOOK::default(), code, w, l)
}

pub fn install_kb_hook() {
    unsafe {
        let h = GetModuleHandleW(None).unwrap_or_default();
        let _ = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), HINSTANCE(h.0), 0);
    }
}

pub fn caps_state() -> bool {
    unsafe { (GetKeyState(VK_CAPITAL.0 as i32) & 1) != 0 }
}

/// Language id of the keyboard layout used by the foreground window.
pub fn fg_lang() -> u16 {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return 0;
        }
        let tid = GetWindowThreadProcessId(fg, None);
        if tid == 0 {
            return 0;
        }
        (GetKeyboardLayout(tid).0 as usize & 0xFFFF) as u16
    }
}

/// (English display name, short tag like "EN")
pub fn lang_name(lid: u16) -> (String, String) {
    unsafe {
        let mut buf = [0u16; 86];
        let n = LCIDToLocaleName(lid as u32, Some(&mut buf), 0);
        if n <= 1 {
            return (format!("Layout {:04X}", lid), "??".into());
        }
        let tag = String::from_utf16_lossy(&buf[..(n - 1) as usize]);
        let short = tag.split('-').next().unwrap_or(&tag).to_uppercase();
        let mut b2 = [0u16; 160];
        let n2 = GetLocaleInfoEx(PCWSTR(buf.as_ptr()), LOCALE_SENGLISHDISPLAYNAME, Some(&mut b2));
        let name = if n2 > 1 { String::from_utf16_lossy(&b2[..(n2 - 1) as usize]) } else { tag.clone() };
        (name, short)
    }
}

// ---------------------------------------------------------------- full screen

pub fn fullscreen_busy() -> bool {
    unsafe {
        match SHQueryUserNotificationState() {
            Ok(s) => s == QUNS_BUSY || s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE,
            Err(_) => false,
        }
    }
}
