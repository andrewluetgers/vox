#!/bin/bash
# vox-speak.sh — Claude Code Stop hook: speak Claude's last response via vox.
#
# Flow: extract the final assistant message from the session transcript,
# summarize it into a few spoken sentences with a headless Haiku agent
# (claude -p), then play it through vox. Short responses are spoken verbatim.
#
# State: ~/.claude/vox/state.json
#   enabled      — master switch (toggled by the /vox skill)
#   voice        — vox voice for readouts
#   speed        — synthesis speed
#   verbatim_max — responses at or under this many chars skip summarization
#
# Files written per readout (used by the /vox skill):
#   last-full.txt   — the original untouched response ("read me the whole thing")
#   last-spoken.txt — what was actually spoken ("repeat that")
#
# One-shot suppression: the /vox skill touches ~/.claude/vox/skip-next so
# its own confirmation turn doesn't talk over a replay it just started.

set -u

VOX_DIR="$HOME/.claude/vox"
STATE="$VOX_DIR/state.json"

# Recursion guard: the summarizer below is itself a claude process whose
# Stop hook would fire and spawn another summarizer.
[ -n "${VOX_TTS_HOOK:-}" ] && exit 0
export VOX_TTS_HOOK=1

command -v vox >/dev/null 2>&1 || exit 0
command -v jq >/dev/null 2>&1 || exit 0

mkdir -p "$VOX_DIR"
[ -f "$STATE" ] || printf '{"enabled": true, "voice": "bm_george", "speed": 1.1, "verbatim_max": 300}\n' >"$STATE"

[ "$(jq -r '.enabled' "$STATE")" = "true" ] || exit 0

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

VOICE=$(jq -r '.voice // "bm_george"' "$STATE")
SPEED=$(jq -r '.speed // 1.1' "$STATE")
VERBATIM_MAX=$(jq -r '.verbatim_max // 300' "$STATE")

if [ "${#TEXT}" -le "$VERBATIM_MAX" ]; then
  SPOKEN=$(printf '%s' "$TEXT" | sed 's/[*_#`]//g')
else
  # Use subscription auth for the summarizer: a stray API key in the
  # environment takes precedence over the claude.ai login and may lack credits.
  SPOKEN=$(printf '%s' "$TEXT" | env -u ANTHROPIC_API_KEY -u ANTHROPIC_PROVIDER_API_KEY \
    claude -p --model haiku --no-session-persistence \
    "Rewrite the coding-assistant response above as a spoken status update: 1 to 3 short conversational sentences, plain prose only — no markdown, no code, no file paths or symbols unless they are the whole point. Lead with the outcome. Reply with only the sentences." \
    2>/dev/null)
  [ -n "$SPOKEN" ] || SPOKEN="$(printf '%s' "$TEXT" | sed 's/[*_#`]//g' | head -c "$VERBATIM_MAX")"
fi

printf '%s' "$SPOKEN" >"$VOX_DIR/last-spoken.txt"

vox --stop >/dev/null 2>&1
exec vox --no-save -v "$VOICE" -s "$SPEED" "$SPOKEN" >/dev/null 2>&1
