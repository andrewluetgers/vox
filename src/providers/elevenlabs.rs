//! ElevenLabs provider. Uses the with-timestamps endpoint so karaoke gets
//! real character alignment instead of the estimated timing.

use super::{key_from_env, s16le_to_f32, words_from_char_times, Availability, Provider, SynthReq, WordEnd};
use anyhow::{bail, Context, Result};
use base64::Engine;
use std::time::Duration;

const ENV_KEY: &str = "ELEVENLABS_API_KEY";

/// Premade voices (stable public IDs). Any other value is passed through as
/// a raw voice ID, so cloned/library voices work by ID.
const VOICES: &[(&str, &str)] = &[
    ("rachel", "21m00Tcm4TlvDq8ikWAM"),
    ("adam", "pNInz6obpgDQGcFmaJgB"),
    ("antoni", "ErXwobaYiN019PkySvjV"),
    ("sarah", "EXAVITQu4vr4xnSDxMaL"),
    ("josh", "TxGEqnHWrfWFTfGW9XjX"),
    ("domi", "AZnzlk1XvdvUeBnXmlld"),
    ("elli", "MF3mGyEYCl7XYWbV9V6O"),
    ("sam", "yoZ06aMxZJJ28mfd3POQ"),
    ("george", "JBFqnCBsd6RMkjVDRZzb"),
    ("charlie", "IKne3meq5aSn9XLyUdCD"),
];

pub struct ElevenLabs;

impl Provider for ElevenLabs {
    fn name(&self) -> &'static str {
        "elevenlabs"
    }

    fn default_model(&self) -> &'static str {
        "eleven_flash_v2_5"
    }

    fn voices(&self) -> Vec<String> {
        VOICES.iter().map(|(n, _)| n.to_string()).collect()
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
        let voice_id = VOICES
            .iter()
            .find(|(n, _)| *n == req.voice.to_lowercase())
            .map(|(_, id)| *id)
            .unwrap_or(req.voice); // raw voice ID passthrough

        let url = format!(
            "https://api.elevenlabs.io/v1/text-to-speech/{voice_id}/with-timestamps?output_format=pcm_24000"
        );
        let mut body = serde_json::json!({
            "text": req.text,
            "model_id": req.model,
        });
        if (req.speed - 1.0).abs() > 0.01 {
            // ElevenLabs supports 0.7-1.2 via voice_settings
            body["voice_settings"] = serde_json::json!({"speed": req.speed.clamp(0.7, 1.2)});
        }
        let resp = ureq::post(&url)
            .set("xi-api-key", &key)
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(120))
            .send_string(&body.to_string())
            .map_err(|e| super::openai_compat::api_err("elevenlabs", e))?;
        let v: serde_json::Value =
            serde_json::from_reader(resp.into_reader()).context("elevenlabs: bad JSON")?;

        let audio_b64 = v["audio_base64"]
            .as_str()
            .context("elevenlabs: no audio_base64 in response")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(audio_b64)
            .context("elevenlabs: bad base64 audio")?;
        on_audio(&s16le_to_f32(&bytes));

        // alignment: parallel arrays of characters and their end times (secs)
        let words = (|| {
            let a = v.get("alignment")?;
            let chars: Vec<String> = a["characters"]
                .as_array()?
                .iter()
                .filter_map(|c| c.as_str().map(String::from))
                .collect();
            let times: Vec<f32> = a["character_end_times_seconds"]
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
