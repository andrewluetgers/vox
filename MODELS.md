# Model & data files

vox needs three pieces of data outside the binary. Two are **downloaded** by
`vox --setup`; one is **copied from the build** (never downloaded).

| File | Size | Source | Fetched by |
|---|---|---|---|
| `kokoro-v1.0.onnx` | ~310 MB | GitHub release (below) | `vox --setup` |
| `voices-v1.0.bin` | ~27 MB | GitHub release (below) | `vox --setup` |
| `espeak-ng-data/` | ~9 MB | `target/release/build/espeak-rs-sys-*/out/share/` | copied during install |

All three live in the cache dir: `~/Library/Caches/vox` (override with
`VOX_CACHE_DIR`).

## Where the downloads come from

Both model files are GitHub **release assets** on the `model-files-v1.0`
release of [`thewh1teagle/kokoro-onnx`](https://github.com/thewh1teagle/kokoro-onnx):

```
https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.onnx
https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin
```

GitHub serves these from its release-assets CDN. The base URL and filenames
are hardcoded in `src/main.rs` (`RELEASE_BASE`, `MODEL_FILE`, `VOICES_FILE`).

### Provenance

- **Model**: Kokoro-82M, an open-weights TTS model
  ([`hexgrad/Kokoro-82M`](https://huggingface.co/hexgrad/Kokoro-82M) on
  Hugging Face), **Apache-2.0**.
- **ONNX build + distribution**: `kokoro-onnx` by *thewh1teagle* (MIT,
  ~2.6k★). It exports Kokoro to ONNX and packs the 26 voice style vectors
  into `voices-v1.0.bin` (a numpy `.npz`). The `model-files-v1.0` release was
  published 2025-01-28 and has been stable since.
- **espeak-ng data** is the phoneme data for the G2P frontend; it ships in
  the `espeak-rs-sys` crate and is copied out of the build tree, so it is
  versioned with the code, not downloaded at runtime.

### Alternate model files

The release also carries smaller quantizations. Select one with
`VOX_MODEL_FILE` (the voices file is unchanged):

| `VOX_MODEL_FILE` | Size | Notes |
|---|---|---|
| `kokoro-v1.0.onnx` (default) | ~310 MB | fp32 — fastest on Apple Silicon CPU |
| `kokoro-v1.0.fp16.onnx` | ~169 MB | fp16 |
| `kokoro-v1.0.int8.onnx` | ~88 MB | int8 — smallest, ~2× slower than fp32 here |

## Is this a reliable source?

Reasonably, with caveats worth knowing:

- **Availability** is good — GitHub's CDN, a popular (~2.6k★) project, a
  release that hasn't changed since Jan 2025. Independent mirrors exist:
  [`leonelhs/kokoro-thewh1teagle`](https://huggingface.co/leonelhs/kokoro-thewh1teagle)
  on Hugging Face and a
  [SourceForge mirror](https://sourceforge.net/projects/kokoro-onnx.mirror/files/model-files-v1.0/).
- **Trust/integrity is on you.** It's a single maintainer's repo (not an
  official org), the release publishes **no checksums**, and vox's downloader
  does **no verification** — it streams bytes to disk and renames. So a
  corrupted, truncated, or tampered file would not be caught automatically.
  **Verify the SHA256 after downloading** (below).

### Verify your download

Expected SHA256 (verified byte-for-byte against the Hugging Face mirror
`leonelhs/kokoro-thewh1teagle`):

```
kokoro-v1.0.onnx  7d5df8ecf7d4b1878015a32686053fd0eebe2bc377234608764cc0ef3636a6c5
voices-v1.0.bin   bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d
```

Check them:

```sh
shasum -a 256 ~/Library/Caches/vox/kokoro-v1.0.onnx ~/Library/Caches/vox/voices-v1.0.bin
```

## Manual / offline download

`vox --setup` downloads via rustls, which trusts only its bundled root CAs.
Behind a TLS-inspecting proxy (common on corporate networks) that fails with
`invalid peer certificate: UnknownIssuer`. `curl` uses the system trust store,
so fetch the files yourself and vox will find them (it skips any file already
present):

```sh
BASE="https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0"
mkdir -p ~/Library/Caches/vox
curl -fSL -o ~/Library/Caches/vox/voices-v1.0.bin  "$BASE/voices-v1.0.bin"
curl -fSL -o ~/Library/Caches/vox/kokoro-v1.0.onnx "$BASE/kokoro-v1.0.onnx"
vox --setup   # confirms "Models ready"
```
