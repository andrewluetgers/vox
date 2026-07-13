#!/bin/bash
# Install the Claude Code voice-readout integration for vox.
#
# Copies the Stop-hook script and the /vox skill into ~/.claude, seeds the
# state file, and merges the hook into ~/.claude/settings.json (idempotent —
# safe to re-run; existing settings and hooks are preserved).
#
#   ./claude/install.sh              install or update
#   ./claude/install.sh --uninstall  remove the hook entry and skill
#
# Requires: jq. The vox binary must be on PATH (see the main README to build
# it), and the claude CLI is used for summarization at runtime.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
CLAUDE_DIR="$HOME/.claude"
VOX_DIR="$CLAUDE_DIR/vox"
SKILL_DIR="$CLAUDE_DIR/skills/vox"
SETTINGS="$CLAUDE_DIR/settings.json"
HOOK_CMD='"$HOME/.claude/vox/vox-speak.sh"'

command -v jq >/dev/null 2>&1 || { echo "error: jq is required (brew install jq)" >&2; exit 1; }

if [ "${1:-}" = "--uninstall" ]; then
  if [ -f "$SETTINGS" ]; then
    tmp=$(mktemp)
    jq '
      if .hooks.Stop then
        .hooks.Stop |= (map(.hooks |= map(select((.command // "") | contains("vox-speak.sh") | not)))
                        | map(select(.hooks | length > 0)))
      else . end' "$SETTINGS" >"$tmp" && mv "$tmp" "$SETTINGS"
  fi
  rm -rf "$SKILL_DIR"
  rm -f "$VOX_DIR/vox-speak.sh"
  echo "Uninstalled. State and transcripts in $VOX_DIR were kept; delete that dir to remove them."
  exit 0
fi

command -v vox >/dev/null 2>&1 || echo "warning: vox not found on PATH — build and install it first (see main README)" >&2
command -v claude >/dev/null 2>&1 || echo "warning: claude CLI not found — long responses will be truncated instead of summarized" >&2

mkdir -p "$VOX_DIR" "$SKILL_DIR"
cp "$HERE/vox-speak.sh" "$VOX_DIR/vox-speak.sh"
chmod +x "$VOX_DIR/vox-speak.sh"
cp "$HERE/skills/vox/SKILL.md" "$SKILL_DIR/SKILL.md"
[ -f "$VOX_DIR/state.json" ] || printf '{"enabled": true, "voice": "bm_george", "speed": 1.1, "verbatim_max": 300}\n' >"$VOX_DIR/state.json"

[ -f "$SETTINGS" ] || echo '{}' >"$SETTINGS"
jq empty "$SETTINGS" || { echo "error: $SETTINGS is not valid JSON — fix it and re-run" >&2; exit 1; }

if jq -e '[.hooks.Stop[]?.hooks[]?.command // empty] | any(contains("vox-speak.sh"))' "$SETTINGS" >/dev/null; then
  echo "Stop hook already present in $SETTINGS — left as is."
else
  tmp=$(mktemp)
  jq --arg cmd "$HOOK_CMD" '
    .hooks.Stop = ((.hooks.Stop // []) + [{
      "matcher": "",
      "hooks": [{
        "type": "command",
        "command": $cmd,
        "async": true,
        "timeout": 300,
        "statusMessage": "vox is preparing the readout"
      }]
    }])' "$SETTINGS" >"$tmp" && mv "$tmp" "$SETTINGS"
  echo "Added vox Stop hook to $SETTINGS."
fi

echo "Installed. In a running Claude Code session, open /hooks once (or restart) to load the hook."
echo "Try it: ask Claude anything, then say \"vox, repeat that\" or \"/vox status\"."
