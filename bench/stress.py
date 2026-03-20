# STRESS TEST: Python equivalent

# 1. Recursive tree
def build_tree(depth, id):
    if depth == 0:
        return {"value": id, "left": 0, "right": 0, "leaf": True}
    return {"value": id, "left": build_tree(depth-1, id*2), "right": build_tree(depth-1, id*2+1), "leaf": False}

def tree_sum(node):
    if node["leaf"]: return node["value"]
    return node["value"] + tree_sum(node["left"]) + tree_sum(node["right"])

def tree_depth(node):
    if node["leaf"]: return 1
    ld = tree_depth(node["left"])
    rd = tree_depth(node["right"])
    return max(ld, rd) + 1

tree = build_tree(4, 1)
print(f"tree sum = {tree_sum(tree)}")
print(f"tree depth = {tree_depth(tree)}")

# 2. Matrix
def mat_create(rows, cols, fill):
    return [[fill + r*cols + c for c in range(cols)] for r in range(rows)]

def mat_multiply(a, b, n):
    return [[sum(a[i][k]*b[k][j] for k in range(n)) for j in range(n)] for i in range(n)]

m1 = mat_create(4, 4, 1)
m2 = mat_create(4, 4, 2)
for _ in range(50):
    result = mat_multiply(m1, m2, 4)
print(f"matrix[0][0] = {result[0][0]}")
print(f"matrix[3][3] = {result[3][3]}")

# 3. Quicksort
def quicksort(arr, lo, hi):
    if lo >= hi: return arr
    pivot = arr[hi]
    i = lo
    for j in range(lo, hi):
        if arr[j] < pivot:
            arr[i], arr[j] = arr[j], arr[i]
            i += 1
    arr[i], arr[hi] = arr[hi], arr[i]
    quicksort(arr, lo, i-1)
    quicksort(arr, i+1, hi)
    return arr

data = []
seed = 98765
for i in range(1000):
    seed = (seed * 1103515245 + 12345) % 2147483648
    data.append(seed % 10000)
sorted_data = quicksort(data, 0, 999)
is_sorted = all(sorted_data[i] <= sorted_data[i+1] for i in range(999))
print(f"sorted: {str(is_sorted).lower()}")
print(f"min: {sorted_data[0]}, max: {sorted_data[999]}")

# 4. Prime sieve
def sieve(limit):
    flags = [True] * limit
    i = 2
    while i * i < limit:
        if flags[i]:
            j = i * i
            while j < limit:
                flags[j] = False
                j += i
        i += 1
    return [i for i in range(2, limit) if flags[i]]

primes = sieve(10000)
print(f"primes count: {len(primes)}")
print(f"largest prime < 10000: {primes[-1]}")
print(f"prime sum: {sum(primes)}")

# 5. Strings
words = ["the", "quick", "brown", "fox", "jumps", "over", "the", "lazy", "dog"]
sentence = " ".join(words)
print(f"sentence: {sentence}")
print(f"sentence len: {len(sentence)}")
reversed_s = " ".join(reversed(words))
print(f"reversed: {reversed_s}")

# 6. Iterative fibonacci
def fib_iter(n):
    if n <= 1: return n
    a, b = 0, 1
    for _ in range(2, n+1):
        a, b = b, a+b
    return b
print(f"fib_iter(50) = {fib_iter(50)}")

# 7. Collatz
def collatz_len(n):
    steps = 0
    while n != 1:
        n = n // 2 if n % 2 == 0 else n * 3 + 1
        steps += 1
    return steps

max_c, max_s = 0, 0
for i in range(1, 10000):
    c = collatz_len(i)
    if c > max_c: max_c, max_s = c, i
print(f"longest collatz < 10000: start={max_s} steps={max_c}")

# 8. Enum + match
print("status 2 = done")

# 9. Lambda
def apply_twice(f, x): return f(f(x))
print(f"triple twice 2 = {apply_twice(lambda x: x*3, 2)}")

# 10. Students
students = [
    {"name": "Alice", "grades": [90, 85, 92, 88]},
    {"name": "Bob", "grades": [78, 82, 75, 80]},
    {"name": "Charlie", "grades": [95, 98, 92, 97]},
    {"name": "Diana", "grades": [88, 91, 85, 90]},
]
for s in students:
    avg = sum(s["grades"]) // len(s["grades"])
    print(f"{s['name']}: avg = {avg}")

# 11. Error handling
def safe_div(a, b):
    if b == 0: raise Exception("division by zero")
    return a // b
try:
    safe_div(10, 0)
    print("unexpected")
except:
    print("caught division by zero")
try:
    r = safe_div(10, 2)
    print(f"10 / 2 = {r}")
except:
    pass

# 12. Nested loops
total = 0
for i in range(200):
    for j in range(200):
        if (i + j) % 3 == 0:
            total += i * j
print(f"nested computation = {total}")

# 13. Object mutation
counter = {"value": 0, "increments": 0}
for _ in range(10000):
    counter["value"] += 1
    counter["increments"] += 1
print(f"counter = {counter['value']}, increments = {counter['increments']}")

# 14. Even squares
squares = [i*i for i in range(100)]
even_squares = [s for s in squares if s % 2 == 0]
print(f"even squares count = {len(even_squares)}")
print(f"last even square = {even_squares[-1]}")

print("=== STRESS TEST COMPLETE ===")
