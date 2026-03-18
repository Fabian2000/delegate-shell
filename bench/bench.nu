# Nushell: loop, string ops, arithmetic
mut s = 0
for i in 1..10000 {
    $s = $s + $i
}
print $"Sum: ($s)"

# String building
mut result = ""
for i in 1..100 {
    $result = $result + "x"
}
print $"Len: ($result | str length)"

# Array ops
mut arr = []
for i in 1..1000 {
    $arr = ($arr | append $i)
}
print $"Array len: ($arr | length)"
