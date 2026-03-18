#!/bin/bash
declare -A counts
text="the quick brown fox jumps over the lazy dog the fox the dog the the quick quick"
for word in $text; do
    counts[$word]=$(( ${counts[$word]:-0} + 1 ))
done
for word in "${!counts[@]}"; do
    echo "$word: ${counts[$word]}"
done | sort
