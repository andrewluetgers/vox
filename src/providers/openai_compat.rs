//! OpenAI-compatible speech endpoints: OpenAI itself and Groq (Orpheus).
//! One request per synth call. OpenAI streams raw s16le PCM at 24 kHz;
//! Groq returns a WAV file which we decode and resample.

use super::{key_from_env, s16le_to_f32, wav_to_f32, Availability, Provider, SynthReq, WordEnd};
use anyhow::{bail, Context, Result};
use std::io::Read;
use std::time::Duration;

pub struct OpenAiCompat {
    name: &'static str,
    url: &'static str,
    env_key: &'static str,
    default_model: &'static str,
    voices: &'static [&'static str],
    /// true: request response_format "pcm" and stream it; false: "wav" buffered
    pcm: bool,
    /// whether the endpoint accepts the `speed` field
    speed_param: bool,
}

pub fn openai() -> OpenAiCompat {
    OpenAiCompat {
        name: "openai",
        url: "https://api.openai.com/v1/audio/speech",
        env_key: "OPENAI_API_KEY",
        default_model: "gpt-4o-mini-tts",
        voices: &[
            "alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage",
            "shimmer", "verse",
        ],
        pcm: true,
        speed_param: true,
    }
}

pub fn groq() -> OpenAiCompat {
    OpenAiCompat {
        name: "groq",
        url: "https://api.groq.com/openai/v1/audio/speech",
        env_key: "GROQ_API_KEY",
        default_model: "canopylabs/orpheus-v1-english",
        voices: &["austin", "hannah", "troy"],
        pcm: false,
        speed_param: false,
    }
}

impl Provider for OpenAiCompat {
    fn name(&self) -> &'static str {
        self.name
    }

    fn default_model(&self) -> &'static str {
        self.default_model
    }

    fn voices(&self) -> Vec<String> {
        self.voices.iter().map(|v| v.to_string()).collect()
    }

    fn availability(&self) -> Availability {
        match key_from_env(self.env_key) {
            Some(_) => Availability::Ready,
            None => Availability::NeedsKey(self.env_key),
        }
    }

    fn synth(
        &self,
        req: &SynthReq,
        on_audio: &mut dyn FnMut(&[f32]) -> bool,
    ) -> Result<Option<Vec<WordEnd>>> {
        let Some(key) = key_from_env(self.env_key) else {
            bail!("{} not set", self.env_key);
        };
        let mut body = serde_json::json!({
            "model": req.model,
            "input": req.text,
            "voice": req.voice,
            "response_format": if self.pcm { "pcm" } else { "wav" },
        });
        if self.speed_param && (req.speed - 1.0).abs() > 0.01 {
            body["speed"] = serde_json::json!(req.speed.clamp(0.25, 4.0));
        }
        let resp = ureq::post(self.url)
            .set("Authorization", &format!("Bearer {key}"))
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(120))
            .send_string(&body.to_string())
            .map_err(|e| api_err(self.name, e))?;

        if self.pcm {
            // raw s16le at 24 kHz, streamed: convert as chunks arrive
            let mut reader = resp.into_reader();
            let mut buf = [0u8; 1 << 15];
            let mut carry: Option<u8> = None;
            loop {
                let n = reader.read(&mut buf).context("read audio stream")?;
                if n == 0 {
                    break;
                }
                let mut bytes = Vec::with_capacity(n + 1);
                if let Some(b) = carry.take() {
                    bytes.push(b);
                }
                bytes.extend_from_slice(&buf[..n]);
                if bytes.len() % 2 == 1 {
                    carry = bytes.pop();
                }
                if !on_audio(&s16le_to_f32(&bytes)) {
                    return Ok(None);
                }
            }
        } else {
            let mut bytes = Vec::new();
            resp.into_reader()
                .read_to_end(&mut bytes)
                .context("read audio response")?;
            let samples = wav_to_f32(&bytes)?;
            on_audio(&samples);
        }
        Ok(None) // no alignment on these endpoints; caller estimates
    }
}

/// Compact API error including the response body (which carries the actual
/// reason: bad key, unknown voice, quota).
pub fn api_err(provider: &str, e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let brief: String = body.chars().take(300).collect();
            anyhow::anyhow!("{provider}: HTTP {code}: {brief}")
        }
        other => anyhow::anyhow!("{provider}: {other}"),
    }
}
