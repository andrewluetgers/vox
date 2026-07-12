#!/usr/bin/env python
"""vox — speak text aloud with Kokoro TTS (local, fast, streaming).

Usage:
    vox "Hello there, this is a test."
    vox -f notes.txt
    echo "piped text works too" | vox
    vox -c                       # read the clipboard
    vox "different voice" --voice bm_lewis
    vox "save it" -o out.wav --no-play

Default voice is bm_george (mature British male). Runs fully local via MLX
on Apple Silicon, several times faster than realtime; audio starts playing
while the rest is still being synthesized.
"""

import argparse
import datetime
import os
import re
import subprocess
import sys

# Optional cache override: model weights can live on an external drive.
# On exFAT volumes the HF xet downloader fails; plain HTTP works.
SSD_CACHE = os.environ.get("VOX_HF_CACHE", "/Volumes/Extreme SSD/hf-cache")
if os.path.isdir(SSD_CACHE):
    os.environ.setdefault("HF_HOME", SSD_CACHE)
    os.environ.setdefault("HF_HUB_DISABLE_XET", "1")

AUDIO_DIR = os.environ.get("VOX_AUDIO_DIR", os.path.expanduser("~/Music/vox"))
MODEL = "prince-canuma/Kokoro-82M"
SAMPLE_RATE = 24000

VOICES = [
    # British male / female
    "bm_george", "bm_lewis", "bm_daniel", "bm_fable", "bf_emma", "bf_isabella",
    # American male / female
    "am_adam", "am_michael", "af_heart", "af_bella", "af_nicole", "af_sarah",
]


def main() -> None:
    p = argparse.ArgumentParser(
        prog="vox", description="Read text aloud with Kokoro TTS (local, streaming)")
    p.add_argument("text", nargs="?", help="text to speak (or use -f / -c / stdin)")
    p.add_argument("-f", "--file", help="read text from a file")
    p.add_argument("-c", "--clip", action="store_true", help="read text from the clipboard")
    p.add_argument("-v", "--voice", default="bm_george", choices=VOICES, metavar="VOICE",
                   help=f"voice (default: bm_george; options: {', '.join(VOICES)})")
    p.add_argument("-s", "--speed", type=float, default=1.0, help="speech speed (default: 1.0)")
    p.add_argument("-o", "--out", help="write audio to this wav file")
    p.add_argument("--no-play", action="store_true", help="don't play, just save")
    p.add_argument("--no-save", action="store_true", help="don't save a wav, just play")
    args = p.parse_args()

    if args.clip:
        text = subprocess.run(["pbpaste"], capture_output=True, text=True).stdout
    elif args.file:
        text = open(args.file).read()
    elif args.text:
        text = args.text
    elif not sys.stdin.isatty():
        text = sys.stdin.read()
    else:
        p.error("no text given (pass as argument, -f FILE, -c, or stdin)")

    text = text.strip()
    if not text:
        p.error("input text is empty")

    import time

    t0 = time.perf_counter()

    import numpy as np
    from mlx_audio.tts.utils import load_model

    model = load_model(MODEL)
    load_s = time.perf_counter() - t0
    print(f"Model loaded in {load_s:.1f}s", file=sys.stderr)

    player = None
    if not args.no_play:
        player = subprocess.Popen(
            ["ffplay", "-f", "s16le", "-ar", str(SAMPLE_RATE), "-ch_layout", "mono",
             "-nodisp", "-autoexit", "-loglevel", "quiet", "-"],
            stdin=subprocess.PIPE,
        )

    pcm = bytearray()
    first_audio_at = None
    # lang_code from voice prefix: 'b' = British English, 'a' = American
    for result in model.generate(
        text=text, voice=args.voice, speed=args.speed,
        lang_code=args.voice[0], verbose=False,
    ):
        chunk = np.asarray(result.audio, dtype=np.float32)
        data = (np.clip(chunk, -1, 1) * 32767).astype(np.int16).tobytes()
        if first_audio_at is None:
            first_audio_at = time.perf_counter() - t0
            print(f"First audio at {first_audio_at:.2f}s "
                  f"(load {load_s:.1f}s + synth {first_audio_at - load_s:.2f}s)",
                  file=sys.stderr)
        pcm.extend(data)
        if player:
            try:
                player.stdin.write(data)
                player.stdin.flush()
            except BrokenPipeError:
                player = None  # user closed the player; keep saving

    total_s = time.perf_counter() - t0
    audio_s = len(pcm) / 2 / SAMPLE_RATE
    synth_s = total_s - load_s
    print(f"Synthesized {audio_s:.1f}s of audio in {synth_s:.1f}s "
          f"({audio_s / synth_s:.1f}x faster than realtime)", file=sys.stderr)

    if not args.no_save:
        if args.out:
            out = args.out
        else:
            os.makedirs(AUDIO_DIR, exist_ok=True)
            stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
            slug = re.sub(r"[^a-z0-9]+", "-", text[:40].lower()).strip("-")
            out = os.path.join(AUDIO_DIR, f"{stamp}-{slug}.wav")
        import wave
        with wave.open(out, "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(SAMPLE_RATE)
            w.writeframes(bytes(pcm))
        print(f"Saved {out}", file=sys.stderr)

    if player and player.stdin:
        player.stdin.close()
        player.wait()


if __name__ == "__main__":
    main()
