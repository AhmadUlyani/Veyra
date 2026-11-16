//! Muting every microphone at once, and whether they are muted.

use windows::Win32::Foundation::BOOL;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

pub struct Watcher {
    en: Option<IMMDeviceEnumerator>,
}

impl Watcher {
    pub fn new() -> Watcher {
        let en = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok() };
        Watcher { en }
    }

    /// Whether the default microphone is muted (None: there is no microphone).
    pub fn mic_muted(&self) -> Option<bool> {
        let en = self.en.as_ref()?;
        unsafe {
            let dev = en.GetDefaultAudioEndpoint(eCapture, eConsole).ok()?;
            let ep: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None).ok()?;
            ep.GetMute().ok().map(|b| b.as_bool())
        }
    }

    /// Mute (or unmute) every active microphone. False if there is none.
    pub fn set_mic_muted(&self, on: bool) -> bool {
        let Some(en) = &self.en else { return false };
        let mut any = false;
        unsafe {
            let Ok(list) = en.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else { return false };
            for i in 0..list.GetCount().unwrap_or(0) {
                let Ok(dev) = list.Item(i) else { continue };
                if let Ok(ep) = dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) {
                    if ep.SetMute(BOOL::from(on), std::ptr::null()).is_ok() {
                        any = true;
                    }
                }
            }
        }
        any
    }
}
