# Providers, models, and voices — design

Status: **first wave implemented (v0.8.0)** — kokoro, openai, elevenlabs,
xai, groq, with provider timestamps feeding karaoke and voice-path config.
One simplification vs. the original proposal: voice references are **path
strings** (`provider/model/voice`), not structured JSON — one syntax works in
config.toml, .vox.json, state.json, VOX_VOICE, and the CLI.

## The schema

Three levels, each owning the one below:

```
provider ─── where synthesis happens (kokoro, macos, openai, elevenlabs, …)
  └── model ─── a specific engine/quality tier, possibly a download
        └── voice ─── a named speaker within that model
```

A configured voice is always a full path: `provider / model / voice`
(e.g. `kokoro/v1.0/bm_george`, `macos/system/Samantha`,
`elevenlabs/eleven_flash_v2_5/Rachel`). Settings store the structured
reference; bare voice names (current behavior) resolve against the default
provider for backwards compatibility.

### Resolution and fallback

If a configured voice can't be resolved (provider unavailable, model not
downloaded, API key missing, network down), fall back down this chain and
surface a one-line warning:

1. The configured `provider/model/voice`.
2. That provider's default voice.
3. The bundled local default: `kokoro/v1.0/bm_george` (downloads on first use,
   as today).

(A `macos/say` zero-download floor was considered and dropped — Kokoro's
first-use download remains the baseline, as today.)

## Providers

### Local: `kokoro` (current engine)

What we ship today, made explicit. Model state becomes first-class:

- **Where models come from**: the kokoro-onnx GitHub release (see
  `RELEASE_BASE` in `src/main.rs`). Cached in `~/Library/Caches/vox/`.
- **Download state**: each model is `not-downloaded | downloading(pct) |
  ready`. The existing `download()` progress becomes visible in both UIs
  instead of only stderr.
- **Model variants**: `kokoro-v1.0.onnx` (fp32, default) and
  `kokoro-v1.0.int8.onnx` (smaller, slower on Apple Silicon) — already
  selectable via `VOX_MODEL_FILE`; expose as models `v1.0` and `v1.0-int8`.

**Languages: no separate download needed.** "The Spanish one" is not another
model — `voices-v1.0.bin` already contains all Kokoro voices across 8
languages (English `af/am/bf/bm`, Spanish `ef/em`, French `ff`, Hindi
`hf/hm`, Italian `if/im`, Japanese `jf/jm`, Chinese `zf/zm`, Portuguese
`pf/pm`), and the espeak-ng data for those languages is already installed.
Enabling them means: (1) extend `VOICE_NAMES` beyond the 12 English voices,
(2) extend `lang_for()` (src/main.rs) to map each prefix to the right
espeak-ng language instead of hardcoding en-gb/en-us. Caveat: ja/zh need
different phonemization paths — verify what the kokoros crate supports before
promising them; es/fr/it/pt should be straightforward espeak mappings.

### Cloud providers

Anthropic has **no TTS API** (text-only output — verified against current API
docs), so it's off the list despite coming up repeatedly. First wave
(chosen 2026-07-13): **OpenAI, ElevenLabs, xAI, Groq.**

| Provider | Models | Env var | Notes |
|---|---|---|---|
| OpenAI | `gpt-4o-mini-tts`, `tts-1`, `tts-1-hd` | `OPENAI_API_KEY` | `/v1/audio/speech`, streams raw PCM |
| ElevenLabs | `eleven_flash_v2_5` (fast), `eleven_multilingual_v2` | `ELEVENLABS_API_KEY` | Best voice variety; `with-timestamps` gives char alignment |
| xAI | `grok-tts` | `XAI_API_KEY` | `POST /v1/tts` (shipped Apr 2026); expressive tags ([laugh], <whisper>); voices via `GET /v1/tts/voices`; $4.20/1M chars |
| Groq | `playai-tts` | `GROQ_API_KEY` | OpenAI-compatible speech endpoint; very fast; $50/1M chars |

