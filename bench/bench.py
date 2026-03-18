# Python: loop, string ops, arithmetic
s = 0
for i in range(1, 10001):
    s += i
print(f"Sum: {s}")

# String building
result = ""
for i in range(1, 101):
    result = result + "x"
print(f"Len: {len(result)}")

# Array ops
arr = []
for i in range(1, 1001):
    arr.append(i)
print(f"Array len: {len(arr)}")
