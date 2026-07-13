#!/bin/bash
# vox-speak.sh — Claude Code Stop hook: speak Claude's last response via vox.
#
# Flow: extract the final assistant message from the session transcript,
# summarize it into a few spoken sentences with a headless Haiku agent
# (claude -p), then play it through vox. Short responses are spoken verbatim.
#
# State: ~/.claude/vox/state.json (shared with the /vox skill and vox-tray)
#   enabled        — master switch
#   voice          — vox voice for readouts
#   speed          — synthesis speed
#   verbatim_max   — responses at or under this many chars skip summarization
#   summary_prompt — override for the summarizer prompt (unset = built-in default)
#   save_audio     — keep wav files of readouts (default false)
#   audio_dir      — where wavs go when save_audio is on (default ~/Music/vox)
#
# Per-project overrides: a .vox.json in the project root (found by walking up
# from the hook's cwd) overrides state.json key-by-key, so a repo can pin its
# own voice, prompt, or "enabled": false.
#
# Files written per readout:
#   last-full.txt   — the original untouched response ("read me the whole thing")
#   last-spoken.txt — what was actually spoken ("repeat that")
#   history.jsonl   — one line per readout: {ts, source, text}
#
# One-shot suppression: the /vox skill touches ~/.claude/vox/skip-next so
# its own confirmation turn doesn't talk over a replay it just started.

set -u

VOX_DIR="$HOME/.claude/vox"
STATE="$VOX_DIR/state.json"

DEFAULT_PROMPT="You rewrite coding-assistant responses as spoken status updates: 1 to 3 short conversational sentences, plain prose only — no markdown, no code, no file paths or symbols unless they are the whole point. Lead with the outcome. Reply with only the sentences."

# Recursion guard: the summarizer below is itself a claude process whose
# Stop hook would fire and spawn another summarizer.
[ -n "${VOX_TTS_HOOK:-}" ] && exit 0
export VOX_TTS_HOOK=1

command -v vox >/dev/null 2>&1 || exit 0
command -v jq >/dev/null 2>&1 || exit 0

mkdir -p "$VOX_DIR"
[ -f "$STATE" ] || printf '{"enabled": true, "voice": "bm_george", "speed": 1.1, "verbatim_max": 300}\n' >"$STATE"

# Project config: nearest .vox.json at or above the cwd.
PROJ=""
d="$PWD"
while [ "$d" != "/" ] && [ -n "$d" ]; do
  if [ -f "$d/.vox.json" ]; then
    PROJ="$d/.vox.json"
    break
  fi
  d=$(dirname "$d")
done

# Register this repo in the global registry so the vox-tray settings picker
# can offer per-repo overrides for every project the hook has spoken from.
# Registered before the enabled check so silenced repos stay visible.
ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
REG="$VOX_DIR/projects.json"
[ -f "$REG" ] || echo '{}' >"$REG"
jq --arg p "$ROOT" --arg ts "$(date +%s)" '.[$p] = {last_seen: ($ts | tonumber)}' "$REG" \
  >"$REG.tmp" 2>/dev/null && mv "$REG.tmp" "$REG"

# cfg <key> <default>: project file wins, then global state, then default.
# `has()` (not `//`) so an explicit false survives the lookup.
cfg() {
  local key=$1 def=$2 f v
  for f in "$PROJ" "$STATE"; do
    { [ -n "$f" ] && [ -f "$f" ]; } || continue
    v=$(jq -r --arg k "$key" 'if has($k) then (.[$k] | tostring) else empty end' "$f" 2>/dev/null)
    if [ -n "$v" ]; then
      printf '%s' "$v"
      return
    fi
  done
  printf '%s' "$def"
}

[ "$(cfg enabled true)" = "true" ] || exit 0

if [ -f "$VOX_DIR/skip-next" ]; then
  rm -f "$VOX_DIR/skip-next"
  exit 0
fi

INPUT=$(cat)
TRANSCRIPT=$(printf '%s' "$INPUT" | jq -r '.transcript_path // empty')
{ [ -n "$TRANSCRIPT" ] && [ -f "$TRANSCRIPT" ]; } || exit 0

