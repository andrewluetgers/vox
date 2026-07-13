# Multi-provider voice architecture (design)

Goal: vox speaks through **providers → models → voices**, configured in the
same settings system (global state.json, per-repo .vox.json, tray GUI), with
API tokens from standard environment variables and graceful fallback when a
voice can't be resolved.

## Schema

A voice reference is `provider:model:voice` (string form) or the expanded
object. Everything today ("bm_george") is shorthand for
`kokoro:kokoro-v1.0:bm_george`.

```jsonc
// settings (state.json / .vox.json) — unchanged keys plus:
{
  "voice": "elevenlabs:eleven_turbo_v2_5:Rachel",   // or bare "bm_george"
  "fallback_voice": "kokoro:kokoro-v1.0:bm_george"  // optional; see Fallback
}
```

```jsonc
// providers registry (~/.claude/vox/providers.json, managed by vox + tray)
{
  "kokoro": {
    "type": "local",
    "models": {
      "kokoro-v1.0": {
        "status": "installed",            // none | downloading (pct) | installed
        "size_mb": 350,
        "languages": ["en-us", "en-gb", "es", "fr", "hi", "it", "ja", "pt-br", "zh"],
        "voices": ["bm_george", "bf_emma", "ef_dora", "..."]
      },
      "kokoro-v1.0-int8": { "status": "none", "size_mb": 92 }
    }
  },
  "macos":      { "type": "system" },      // voices discovered via `say -v ?`
  "openai":     { "type": "cloud", "api_key_env": "OPENAI_API_KEY" },
  "elevenlabs": { "type": "cloud", "api_key_env": "ELEVENLABS_API_KEY" },
  "azure":      { "type": "cloud", "api_key_env": "AZURE_SPEECH_KEY" },
  "google":     { "type": "cloud", "api_key_env": "GOOGLE_APPLICATION_CREDENTIALS" },
  "polly":      { "type": "cloud", "api_key_env": "AWS_ACCESS_KEY_ID" },
  "cartesia":   { "type": "cloud", "api_key_env": "CARTESIA_API_KEY" },
  "deepgram":   { "type": "cloud", "api_key_env": "DEEPGRAM_API_KEY" }
}
```

## Providers

**Local / system (zero or one download):**

- **kokoro** (current engine). One important finding: the v1.0 voices file
  vox already downloads contains *all* language voice packs — Spanish
  (`ef_dora`, `em_alex`, `em_santa`), French (`ff_siwis`), Japanese, Chinese,
  Hindi, Italian, Portuguese. "Download the Spanish one" is mostly *unlock*,
  not download: allow the voice names and map the right espeak-ng language
  code in `lang_for()` (espeak-ng data is already bundled). The int8 model
  and future v1.1 weights are real downloads → the status/progress field.
- **macos** (`say` / AVSpeechSynthesizer). The true zero-download default:
  works on every Mac with no model fetch. Caveat found in research: **Siri
  voices are not available to third-party apps or `say`** — Apple restricts
  them. What we get are the system voices, including the high-quality
  "Enhanced/Premium" ones the user can add in System Settings → Accessibility
  → Spoken Content. `say -v '?'` lists them; `say -v Samantha --file-format`
  renders to file. Good fallback-of-last-resort and instant-start option.

**Cloud (token via env var, standard names):**

| Provider | Env var | Notes |
|---|---|---|
| OpenAI | `OPENAI_API_KEY` | `gpt-4o-mini-tts`, `tts-1`, `tts-1-hd`; 13 voices (alloy, nova, …); streaming |
| ElevenLabs | `ELEVENLABS_API_KEY` | best-in-class voices, voice IDs per account; websocket streaming |
| Azure Speech | `AZURE_SPEECH_KEY` + `AZURE_SPEECH_REGION` | huge language coverage |
| Google Cloud TTS | `GOOGLE_APPLICATION_CREDENTIALS` | WaveNet/Neural2/Chirp voices |
| Amazon Polly | `AWS_ACCESS_KEY_ID` / secret / region | neural + long-form |
| Cartesia | `CARTESIA_API_KEY` | Sonic — lowest-latency streaming, dev-friendly |
| Deepgram | `DEEPGRAM_API_KEY` | Aura — fast, cheap |
| PlayHT / Groq | `PLAYHT_*` / `GROQ_API_KEY` | PlayAI voices; Groq hosts them cheap |

**Anthropic has no TTS API** (as of early 2026) — listed here so nobody
looks for `ANTHROPIC_API_KEY` TTS support later.

## Resolution & fallback

Resolving `provider:model:voice` at speak time:

1. Provider unknown/disabled → fallback chain.
2. `type: cloud` and env var unset/empty → fallback (and surface why in
   `status`/tray badge: "ELEVENLABS_API_KEY not set").
3. `type: local` and model `status != installed` → fallback; tray offers
   the download.
4. Success → speak.

Fallback chain: `fallback_voice` setting → `kokoro:…:bm_george` if installed
→ `macos:say:<default system voice>`. The chain ends at macos because it
requires nothing. Every fallback is logged to history with the voice that
actually spoke.

## Download manager

Generalize `vox --setup` into a model registry: `vox models list|install|rm`,
each model with status none/downloading(pct)/installed, size, and languages.
Tray Settings gains a **Providers** section: provider list with key status
(env var found: yes/no), per-model install buttons with progress, and the
voice picker grouped provider → model → voice (with the existing
speak-a-sample preview on selection).

## Staging

1. Engine trait in vox core (`Synth`: speak(text, voice, speed) → stream +
   wav), Kokoro and `say` implementations; resolution + fallback.
2. Unlock Kokoro's non-English voices (lang_for map + voice list) — cheap win.
3. Providers registry + `vox models` + tray Providers section.
4. Cloud providers, one at a time: OpenAI first (simplest API), then
   ElevenLabs, then Cartesia/Deepgram. Streaming into the existing player so
   transport controls keep working.
