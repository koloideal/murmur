#!/bin/bash
# Swipe dispatcher for the waybar media widget.
# Smooth touchpad scrolling fires bursts of events per gesture, trickling
# over half a second; a plain fixed window lets the tail of one swipe
# execute a second time. Here every burst event refreshes the timestamp,
# so the burst as a whole cools down — exactly one action per gesture.
# flock makes the check-and-set atomic across the parallel processes
# waybar spawns.
# Usage: murmur-player-swipe.sh next|previous|seek+|seek-
dir="$1"
lock=/tmp/murmur-swipe.lock
(
    flock -w 0.1 9 || exit 0
    now=$(date +%s%N)
    last=$(cat "$lock" 2>/dev/null || echo 0)
    fresh=$(( now - last >= 500000000 ))
    echo "$now" > "$lock"
    (( fresh )) || exit 0
    echo "$(date '+%T.%3N') $dir" >> /tmp/murmur-swipe.log
    case "$dir" in
        next|previous) playerctl -p murmur "$dir" ;;
        seek+) playerctl -p murmur position 5+ ;;
        seek-) playerctl -p murmur position 5- ;;
    esac
) 9>"$lock.flock"
