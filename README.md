# vox

Local streaming text-to-speech CLI in a single Rust binary. Kokoro-82M via
ONNX Runtime — no cloud, no API keys, no Python. First word in ~1–3 seconds,
synthesis 2.5–3.6× faster than realtime on an M1 Pro.

Default voice is `bm_george`, a mature British male.

## Install

```sh
git clone https://github.com/andrewluetgers/vox.git
cd vox
cargo build --release
cp target/release/vox ~/.local/bin/
vox --setup   # one-time model download (~350 MB) to ~/Library/Caches/vox
```

The binary is ~34 MB and self-contained (ONNX Runtime statically linked,
G2P built in). Model weights live outside the binary and download once.

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
