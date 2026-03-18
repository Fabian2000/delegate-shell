#!/bin/bash
fib() {
    local n=$1
    if [ "$n" -le 1 ]; then
        echo "$n"
        return
    fi
    local a=$(fib $((n - 1)))
    local b=$(fib $((n - 2)))
    echo $((a + b))
}
result=$(fib 20)
echo "fib(20) = $result"
