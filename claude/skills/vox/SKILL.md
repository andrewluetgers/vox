---
name: vox
description: Control the vox spoken readouts of Claude's responses (the Stop-hook TTS). Use whenever the user addresses vox or the audio readout rather than the coding task — "vox stop", "quiet", "repeat that", "say that again", "read me the whole thing / the full output", "vox slower / faster", "mute vox", "vox on / off", "vox status", or a voice change. Also /vox <command>.
---

# vox readout control

Claude's responses are spoken aloud by a Stop hook (`~/.claude/vox/vox-speak.sh`)
that summarizes each response with a Haiku agent and plays it through the `vox`
CLI. This skill is the control surface for that readout. When the user says
"vox, ..." they mean the audio — never reinterpret it as a coding instruction.

## State

`~/.claude/vox/state.json` — read live by the hook on every readout:

```json
{ "enabled": true, "voice": "bm_george", "speed": 1.1, "verbatim_max": 300 }
```

Other files in `~/.claude/vox/`:
- `last-full.txt` — the original, unsummarized last response
- `last-spoken.txt` — what was actually read aloud
- `skip-next` — if present, the hook skips exactly one readout and deletes it

## The skip-next rule (important)

After handling any vox command, your own response triggers the Stop hook,
which calls `vox --stop` and would talk over (or kill) a replay you just
started. So: **for every vox action except turning vox ON, run
`touch ~/.claude/vox/skip-next` as part of the command.** When turning vox
back on, do NOT touch skip-next — the spoken confirmation is the feedback.

## Actions

Keep confirmations to one short sentence. Run replays with
`run_in_background: true` so the turn isn't blocked while audio plays.

**stop / quiet** — silence current speech:
```sh
vox --stop && touch ~/.claude/vox/skip-next
```

**off / mute** — disable readouts:
```sh
jq '.enabled = false' ~/.claude/vox/state.json > /tmp/vox-state.$$ && mv /tmp/vox-state.$$ ~/.claude/vox/state.json && vox --stop && touch ~/.claude/vox/skip-next
```

**on / unmute** — enable readouts (no skip-next; the hook speaking your
confirmation proves it works):
```sh
jq '.enabled = true' ~/.claude/vox/state.json > /tmp/vox-state.$$ && mv /tmp/vox-state.$$ ~/.claude/vox/state.json
```

**repeat / say that again** — replay the last spoken summary (background):
```sh
touch ~/.claude/vox/skip-next && vox --stop && vox --no-save -v "$(jq -r .voice ~/.claude/vox/state.json)" -s "$(jq -r .speed ~/.claude/vox/state.json)" "$(cat ~/.claude/vox/last-spoken.txt)"
```

**full / read the whole thing** — read the original unsummarized response
(background; can be long):
```sh
touch ~/.claude/vox/skip-next && vox --stop && vox --no-save -v "$(jq -r .voice ~/.claude/vox/state.json)" -s "$(jq -r .speed ~/.claude/vox/state.json)" -f ~/.claude/vox/last-full.txt
```

**slower / faster** — step speed by 0.15 (clamp to 0.5–2.0). Applies from the
next utterance; vox can't retune speech already playing. Example for faster:
```sh
jq '.speed = ([([(.speed + 0.15), 2.0] | min), 0.5] | max)' ~/.claude/vox/state.json > /tmp/vox-state.$$ && mv /tmp/vox-state.$$ ~/.claude/vox/state.json && touch ~/.claude/vox/skip-next
```
(Use `- 0.15` for slower. If the user gives a number — "speed 1.5" — set it
directly.)

**voice <name>** — set `.voice` in state.json the same way, then touch
skip-next. Voices: bm_george, bm_lewis, bm_daniel, bm_fable, bf_emma,
bf_isabella (British); am_adam, am_michael, af_heart, af_bella, af_nicole,
af_sarah (American).

**status** — `cat ~/.claude/vox/state.json`, report enabled/voice/speed in
one sentence, then touch skip-next.

If state.json is missing, the hook recreates it with defaults on the next
turn; you can also create it with the JSON shown above.
