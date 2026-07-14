//! TTS provider abstraction: kokoro (local) plus cloud providers, all
//! normalized to one contract — f32 mono samples at player::SAMPLE_RATE
//! delivered through a callback, with optional word timings for karaoke.
//!
//! Voices are referenced as path strings so they flow unchanged through
//! config.toml, .vox.json, state.json, VOX_VOICE, and the CLI:
//!   "bm_george"                          -> kokoro, default model
//!   "openai/nova"                        -> provider default model
//!   "openai/tts-1-hd/nova"               -> explicit model
//!   "groq/canopylabs/orpheus-v1-english/austin"  (models may contain '/')

pub mod elevenlabs;
pub mod kokoro;
pub mod openai_compat;
pub mod xai;

use crate::player::SAMPLE_RATE;
use anyhow::Result;

/// A word's end position, in samples relative to the start of the synth call.
pub struct WordEnd {
    pub text: String,
    pub end: u64,
}

pub enum Availability {
    Ready,
    /// Provider works once this environment variable holds an API key.
    NeedsKey(&'static str),
}

pub struct SynthReq<'a> {
    pub text: &'a str,
    pub model: &'a str,
    pub voice: &'a str,
    pub speed: f32,
}

pub trait Provider: Send {
    fn name(&self) -> &'static str;
    fn default_model(&self) -> &'static str;
    /// Known voices, for pickers and --list-voices. Cloud lists may be
    /// non-exhaustive; unknown names are passed through to the API.
    fn voices(&self) -> Vec<String>;
    fn availability(&self) -> Availability;
    /// Synthesize one chunk of text. `on_audio` receives f32 mono samples at
    /// SAMPLE_RATE as they arrive; returning false cancels. Returns word
    /// timings when the provider supplies alignment, None for "estimate".
    fn synth(
        &self,
        req: &SynthReq,
        on_audio: &mut dyn FnMut(&[f32]) -> bool,
    ) -> Result<Option<Vec<WordEnd>>>;
}

/// The cloud provider set (kokoro is constructed separately since it owns
/// the loaded ONNX model).
pub fn cloud_providers() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(openai_compat::openai()),
        Box::new(elevenlabs::ElevenLabs),
        Box::new(xai::Xai),
        Box::new(openai_compat::groq()),
    ]
}

/// A parsed voice reference. `provider/[model/]voice`; bare names are kokoro.
#[derive(Debug, PartialEq)]
pub struct VoicePath {
    pub provider: String,
    pub model: Option<String>,
    pub voice: String,
}

impl VoicePath {
    pub fn parse(s: &str) -> VoicePath {
        let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
        match parts.len() {
            0 | 1 => VoicePath {
                provider: "kokoro".into(),
                model: None,
                voice: parts.first().unwrap_or(&"").to_string(),
            },
            2 => VoicePath {
                provider: parts[0].into(),
                model: None,
                voice: parts[1].into(),
            },
            // first = provider, last = voice, middle (may contain '/') = model
            n => VoicePath {
                provider: parts[0].into(),
                model: Some(parts[1..n - 1].join("/")),
                voice: parts[n - 1].into(),
            },
        }
    }
}

/// Split text into sentences (bounded length) for incremental synthesis.
/// Shared by the TUI worker and the one-shot CLI path.
pub fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        cur.push(ch);
        if matches!(ch, '.' | '!' | '?' | ';' | ':' | '\n') || cur.len() > 300 {
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
            }
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

// ---- audio helpers shared by the cloud providers ----

/// Little-endian signed 16-bit PCM to f32. Ignores a trailing odd byte
/// (callers carry it into the next chunk).
pub fn s16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect()
}

