let text = "the quick brown fox jumps over the lazy dog the fox the dog the the quick quick"
let words = ($text | split row " ")

mut keys = []
mut vals = []

def find_idx [keys: list<string>, w: string]: nothing -> int {
    for i in 0..(($keys | length) - 1) {
        if ($keys | get $i) == $w { return $i }
    }
    -1
}

for w in $words {
    let idx = find_idx $keys $w
    if $idx == -1 {
        $keys = ($keys | append $w)
        $vals = ($vals | append 1)
    } else {
        $vals = ($vals | update $idx (($vals | get $idx) + 1))
    }
}

for i in 0..(($keys | length) - 1) {
    print $"($keys | get $i): ($vals | get $i)"
}
