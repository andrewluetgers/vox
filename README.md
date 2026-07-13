# vox

Local streaming text-to-speech CLI in a single Rust binary. Kokoro-82M via
ONNX Runtime — no cloud, no API keys, no Python. First word in ~1–3 seconds,
synthesis 2.5–3.6× faster than realtime on an M1 Pro.

Default voice is `bm_george`, a mature British male.

## Install

```sh
git clone https://github.com/andrewluetgers/vox.git
cd vox
cargo build --release   # needs cmake (brew install cmake) for espeak-ng
cp target/release/vox ~/.local/bin/
mkdir -p ~/Library/Caches/vox
cp -r target/release/build/espeak-rs-sys-*/out/share/espeak-ng-data ~/Library/Caches/vox/
vox --setup   # one-time model download (~350 MB) to ~/Library/Caches/vox
```

The binary is ~34 MB with ONNX Runtime and espeak-ng statically linked.
Two data pieces live outside it: the model weights (downloaded once by
`--setup`) and espeak-ng's phoneme data (~9 MB, copied from the build as
above; vox finds it in its cache dir, or set
`PIPER_ESPEAKNG_DATA_DIRECTORY`).

Phonemization is espeak-ng — the same G2P Kokoro was trained with, so
pronunciation matches the reference Python implementation, including
proper British phonemes (`en-gb-x-rp`) for the `b*` voices.

## Usage

```sh
vox "hello there"            # speak a string
vox -f notes.txt             # read a file
pbpaste | vox                # read piped text
vox -c                       # read the clipboard
vox -v bf_emma -s 1.2 "…"    # other voice, faster pace
vox --no-play -o out.wav "…" # render to file silently
vox --no-save "…"            # just speak, keep nothing
vox --list-voices
```

Voices: `bm_george`, `bm_lewis`, `bm_daniel`, `bm_fable`, `bf_emma`,
`bf_isabella` (British); `am_adam`, `am_michael`, `af_heart`, `af_bella`,
`af_nicole`, `af_sarah` (American).

## Playback controls

While vox is speaking in an interactive terminal:

- **space** — pause / resume
- **← / →** — skip back / forward 15 s (**Shift** for 30 s)
- **hold ← / →** — scrub backward / forward at 3× (release to resume normal playback)
- **↑ / ↓** — playback speed ±0.25× (0.25–3×, tape-style: pitch follows rate)
- **q** or **Esc** or **Ctrl-C** — cancel

Skipping ahead of what's been synthesized waits in silence until synthesis
catches up. Playback speed (↑/↓, applied live) is independent of synthesis
speed (`-s`, baked into the voice).

From anywhere else (another terminal, a script, a hotkey):

```sh
vox --stop    # silences any running vox
```

## Pronunciation fixes

The G2P is dictionary-based, so unusual words, names, and jargon can come
out oddly. Fix them with a lexicon at `~/.config/vox/lexicon.txt`
(override path with `VOX_LEXICON`) — one `word = respelling` per line,
matched whole-word and case-insensitively before synthesis:

```
# how it should sound, spelled phonetically
kokoro = koh-KOH-roh
qwen = kwen
nginx = engine-ex
```

## Defaults via environment

```sh
export VOX_VOICE=bm_george   # default voice
export VOX_SPEED=1.15        # default speech speed
```

## Where things go

- Spoken audio is saved to `~/Music/vox/` (override with `VOX_AUDIO_DIR`,
  or pass `--no-save`).
- Models cache in `~/Library/Caches/vox` (override with `VOX_CACHE_DIR`).
  `VOX_MODEL_FILE` selects an alternate model file — e.g.
  `kokoro-v1.0.int8.onnx` for the 92 MB int8 model, which is smaller but
  ~2× slower than fp32 on Apple Silicon.

## Performance (M1 Pro, 32 GB)

| Stage | Time |
|---|---|
| Model load | ~0.4 s |
| First sound | ~1–3 s (first sentence synthesizes alone) |
| Synthesis pace | 2.5–3.6× faster than realtime, streams while playing |

## History

v0.1 was a Python implementation on mlx-audio (see git history). The Rust
port cut first-sound latency from ~8 s to ~1–3 s and replaced a Python
environment with one binary.
