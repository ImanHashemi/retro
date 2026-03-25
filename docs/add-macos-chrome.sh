#!/usr/bin/env bash
# Add macOS-style window chrome to a screenshot.
# Usage: ./docs/add-macos-chrome.sh input.png output.png [title]
set -e

INPUT="$1"
OUTPUT="$2"
TITLE="${3:-retro dash}"
TITLEBAR_HEIGHT=32
RADIUS=10

if [ -z "$INPUT" ] || [ -z "$OUTPUT" ]; then
    echo "Usage: $0 input.png output.png [title]"
    exit 1
fi

WIDTH=$(magick identify -format "%w" "$INPUT")
HEIGHT=$(magick identify -format "%h" "$INPUT")
TOTAL_H=$((HEIGHT + TITLEBAR_HEIGHT))

# Step 1: Create title bar with traffic lights and title
magick -size "${WIDTH}x${TITLEBAR_HEIGHT}" xc:'#181825' \
    -fill '#ff5f57' -draw "circle 16,16 22,16" \
    -fill '#febc2e' -draw "circle 36,16 42,16" \
    -fill '#28c840' -draw "circle 56,16 62,16" \
    -fill '#7f849c' -font "Helvetica" -pointsize 13 \
    -gravity center -annotate +0+0 "$TITLE" \
    /tmp/retro-titlebar.png

# Step 2: Stack title bar on top of screenshot
magick /tmp/retro-titlebar.png "$INPUT" -append /tmp/retro-stacked.png

# Step 3: Add rounded corners via mask
magick -size "${WIDTH}x${TOTAL_H}" xc:black \
    -fill white -draw "roundrectangle 0,0 $((WIDTH-1)),$((TOTAL_H-1)) $RADIUS,$RADIUS" \
    /tmp/retro-mask.png

magick /tmp/retro-stacked.png /tmp/retro-mask.png \
    -alpha off -compose CopyOpacity -composite \
    /tmp/retro-rounded.png

# Step 4: Add drop shadow on white background
magick \( /tmp/retro-rounded.png -background black -shadow 40x15+0+8 \) \
    /tmp/retro-rounded.png \
    -background white -layers merge +repage \
    "$OUTPUT"

rm -f /tmp/retro-titlebar.png /tmp/retro-stacked.png /tmp/retro-mask.png /tmp/retro-rounded.png
echo "Created $OUTPUT"
