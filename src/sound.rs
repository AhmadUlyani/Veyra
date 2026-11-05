//! A soft two-note chime synthesized in memory (no audio files shipped).

use std::sync::OnceLock;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

static WAV: OnceLock<Vec<u8>> = OnceLock::new();

pub fn chime() {
    let wav = WAV.get_or_init(build);
    unsafe {
        let _ = PlaySoundW(PCWSTR(wav.as_ptr() as *const u16), HMODULE::default(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

fn bell(t: f32, f: f32) -> f32 {
    if t < 0.0 {
        return 0.0;
    }
    let tau = std::f32::consts::TAU;
    let attack = (t / 0.006).min(1.0);
    attack
        * ((tau * f * t).sin() * (-3.2 * t).exp() * 0.6
            + (tau * f * 2.0 * t).sin() * (-5.0 * t).exp() * 0.25
            + (tau * f * 3.01 * t).sin() * (-8.0 * t).exp() * 0.1)
}

fn build() -> Vec<u8> {
    let sr = 44100u32;
    let n = (sr as f32 * 1.8) as usize;
    let mut pcm = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f32 / sr as f32;
        let s = bell(t, 880.0) * 0.55 + bell(t - 0.17, 1318.5) * 0.45;
        let v = (s * 0.3 * 32767.0).clamp(-32767.0, 32767.0) as i16;
        pcm.extend_from_slice(&v.to_le_bytes());
    }
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&sr.to_le_bytes());
    w.extend_from_slice(&(sr * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(&pcm);
    w
}
