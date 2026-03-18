#!/bin/bash
N=50
# Initialize NxN matrix with i*N+j
declare -a matrix
for ((i=0; i<N; i++)); do
    for ((j=0; j<N; j++)); do
        matrix[$((i*N+j))]=$((i*N+j))
    done
done
# Sum all elements
sum=0
for ((i=0; i<N*N; i++)); do
    sum=$((sum + matrix[i]))
done
echo "Matrix sum: $sum"
