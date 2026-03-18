def fib [n: int]: nothing -> int {
    if $n <= 1 { return $n }
    (fib ($n - 1)) + (fib ($n - 2))
}

let result = fib 20
print $"fib\(20) = ($result)"
