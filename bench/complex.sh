#!/bin/bash
# Complex benchmark: simulates a log analyzer

# --- Generate fake log data ---
levels=("INFO" "WARN" "ERROR" "DEBUG" "INFO" "INFO" "ERROR" "WARN" "INFO" "DEBUG")
services=("auth" "api" "db" "cache" "worker" "gateway" "scheduler" "monitor")
messages=("request completed" "connection timeout" "cache miss" "query slow" "rate limited" "health check ok" "retry attempt" "disk usage high")

declare -a log_level
declare -a log_service
declare -a log_message

seed=12345
for ((i=0; i<5000; i++)); do
    seed=$(( (seed * 1103515245 + 12345) % 2147483648 ))
    log_level[$i]="${levels[$((seed % ${#levels[@]}))]}"
    seed2=$(( (seed * 7 + 3) % ${#services[@]} ))
    log_service[$i]="${services[$seed2]}"
    seed3=$(( (seed * 13 + 5) % ${#messages[@]} ))
    log_message[$i]="${messages[$seed3]}"
done

# --- Count by level ---
declare -A level_counts
for level in INFO WARN ERROR DEBUG; do
    level_counts[$level]=0
done
for ((i=0; i<5000; i++)); do
    l="${log_level[$i]}"
    level_counts[$l]=$(( ${level_counts[$l]} + 1 ))
done

echo "=== Log Level Distribution ==="
for level in INFO WARN ERROR DEBUG; do
    echo "$level: ${level_counts[$level]}"
done

# --- Count errors by service ---
declare -A svc_errors
for svc in auth api db cache worker gateway scheduler monitor; do
    svc_errors[$svc]=0
done
for ((i=0; i<5000; i++)); do
    if [[ "${log_level[$i]}" == "ERROR" ]]; then
        s="${log_service[$i]}"
        svc_errors[$s]=$(( ${svc_errors[$s]} + 1 ))
    fi
done

echo "=== Errors by Service ==="
for svc in auth api db cache worker gateway scheduler monitor; do
    if [[ ${svc_errors[$svc]} -gt 0 ]]; then
        echo "$svc: ${svc_errors[$svc]}"
    fi
done

# --- Find longest streak of same level ---
max_streak=0
current_streak=1
prev_level="${log_level[0]}"

for ((i=1; i<5000; i++)); do
    if [[ "${log_level[$i]}" == "$prev_level" ]]; then
        current_streak=$((current_streak + 1))
        if [[ $current_streak -gt $max_streak ]]; then
            max_streak=$current_streak
        fi
    else
        current_streak=1
    fi
    prev_level="${log_level[$i]}"
done

echo "=== Longest Streak ==="
echo "Max consecutive same level: $max_streak"

# --- Moving average of error rate (window=100) ---
window=100
declare -a error_flags
for ((i=0; i<5000; i++)); do
    if [[ "${log_level[$i]}" == "ERROR" ]]; then
        error_flags[$i]=1
    else
        error_flags[$i]=0
    fi
done

max_rate=0
for ((i=0; i<=5000-window; i++)); do
    sum=0
    for ((j=i; j<i+window; j++)); do
        sum=$((sum + error_flags[j]))
    done
    if [[ $sum -gt $max_rate ]]; then
        max_rate=$sum
    fi
done

echo "=== Error Rate ==="
echo "Peak error rate (per $window): $max_rate"

# --- String building: format top error messages ---
declare -a err_msgs
declare -a err_counts
num_err_msgs=0

for ((i=0; i<5000; i++)); do
    if [[ "${log_level[$i]}" == "ERROR" ]]; then
        msg="${log_message[$i]}"
        found=-1
        for ((k=0; k<num_err_msgs; k++)); do
            if [[ "${err_msgs[$k]}" == "$msg" ]]; then
                found=$k
                break
            fi
        done
        if [[ $found -eq -1 ]]; then
            err_msgs[$num_err_msgs]="$msg"
            err_counts[$num_err_msgs]=1
            num_err_msgs=$((num_err_msgs + 1))
        else
            err_counts[$found]=$(( ${err_counts[$found]} + 1 ))
        fi
    fi
done

# Bubble sort by count
for ((i=0; i<num_err_msgs-1; i++)); do
    for ((j=0; j<num_err_msgs-i-1; j++)); do
        if [[ ${err_counts[$j]} -lt ${err_counts[$((j+1))]} ]]; then
            tmp=${err_counts[$j]}
            err_counts[$j]=${err_counts[$((j+1))]}
            err_counts[$((j+1))]=$tmp
            tmp2="${err_msgs[$j]}"
            err_msgs[$j]="${err_msgs[$((j+1))]}"
            err_msgs[$((j+1))]="$tmp2"
        fi
    done
done

echo "=== Top Error Messages ==="
top=3
if [[ $num_err_msgs -lt $top ]]; then top=$num_err_msgs; fi
for ((i=0; i<top; i++)); do
    echo "$((i+1)). ${err_msgs[$i]} (${err_counts[$i]}x)"
done

echo "=== Done: processed 5000 log entries ==="