Deferred: Cartesia, Deepgram (easy adds later); Google/Polly/Azure
(service-account auth doesn't fit the env-var model).

**Keys come from environment variables only** — the standard var per provider,
checked at synthesis time. Never stored in `state.json` or `.vox.json` (both
are plain-text files that get committed/synced). Settings UI shows key status
as detected/missing per provider ("`ELEVENLABS_API_KEY` ✓ set") but never the
value, and never offers to save one. A provider without its env var appears
grayed out with the var name as the hint.

## Configuration

`state.json` (and `.vox.json` per-repo overrides) gain:

```json
{
  "voice": { "provider": "kokoro", "model": "v1.0", "voice": "bm_george" }
}
```

- Legacy string form `"voice": "bm_george"` remains valid → resolves as
  `kokoro/v1.0/<name>`.
- "Reset" semantics unchanged: delete the key, fall back to in-code default.
- Per-provider default voices live in code, not config.

## Architecture sketch

A `Provider` trait in `src/` roughly:

```rust
trait TtsProvider {
    fn id(&self) -> &str;
    fn models(&self) -> Vec<ModelInfo>;        // incl. download state
    fn voices(&self, model: &str) -> Vec<VoiceInfo>;
    fn available(&self) -> Availability;        // Ready | NeedsDownload | NeedsKey(var) | Unavailable
    fn synth_stream(&self, req: SynthRequest) -> Result<AudioStream>;
}
```

Everything downstream (playback, transport controls, karaoke timing, history
logging, last-spoken.txt) stays provider-agnostic.

### Feature compatibility — why everything keeps working

The contract every provider must meet is narrow: **deliver f32 PCM samples,
sentence by sentence, into the shared Player buffer.** All vox features hang
off that buffer, not off the engine:

- **Transport (pause / seek / scrub / pitch-preserving speed)** operates on
  buffered samples — provider-independent by construction. The synthesis
  frontier (skips capped at what's been synthesized) is just "how much PCM
  has arrived," same for a cloud stream as for local ONNX.
- **Karaoke word timing**: today an estimate — word boundaries interpolated
  by character weight within each sentence's sample span (`src/tui.rs`). Any
  provider that returns audio per sentence gets at least this behavior. When
  a provider supplies real timestamps (see below), those are used instead.
- **WAV save** (`save_wav`, `--out`, audio-dir TTL copies) writes the buffer
  via hound — works for every provider unchanged.
- **History / last-spoken / `vox --stop`** never touch audio internals.

Per-provider notes on meeting the contract:

| Provider | How we get PCM | Caveat |
|---|---|---|
| kokoro | in-process ONNX (today) | — |
| OpenAI | `response_format: "pcm"` — raw 24 kHz s16 streamed | convert s16→f32 |
| ElevenLabs | `pcm_24000` streaming (`with-timestamps` variant for word timing) | s16→f32 |
| xAI | `POST /v1/tts` audio stream | request PCM/WAV format |
| Groq | OpenAI-compatible speech endpoint, `response_format: "wav"` | parse WAV header, feed samples |

### Provider timestamps (karaoke upgrade)

The synthesis stream carries optional word timings alongside audio:

```rust
struct WordTiming { text_range: Range<usize>, end_sample: u64 }
enum Timing { Estimated, Provider(Vec<WordTiming>) }
```

- Providers that expose alignment (ElevenLabs `stream/with-timestamps`
  returns character-level alignment; others as available) map it to
  `end_sample` positions in the buffer — the exact shape the karaoke
  renderer already consumes.
- Providers without alignment return `Estimated`, and the TUI keeps the
  current character-weight interpolation. Same rendering path either way;
  only the source of `Word.end_sample` changes.

One real plumbing item: `player::SAMPLE_RATE` is a compile-time constant.
Providers emit 22.05/24/44.1 kHz — either resample everything to the player
rate or make the rate per-utterance. Request PCM/WAV formats everywhere so we
never need an MP3 decoder.

## UI (parity rule applies)

- **Tray**: Voice submenu grows provider sections; header already names the
  model — becomes `provider · model`. "More voice settings…" deep-links the
  panel Settings, which gains a **Providers** area: per-provider status
  (ready / needs download with progress / needs `ENV_VAR`), model download
  buttons, voice picker with spoken samples (existing behavior).
- **TUI**: Tab-settings voice field becomes a provider → model → voice
  cascade (or a single flattened picker with `provider/model/voice` rows —
  decide during implementation, keep it one popup). Download progress shows
  in the status line, as first-run download does today.

## Out of scope / open questions

- OS-level "download the Spanish one" phrasing → resolved: same model, more
  voices; only UI exposure needed.
- STT/voice input — different feature entirely.
- Whether summarizer prompt should vary per provider — no; summarization is
  orthogonal (it's the Claude hook), providers only change synthesis.
- Cost display for cloud providers (per-char pricing) — nice-to-have, later.
- ja/zh phonemization support in kokoros — needs a spike.
