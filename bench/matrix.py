n = 50
matrix = []
for i in range(n):
    for j in range(n):
        matrix.append(i * n + j)

s = 0
for i in range(len(matrix)):
    s += matrix[i]

print(f"Matrix sum: {s}")
