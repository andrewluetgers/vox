//! xAI (Grok) TTS provider. POST /v1/tts returns JSON with base64 audio and,
//! when requested, per-character timing — real karaoke alignment.

use super::{key_from_env, s16le_to_f32, words_from_char_times, Availability, Provider, SynthReq, WordEnd};
use crate::player::SAMPLE_RATE;
use anyhow::{bail, Context, Result};
use base64::Engine;
use std::time::Duration;

const ENV_KEY: &str = "XAI_API_KEY";

pub struct Xai;

impl Provider for Xai {
    fn name(&self) -> &'static str {
        "xai"
    }

    fn default_model(&self) -> &'static str {
        "grok-tts"
    }

    fn voices(&self) -> Vec<String> {
        // built-in roster; custom voice IDs pass through
        vec!["ara".into(), "eve".into(), "leo".into()]
    }

    fn availability(&self) -> Availability {
        match key_from_env(ENV_KEY) {
            Some(_) => Availability::Ready,
            None => Availability::NeedsKey(ENV_KEY),
        }
    }

    fn synth(
        &self,
        req: &SynthReq,
        on_audio: &mut dyn FnMut(&[f32]) -> bool,
    ) -> Result<Option<Vec<WordEnd>>> {
        let Some(key) = key_from_env(ENV_KEY) else {
            bail!("{ENV_KEY} not set");
        };
        let body = serde_json::json!({
            "text": req.text,
            "voice_id": req.voice,
            "language": "auto",
            "output_format": { "codec": "pcm", "sample_rate": SAMPLE_RATE },
            "speed": req.speed.clamp(0.5, 2.0),
            "with_timestamps": true,
        });
        let resp = ureq::post("https://api.x.ai/v1/tts")
            .set("Authorization", &format!("Bearer {key}"))
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(120))
            .send_string(&body.to_string())
            .map_err(|e| super::openai_compat::api_err("xai", e))?;
        let v: serde_json::Value =
            serde_json::from_reader(resp.into_reader()).context("xai: bad JSON")?;

        let audio_b64 = v["audio"].as_str().context("xai: no audio in response")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(audio_b64)
            .context("xai: bad base64 audio")?;
        on_audio(&s16le_to_f32(&bytes));

        // audio_timestamps: { graph_chars: [".."], graph_times: [secs] }
        let words = (|| {
            let ts = v.get("audio_timestamps")?;
            let chars: Vec<String> = ts["graph_chars"]
                .as_array()?
                .iter()
                .filter_map(|c| c.as_str().map(String::from))
                .collect();
            let times: Vec<f32> = ts["graph_times"]
                .as_array()?
                .iter()
                .filter_map(|t| t.as_f64().map(|f| f as f32))
                .collect();
            if chars.is_empty() || chars.len() != times.len() {
                return None;
            }
            Some(words_from_char_times(&chars, &times))
        })();
        Ok(words)
    }
}
