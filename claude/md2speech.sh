#!/bin/bash
# md2speech — turn markdown into speakable plain text (stdin -> stdout).
#
# Philosophy (how listening apps do it, vs. screen readers): never speak the
# syntax, convert structure into pauses. Formatting markers vanish; headers
# and list items end with punctuation so the TTS pauses between them; code
# blocks collapse to a single announcement; links read their text, not URL;
# table rows read as comma-separated phrases.

exec awk '
BEGIN { in_code = 0 }

# Fenced code blocks: one announcement, contents skipped.
/^[[:space:]]*(```|~~~)/ {
  if (in_code) {
    in_code = 0
  } else {
    in_code = 1
    print "Code block omitted."
  }
  next
}
in_code { next }

{
  line = $0

  # Table separator rows (|---|---|) vanish.
  if (line ~ /^[[:space:]]*\|[-:| [:space:]]+$/) next

  # Horizontal rules (--- *** ___) become a blank line: just a pause.
  t = line
  gsub(/[^-*_]/, "", t)
  if (length(t) >= 3 && line ~ /^[[:space:]*_-]+$/) { print ""; next }

  # Table rows: pipes become commas, row ends with a period.
  if (line ~ /^[[:space:]]*\|/) {
    sub(/^[[:space:]]*\|[[:space:]]*/, "", line)
    sub(/[[:space:]]*\|[[:space:]]*$/, "", line)
    gsub(/[[:space:]]*\|[[:space:]]*/, ", ", line)
    if (line !~ /[.!?]$/) line = line "."
    print line
    next
  }

  # Blockquote markers.
  sub(/^[[:space:]]*(>[[:space:]]*)+/, "", line)

  # Headers: drop the hashes; the period buys a spoken pause. Not announced.
  if (line ~ /^[[:space:]]*#+[[:space:]]/) {
    sub(/^[[:space:]]*#+[[:space:]]*/, "", line)
    if (line !~ /[.!?:]$/) line = line "."
  }

  # Bullets: drop the marker, keep the sentence, end with punctuation so each
  # item is its own spoken phrase. No "bullet bullet bullet".
  if (line ~ /^[[:space:]]*[-*+][[:space:]]/) {
    sub(/^[[:space:]]*[-*+][[:space:]]+/, "", line)
    if (line !~ /[.!?:;,]$/) line = line "."
  }

  # Numbered items read naturally ("1." is spoken "one") — keep the number,
  # just guarantee the pause at the end.
  if (line ~ /^[[:space:]]*[0-9]+[.)][[:space:]]/) {
    if (line !~ /[.!?:;,]$/) line = line "."
  }

  # Links and images: keep the text, drop the url.
  while (match(line, /!?\[[^]]*\]\([^)]*\)/)) {
    m = substr(line, RSTART, RLENGTH)
    sub(/^!?\[/, "", m)
    sub(/\].*$/, "", m)
    line = substr(line, 1, RSTART - 1) m substr(line, RSTART + RLENGTH)
  }

  # Inline formatting characters vanish: bold, italic, code, strikethrough.
  gsub(/\*\*|__|[*`~]/, "", line)

  # Underscore italics only when the underscores sit on word boundaries, so
  # snake_case identifiers survive.
  while (match(line, /(^|[[:space:]])_[^_]+_([[:space:]]|[.,!?;:)]|$)/)) {
    seg = substr(line, RSTART, RLENGTH)
    gsub(/_/, "", seg)
    line = substr(line, 1, RSTART - 1) seg substr(line, RSTART + RLENGTH)
  }

  print line
}
'
