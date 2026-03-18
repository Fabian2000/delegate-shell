let n = 50
mut matrix = []
for i in 0..($n - 1) {
    for j in 0..($n - 1) {
        $matrix = ($matrix | append ($i * $n + $j))
    }
}

mut s = 0
for i in 0..(($matrix | length) - 1) {
    $s = $s + ($matrix | get $i)
}

print $"Matrix sum: ($s)"
