#!/bin/bash
# Bash: loop, string ops, arithmetic
sum=0
for i in $(seq 1 10000); do
    sum=$((sum + i))
done
echo "Sum: $sum"

# String building
result=""
for i in $(seq 1 100); do
    result="${result}x"
done
echo "Len: ${#result}"

# Array ops
arr=()
for i in $(seq 1 1000); do
    arr+=($i)
done
echo "Array len: ${#arr[@]}"
