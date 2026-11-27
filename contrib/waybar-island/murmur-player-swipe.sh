#!/bin/bash
# Throttled swipe dispatcher for the waybar media widget.
# Smooth touchpad scrolling fires bursts of events per gesture;
# a 500ms lock collapses each swipe into exactly one action.
# Usage: murmur-player-swipe.sh next|previous|seek+|seek-
dir="$1"
lock=/tmp/murmur-swipe.lock
now=$(date +%s%N)
last=$(cat "$lock" 2>/dev/null || echo 0)
if (( now - last < 400000000 )); then exit 0; fi
echo "$now" > "$lock"
echo "$(date '+%T.%3N') $dir" >> /tmp/murmur-swipe.log
case "$dir" in
    next|previous) playerctl -p murmur "$dir" ;;
    seek+) playerctl -p murmur position 5+ ;;
    seek-) playerctl -p murmur position 5- ;;
esac
