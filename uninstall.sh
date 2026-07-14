#!/bin/bash
# Uninstall vox from this machine.
#
#   ./uninstall.sh           remove the binary, tray launch agent, and the
#                            Claude Code hook/skill; keep user data
#   ./uninstall.sh --purge   also delete state/history, the model cache,
#                            config, and saved audio
#
# Reverses everything the install steps create:
#   ~/.local/bin/vox                        (main README: cargo build + cp)
#   ~/Library/LaunchAgents/dev.andrewluetgers.vox-tray.plist  (tray setting)
#   Stop hook + /vox skill in ~/.claude     (claude/install.sh)
# User data, kept unless --purge:
#   ~/.claude/vox            state.json, history, last-spoken, md filter
#   ~/Library/Caches/vox     Kokoro model files (~350 MB) + espeak data
#   ~/.config/vox            TUI config.toml, lexicon
#   ~/Music/vox              saved readout audio (default location)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
LABEL="dev.andrewluetgers.vox-tray"

echo "Stopping vox…"
pkill -x vox-tray 2>/dev/null && echo "  stopped vox-tray"
pkill -x vox 2>/dev/null && echo "  stopped vox"

# tray launch agent (created by the tray's "Launch at startup" setting)
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null
if [ -f "$HOME/Library/LaunchAgents/$LABEL.plist" ]; then
  rm -f "$HOME/Library/LaunchAgents/$LABEL.plist"
  echo "Removed launch agent."
fi

# Claude Code hook + skill (delegated so the jq surgery lives in one place)
if [ -x "$HERE/claude/install.sh" ]; then
  "$HERE/claude/install.sh" --uninstall
fi

if [ -f "$HOME/.local/bin/vox" ]; then
  rm -f "$HOME/.local/bin/vox"
  echo "Removed ~/.local/bin/vox."
fi

if [ "${1:-}" = "--purge" ]; then
  rm -rf "$HOME/.claude/vox" "$HOME/Library/Caches/vox" "$HOME/.config/vox" "$HOME/Music/vox"
  echo "Purged state, history, model cache, config, and saved audio."
else
  echo "Kept user data:"
  for d in "$HOME/.claude/vox" "$HOME/Library/Caches/vox" "$HOME/.config/vox" "$HOME/Music/vox"; do
    [ -e "$d" ] && echo "  $d"
  done
  echo "Re-run with --purge to delete these too."
fi

echo "Done. (Anything built inside this repo — target/, tray/target/ — is untouched.)"
