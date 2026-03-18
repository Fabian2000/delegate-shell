let n = 50;
let matrix = [];
for (let i = 0; i < n; i++) {
    for (let j = 0; j < n; j++) {
        matrix.push(i * n + j);
    }
}

let s = 0;
for (let i = 0; i < matrix.length; i++) {
    s += matrix[i];
}

console.log(`Matrix sum: ${s}`);
