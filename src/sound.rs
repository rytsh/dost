//! Dog sounds: the embedded bark, the user's own WAV files and a voice per event, scaled to
//! the requested volume. Playback is platform-specific (Windows: `PlaySoundW`, macOS:
//! `NSSound`); this module only prepares the data.

/// `assets/sounds/bark.wav`: small dog bark (sfxmint.com, CC0).
pub const BARK_WAV: &[u8] = include_bytes!("../assets/sounds/bark.wav");

/// Why the dog makes a sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Voice {
    /// News, a reminder, being clicked.
    Bark,
    /// Something finished well: a short, higher double bark.
    Happy,
    /// Something failed: a lower, drawn-out howl.
    Sad,
    /// A question waits for an answer: a short, rising "wuf?".
    Ask,
}

/// The sounds the dog can make, ready to be scaled and played.
#[derive(Clone, Debug)]
pub struct Sounds {
    /// The user's barks (or the built-in one).
    barks: Vec<Vec<u8>>,
    happy: Vec<u8>,
    sad: Vec<u8>,
    ask: Vec<u8>,
    rng: fastrand::Rng,
}

impl Default for Sounds {
    fn default() -> Self {
        Sounds::new(&[])
    }
}

impl Sounds {
    /// `files`: the user's WAV files; ones that cannot be read or are not 16-bit PCM are
    /// skipped (the built-in bark is used when none is left). Event voices are made from
    /// the first bark.
    pub fn new(files: &[String]) -> Self {
        let mut barks: Vec<Vec<u8>> = files
            .iter()
            .filter_map(|f| std::fs::read(expand_home(f)).ok())
            .filter(|d| pcm16_data(d).is_some())
            .collect();
        if barks.is_empty() {
            barks.push(BARK_WAV.to_vec());
        }
        let base = barks[0].clone();
        Sounds {
            happy: double(&resample(&base, 1.25)),
            sad: howl(&base),
            ask: rising(&base),
            barks,
            rng: fastrand::Rng::new(),
        }
    }

    /// Reports the files that could not be used (missing or not 16-bit PCM WAV).
    pub fn bad_files(files: &[String]) -> Vec<String> {
        files
            .iter()
            .filter(|f| {
                std::fs::read(expand_home(f))
                    .ok()
                    .is_none_or(|d| pcm16_data(&d).is_none())
            })
            .cloned()
            .collect()
    }

    /// The WAV for `voice` at `volume` percent. Without `events`, every voice is a bark.
    pub fn wav(&mut self, voice: Voice, volume: u8, events: bool) -> Option<Vec<u8>> {
        let data = match voice {
            Voice::Happy if events => &self.happy,
            Voice::Sad if events => &self.sad,
            Voice::Ask if events => &self.ask,
            _ => {
                let i = self.rng.usize(..self.barks.len());
                &self.barks[i]
            }
        };
        scaled_wav(data, volume)
    }
}

fn expand_home(path: &str) -> std::path::PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    match (path.strip_prefix("~/").or(path.strip_prefix("~\\")), home) {
        (Some(rest), Some(h)) => std::path::PathBuf::from(h).join(rest),
        _ => std::path::PathBuf::from(path),
    }
}

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
    pcm16(wav).map(|w| (w.data, w.len))
}

struct Pcm {
    channels: u16,
    rate: u32,
    data: usize,
    len: usize,
}

fn pcm16(wav: &[u8]) -> Option<Pcm> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let u16_at = |i: usize| Some(u16::from_le_bytes(wav.get(i..i + 2)?.try_into().ok()?));
    let u32_at = |i: usize| Some(u32::from_le_bytes(wav.get(i..i + 4)?.try_into().ok()?));
    let mut pos = 12;
    let mut fmt = None;
    while pos + 8 <= wav.len() {
        let id = &wav[pos..pos + 4];
        let len = u32_at(pos + 4)? as usize;
        let body = pos + 8;
        if id == b"fmt " {
            let pcm = u16_at(body)? == 1 && u16_at(body + 14)? == 16;
            fmt = pcm.then_some((u16_at(body + 2)?.max(1), u32_at(body + 4)?));
        } else if id == b"data" {
            let (channels, rate) = fmt?;
            return Some(Pcm {
                channels,
                rate,
                data: body,
                len: len.min(wav.len() - body),
            });
        }
        pos = body + len + (len & 1);
    }
    None
}

/// Samples (interleaved) and format of a 16-bit WAV.
fn samples(wav: &[u8]) -> Option<(Vec<i16>, u16, u32)> {
    let p = pcm16(wav)?;
    let s = wav[p.data..p.data + p.len]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect();
    Some((s, p.channels, p.rate))
}

