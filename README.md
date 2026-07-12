# vox

Local streaming text-to-speech CLI. Kokoro-82M via [MLX](https://github.com/ml-explore/mlx)
on Apple Silicon — no cloud, no API keys, synthesis runs ~3× faster than
realtime and starts speaking in about 5 seconds.

Default voice is `bm_george`, a mature British male.

## Install

Requires macOS on Apple Silicon, [uv](https://docs.astral.sh/uv/), and
`ffplay` (`brew install ffmpeg`) for streaming playback.

```sh
git clone https://github.com/andrewluetgers/vox.git
uv tool install --editable ./vox
```

This puts a `vox` command on your PATH (via `~/.local/bin`).

## Usage

```sh
vox "hello there"            # speak a string
vox -f notes.txt             # read a file
pbpaste | vox                # read piped text
vox -c                       # read the clipboard
vox -v bf_emma -s 1.2 "…"    # other voice, faster pace
vox --no-play -o out.wav "…" # render to file silently
vox --no-save "…"            # just speak, keep nothing
```

Voices: `bm_george`, `bm_lewis`, `bm_daniel`, `bm_fable`, `bf_emma`,
`bf_isabella` (British); `am_adam`, `am_michael`, `af_heart`, `af_bella`,
`af_nicole`, `af_sarah` (American).

## Where things go

- Spoken audio is saved to `~/Music/vox/` with timestamped names
  (override with `VOX_AUDIO_DIR`, or skip saving with `--no-save`).
- Model weights (~600 MB) download once to the Hugging Face cache.
  `VOX_HF_CACHE` points the cache somewhere else (e.g. an external drive);
  on exFAT volumes vox automatically disables the HF xet downloader,
  which fails there.

## Latency (M1 Pro, 32 GB)

| Stage | Time |
|---|---|
| Model load | ~3 s |
| First sound | ~5 s |
| Synthesis pace | ~3× faster than realtime, streams while generating |