/// Linear resampler, good enough for speech rate conversion.
pub fn resample(samples: &[f32], from_hz: u32, to_hz: u32) -> Vec<f32> {
    if from_hz == to_hz || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from_hz as f64 / to_hz as f64;
    let out_len = (samples.len() as f64 / ratio).floor() as usize;
    (0..out_len)
        .map(|i| {
            let src = i as f64 * ratio;
            let i0 = src as usize;
            let frac = (src - i0 as f64) as f32;
            let a = samples[i0.min(samples.len() - 1)];
            let b = samples[(i0 + 1).min(samples.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// Decode a WAV file from memory to f32 mono at SAMPLE_RATE.
pub fn wav_to_f32(bytes: &[u8]) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes))?;
    let spec = reader.spec();
    let mono: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .filter_map(|s| s.ok())
                .map(|s| s as f32 / max)
                .collect()
        }
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(|s| s.ok()).collect(),
    };
    // average channels down to mono
    let mono = if spec.channels > 1 {
        mono.chunks(spec.channels as usize)
            .map(|c| c.iter().sum::<f32>() / c.len() as f32)
            .collect()
    } else {
        mono
    };
    Ok(resample(&mono, spec.sample_rate, SAMPLE_RATE))
}

/// Map per-character times (seconds, one entry per character of the spoken
/// text) to word end positions in samples. Used for ElevenLabs alignment and
/// xAI graph_chars/graph_times. `times[i]` is the time at character `i`; a
/// word ends at the time of its last character (clamped non-decreasing).
pub fn words_from_char_times(chars: &[String], times: &[f32]) -> Vec<WordEnd> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut cur_end = 0f32;
    let mut last_end = 0f32;
    for (i, ch) in chars.iter().enumerate() {
        let t = times.get(i).copied().unwrap_or(last_end);
        if ch.trim().is_empty() {
            if !cur.is_empty() {
                last_end = cur_end.max(last_end);
                words.push(WordEnd {
                    text: std::mem::take(&mut cur),
                    end: (last_end * SAMPLE_RATE as f32) as u64,
                });
            }
        } else {
            cur.push_str(ch);
            cur_end = t;
        }
    }
    if !cur.is_empty() {
        last_end = cur_end.max(last_end);
        words.push(WordEnd {
            text: cur,
            end: (last_end * SAMPLE_RATE as f32) as u64,
        });
    }
    words
}

pub fn key_from_env(var: &'static str) -> Option<String> {
    std::env::var(var).ok().filter(|k| !k.trim().is_empty())
}