/// A 16-bit PCM WAV with these samples.
fn encode(samples: &[i16], channels: u16, rate: u32) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Reads frame `t` (fractional) of channel `c`, linearly interpolated.
fn at(s: &[i16], ch: usize, t: f32, c: usize) -> f32 {
    let frames = s.len() / ch;
    let i = t as usize;
    if i + 1 >= frames {
        return 0.0;
    }
    let f = t - i as f32;
    s[i * ch + c] as f32 * (1.0 - f) + s[(i + 1) * ch + c] as f32 * f
}

/// Plays `wav` `speed` times faster (higher and shorter, or lower and longer). The pitch
/// may also glide: `speed` at the start to `end` at the end.
fn glide(wav: &[u8], speed: f32, end: f32) -> Option<Vec<i16>> {
    let (s, ch, _) = samples(wav)?;
    let ch = ch as usize;
    let frames = s.len() / ch;
    let mut out = Vec::new();
    let mut t = 0.0f32;
    while (t as usize) + 1 < frames {
        let k = speed + (end - speed) * (t / frames as f32);
        for c in 0..ch {
            out.push(at(&s, ch, t, c).round() as i16);
        }
        t += k;
    }
    Some(out)
}

fn resample(wav: &[u8], speed: f32) -> Vec<u8> {
    match (glide(wav, speed, speed), pcm16(wav)) {
        (Some(s), Some(p)) => encode(&s, p.channels, p.rate),
        _ => wav.to_vec(),
    }
}

/// Two short yips in a row.
fn double(wav: &[u8]) -> Vec<u8> {
    let Some((s, ch, rate)) = samples(wav) else {
        return wav.to_vec();
    };
    let gap = vec![0i16; (rate as usize / 14) * ch as usize];
    let short = &s[..s.len() * 3 / 4 / ch as usize * ch as usize];
    let out: Vec<i16> = [short, &gap[..], short].concat();
    encode(&out, ch, rate)
}

/// Lower and longer, sliding down, with a soft fade: a little howl.
fn howl(wav: &[u8]) -> Vec<u8> {
    let (Some(mut s), Some(p)) = (glide(wav, 0.72, 0.55), pcm16(wav)) else {
        return wav.to_vec();
    };
    let n = s.len();
    for (i, v) in s.iter_mut().enumerate() {
        let fade = 1.0 - (i as f32 / n as f32).powi(2);
        *v = (*v as f32 * fade) as i16;
    }
    encode(&s, p.channels, p.rate)
}

/// Short and sliding up, like a question.
fn rising(wav: &[u8]) -> Vec<u8> {
    match (glide(wav, 1.0, 1.45), pcm16(wav)) {
        (Some(s), Some(p)) => {
            let keep = s.len() * 2 / 3 / p.channels as usize * p.channels as usize;
            encode(&s[..keep], p.channels, p.rate)
        }
        _ => wav.to_vec(),
    }
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

    #[test]
    fn event_voices() {
        let mut s = Sounds::new(&[]);
        let len = |w: &[u8]| pcm16_data(w).unwrap().1;
        let bark = s.wav(Voice::Bark, 100, true).unwrap();
        assert_eq!(bark, BARK_WAV);
        let sad = s.wav(Voice::Sad, 100, true).unwrap();
        let ask = s.wav(Voice::Ask, 100, true).unwrap();
        let happy = s.wav(Voice::Happy, 100, true).unwrap();
        assert!(len(&sad) > len(&bark), "the howl is longer");
        assert!(len(&ask) < len(&bark), "the question is short");
        assert!(len(&happy) > len(&bark) / 2);
        // Off: always the bark.
        assert_eq!(s.wav(Voice::Sad, 100, false).unwrap(), BARK_WAV);
    }

    #[test]
    fn user_files() {
        let dir = std::env::temp_dir().join(format!("dost-sound-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("woof.wav");
        let bad = dir.join("bad.wav");
        let (s, ch, rate) = samples(BARK_WAV).unwrap();
        std::fs::write(&good, encode(&s[..s.len() / 2], ch, rate)).unwrap();
        std::fs::write(&bad, b"nope").unwrap();
        let files: Vec<String> = [&good, &bad, &dir.join("missing.wav")]
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let mut sounds = Sounds::new(&files);
        assert_eq!(sounds.barks.len(), 1);
        assert_ne!(sounds.wav(Voice::Bark, 100, true).unwrap(), BARK_WAV);
        assert_eq!(Sounds::bad_files(&files).len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
