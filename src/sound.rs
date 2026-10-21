//! Dog sounds: scales the embedded WAV to the requested volume.
//! Playback is platform-specific (Windows: `PlaySoundW`); this module only prepares the data.

/// `assets/sounds/bark.wav`: small dog bark (sfxmint.com, CC0).
pub const BARK_WAV: &[u8] = include_bytes!("../assets/sounds/bark.wav");

/// Scales the samples of a 16-bit PCM WAV by `volume` percent. The header is unchanged.
/// `None` if the format is not supported.
pub fn scaled_wav(wav: &[u8], volume: u8) -> Option<Vec<u8>> {
    let (data_start, data_len) = pcm16_data(wav)?;
    let mut out = wav.to_vec();
    let gain = volume.min(100) as i32;
    let end = (data_start + data_len).min(out.len());
    for s in out[data_start..end].as_chunks_mut::<2>().0 {
        let v = i16::from_le_bytes(*s) as i32 * gain / 100;
        *s = (v as i16).to_le_bytes();
    }
    Some(out)
}

/// Start and length of the 16-bit PCM `data` chunk inside a RIFF/WAVE file.
fn pcm16_data(wav: &[u8]) -> Option<(usize, usize)> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let u16_at = |i: usize| Some(u16::from_le_bytes(wav.get(i..i + 2)?.try_into().ok()?));
    let u32_at = |i: usize| Some(u32::from_le_bytes(wav.get(i..i + 4)?.try_into().ok()?));
    let mut pos = 12;
    let mut pcm16 = false;
    while pos + 8 <= wav.len() {
        let id = &wav[pos..pos + 4];
        let len = u32_at(pos + 4)? as usize;
        let body = pos + 8;
        if id == b"fmt " {
            pcm16 = u16_at(body)? == 1 && u16_at(body + 14)? == 16;
        } else if id == b"data" {
            return pcm16.then_some((body, len.min(wav.len() - body)));
        }
        pos = body + len + (len & 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(wav: &[u8], i: usize) -> i16 {
        let (start, _) = pcm16_data(wav).unwrap();
        i16::from_le_bytes([wav[start + i * 2], wav[start + i * 2 + 1]])
    }

    #[test]
    fn bark_is_pcm16_and_scales() {
        let (_, len) = pcm16_data(BARK_WAV).expect("bark.wav must be 16-bit PCM");
        assert!(len > 1000);
        let loud = (0..len / 2)
            .max_by_key(|&i| sample(BARK_WAV, i).unsigned_abs())
            .unwrap();
        let full = sample(BARK_WAV, loud) as i32;
        let half = scaled_wav(BARK_WAV, 50).unwrap();
        assert_eq!(half.len(), BARK_WAV.len());
        assert_eq!(sample(&half, loud) as i32, full * 50 / 100);
        assert_eq!(scaled_wav(BARK_WAV, 100).unwrap(), BARK_WAV);
        assert!(scaled_wav(b"not a wav", 50).is_none());
    }
}