# Last assistant entry that contains text (skips tool-use-only entries).
TEXT=$(jq -rs '
  [ .[]
    | select(.type == "assistant")
    | .message.content
    | if type == "array" then map(select(.type == "text") | .text) | join("\n") else tostring end
    | select(length > 0)
  ] | last // empty' "$TRANSCRIPT" 2>/dev/null)

[ -n "$TEXT" ] || exit 0

# Stop fires on resume/clear/compact too — don't re-speak the same message.
if [ -f "$VOX_DIR/last-full.txt" ] && [ "$TEXT" = "$(cat "$VOX_DIR/last-full.txt")" ]; then
  exit 0
fi
printf '%s' "$TEXT" >"$VOX_DIR/last-full.txt"

VOICE=$(cfg voice bm_george)
SPEED=$(cfg speed 1.1)
VERBATIM_MAX=$(cfg verbatim_max 300)
PROMPT=$(cfg summary_prompt "$DEFAULT_PROMPT")

# Markdown -> speakable text: strip formatting, turn structure into pauses.
speakable() {
  if [ -x "$VOX_DIR/md2speech.sh" ]; then
    "$VOX_DIR/md2speech.sh"
  else
    sed 's/[*_#`]//g'
  fi
}

if [ "${#TEXT}" -le "$VERBATIM_MAX" ]; then
  SPOKEN=$(printf '%s' "$TEXT" | speakable)
else
  # The prompt goes in --system-prompt (replacing the agentic Claude Code one)
  # and --disallowedTools keeps the summarizer from wandering off to read the
  # repo instead of rewriting the text. Unset API keys so claude -p uses the
  # subscription login (a stray key takes precedence and may lack credits).
  SPOKEN=$(printf '%s' "$TEXT" | env -u ANTHROPIC_API_KEY -u ANTHROPIC_PROVIDER_API_KEY \
    claude -p --model haiku --no-session-persistence --disallowedTools "*" \
    --system-prompt "$PROMPT" 2>/dev/null | speakable)
  [ -n "$SPOKEN" ] || SPOKEN="$(printf '%s' "$TEXT" | speakable | head -c "$VERBATIM_MAX")"
  # Runaway guard: a spoken update should never be a wall of text.
  if [ "${#SPOKEN}" -gt 900 ]; then
    SPOKEN="$(printf '%s' "$SPOKEN" | head -c 900). Summary ran long, cut off here."
  fi
fi

printf '%s' "$SPOKEN" >"$VOX_DIR/last-spoken.txt"

# Text history (browsable/replayable in vox-tray): optional and TTL-bound.
# last-full/last-spoken above are always kept — "repeat that" works even
# with history off.
if [ "$(cfg save_history true)" = "true" ]; then
  # The hook runs with cwd = the Claude session's project, so history can
  # say which repo was talking.
  PROJECT=$(basename "$ROOT")
  jq -nc --arg ts "$(date +%s)" --arg text "$SPOKEN" --arg project "$PROJECT" \
    '{ts: ($ts | tonumber), source: "claude", project: $project, text: $text}' >>"$VOX_DIR/history.jsonl"
  TTL=$(cfg history_ttl_minutes 20)
  TTL=${TTL%.*}
  if [ "$TTL" -gt 0 ] 2>/dev/null; then
    CUT=$(($(date +%s) - TTL * 60))
    jq -c --argjson cut "$CUT" 'select(.ts >= $cut)' "$VOX_DIR/history.jsonl" \
      >"$VOX_DIR/history.jsonl.tmp" 2>/dev/null && mv "$VOX_DIR/history.jsonl.tmp" "$VOX_DIR/history.jsonl"
  fi
fi

# Audio saving: off by default so readouts don't accumulate on disk.
SAVE_ARGS=()
if [ "$(cfg save_audio false)" = "true" ]; then
  AUDIO_DIR=$(cfg audio_dir "$HOME/Music/vox")
  export VOX_AUDIO_DIR="${AUDIO_DIR/#\~/$HOME}"
else
  SAVE_ARGS+=(--no-save)
fi

vox --stop >/dev/null 2>&1
exec vox ${SAVE_ARGS[@]+"${SAVE_ARGS[@]}"} -v "$VOICE" -s "$SPEED" "$SPOKEN" >/dev/null 2>&1