/// Boil a provider error down to a menu-sized reason: prefer the API's
/// error code/type token ("insufficient_quota" -> "insufficient quota"),
/// then the HTTP status, then a truncated message.
pub fn short_reason(msg: &str) -> String {
    for key in ["\"code\":", "\"type\":", "\"status\":"] {
        if let Some(i) = msg.find(key) {
            let rest = &msg[i + key.len()..];
            if let Some(start) = rest.find('"') {
                let rest = &rest[start + 1..];
                if let Some(end) = rest.find('"') {
                    let token = &rest[..end];
                    if !token.is_empty() && token.len() < 40 && token != "error" {
                        return token.replace('_', " ");
                    }
                }
            }
        }
    }
    if let Some(i) = msg.find("HTTP ") {
        let code: String = msg[i + 5..].chars().take_while(|c| c.is_ascii_digit()).collect();
        if !code.is_empty() {
            return format!("HTTP {code}");
        }
    }
    let mut s: String = msg.chars().take(50).collect();
    if msg.chars().count() > 50 {
        s.push('…');
    }
    s
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Human label for a voice. Kokoro names encode accent and gender in their
/// prefix, so "bm_george" reads as "British male · George"; cloud voices are
/// just capitalized.
pub fn voice_label(provider: &str, voice: &str) -> String {
    if provider == "kokoro" {
        let class = match voice.get(..2) {
            Some("bm") => "British male",
            Some("bf") => "British female",
            Some("am") => "American male",
            Some("af") => "American female",
            _ => "",
        };
        let name = capitalize(voice.split('_').nth(1).unwrap_or(voice));
        if class.is_empty() {
            name
        } else {
            format!("{class} · {name}")
        }
    } else {
        capitalize(voice)
    }
}

/// Display name for a provider section header.
pub fn provider_label(name: &str) -> String {
    match name {
        "kokoro" => "Kokoro-82M · local".into(),
        "openai" => "OpenAI · cloud".into(),
        "elevenlabs" => "ElevenLabs · cloud".into(),
        "xai" => "xAI Grok · cloud".into(),
        "groq" => "Groq · cloud".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_path_bare_name_is_kokoro() {
        assert_eq!(
            VoicePath::parse("bm_george"),
            VoicePath { provider: "kokoro".into(), model: None, voice: "bm_george".into() }
        );
    }

    #[test]
    fn voice_path_provider_and_voice() {
        assert_eq!(
            VoicePath::parse("openai/nova"),
            VoicePath { provider: "openai".into(), model: None, voice: "nova".into() }
        );
    }

    #[test]
    fn voice_path_with_model() {
        assert_eq!(
            VoicePath::parse("openai/tts-1-hd/nova"),
            VoicePath {
                provider: "openai".into(),
                model: Some("tts-1-hd".into()),
                voice: "nova".into()
            }
        );
    }

    #[test]
    fn voice_path_model_containing_slashes() {
        assert_eq!(
            VoicePath::parse("groq/canopylabs/orpheus-v1-english/austin"),
            VoicePath {
                provider: "groq".into(),
                model: Some("canopylabs/orpheus-v1-english".into()),
                voice: "austin".into()
            }
        );
    }

    #[test]
    fn s16le_conversion_and_odd_byte() {
        let bytes = [0x00, 0x40, 0xFF]; // 0x4000 = 16384 -> 0.5, trailing byte ignored
        let out = s16le_to_f32(&bytes);
        assert_eq!(out.len(), 1);
        assert!((out[0] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn resample_halves_and_keeps_rate() {
        let s: Vec<f32> = (0..480).map(|i| i as f32).collect();
        assert_eq!(resample(&s, 48000, 24000).len(), 240);
        assert_eq!(resample(&s, 24000, 24000).len(), 480);
    }

    #[test]
    fn wav_roundtrip_resamples_to_player_rate() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = hound::WavWriter::new(&mut buf, spec).unwrap();
            for i in 0..4800 {
                w.write_sample(((i % 100) * 300) as i16).unwrap();
            }
            w.finalize().unwrap();
        }
        let out = wav_to_f32(buf.get_ref()).unwrap();
        assert_eq!(out.len(), 2400); // 0.1s at 24k
    }

    #[test]
    fn char_times_group_into_words() {
        let chars: Vec<String> = "hi yo".chars().map(|c| c.to_string()).collect();
        let times = [0.1, 0.2, 0.25, 0.3, 0.4];
        let words = words_from_char_times(&chars, &times);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, "hi");
        assert_eq!(words[0].end, (0.2 * SAMPLE_RATE as f32) as u64);
        assert_eq!(words[1].text, "yo");
        assert_eq!(words[1].end, (0.4 * SAMPLE_RATE as f32) as u64);
    }

    #[test]
    fn sentences_split_and_bound() {
        let v = sentences("One. Two! Three");
        assert_eq!(v, vec!["One.", "Two!", "Three"]);
    }

    #[test]
    fn short_reason_prefers_code_then_type_then_http() {
        let openai = r#"openai: HTTP 429: {"error": {"message": "You exceeded your current quota", "type": "insufficient_quota", "param": null, "code": "insufficient_quota"}}"#;
        assert_eq!(short_reason(openai), "insufficient quota");
        let groq = r#"groq: HTTP 400: {"error":{"message":"The model requires terms acceptance.","type":"invalid_request_error","code":"model_terms_required"}}"#;
        assert_eq!(short_reason(groq), "model terms required");
        assert_eq!(short_reason("xai: HTTP 503: upstream busy"), "HTTP 503");
        assert_eq!(short_reason("connection refused"), "connection refused");
    }

    #[test]
    fn voice_labels_read_as_words() {
        assert_eq!(voice_label("kokoro", "bm_george"), "British male · George");
        assert_eq!(voice_label("kokoro", "af_heart"), "American female · Heart");
        assert_eq!(voice_label("openai", "nova"), "Nova");
        assert_eq!(voice_label("xai", "ara"), "Ara");
    }
}
