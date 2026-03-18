text = "the quick brown fox jumps over the lazy dog the fox the dog the the quick quick"
words = text.split(" ")

keys = []
vals = []

def find_idx(keys, w):
    for i in range(len(keys)):
        if keys[i] == w:
            return i
    return -1

for w in words:
    idx = find_idx(keys, w)
    if idx == -1:
        keys.append(w)
        vals.append(1)
    else:
        vals[idx] = vals[idx] + 1

for i in range(len(keys)):
    print(f"{keys[i]}: {vals[i]}")
