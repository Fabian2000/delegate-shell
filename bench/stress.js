// STRESS TEST: Node.js equivalent

// 1. Recursive tree
function build_tree(depth, id) {
    if (depth === 0) return {value: id, left: 0, right: 0, leaf: true};
    return {value: id, left: build_tree(depth-1, id*2), right: build_tree(depth-1, id*2+1), leaf: false};
}
function tree_sum(node) {
    if (node.leaf) return node.value;
    return node.value + tree_sum(node.left) + tree_sum(node.right);
}
function tree_depth(node) {
    if (node.leaf) return 1;
    return Math.max(tree_depth(node.left), tree_depth(node.right)) + 1;
}
let tree = build_tree(4, 1);
console.log(`tree sum = ${tree_sum(tree)}`);
console.log(`tree depth = ${tree_depth(tree)}`);

// 2. Matrix
function mat_create(rows, cols, fill) {
    let m = [];
    for (let r = 0; r < rows; r++) {
        let row = [];
        for (let c = 0; c < cols; c++) row.push(fill + r*cols + c);
        m.push(row);
    }
    return m;
}
function mat_multiply(a, b, n) {
    let result = [];
    for (let i = 0; i < n; i++) {
        let row = [];
        for (let j = 0; j < n; j++) {
            let total = 0;
            for (let k = 0; k < n; k++) total += a[i][k] * b[k][j];
            row.push(total);
        }
        result.push(row);
    }
    return result;
}
let m1 = mat_create(4, 4, 1);
let m2 = mat_create(4, 4, 2);
let result;
for (let iter = 0; iter < 50; iter++) result = mat_multiply(m1, m2, 4);
console.log(`matrix[0][0] = ${result[0][0]}`);
console.log(`matrix[3][3] = ${result[3][3]}`);

// 3. Quicksort
function quicksort(arr, lo, hi) {
    if (lo >= hi) return arr;
    let pivot = arr[hi], i = lo;
    for (let j = lo; j < hi; j++) {
        if (arr[j] < pivot) { [arr[i], arr[j]] = [arr[j], arr[i]]; i++; }
    }
    [arr[i], arr[hi]] = [arr[hi], arr[i]];
    quicksort(arr, lo, i-1);
    quicksort(arr, i+1, hi);
    return arr;
}
let data = [], seed = 98765;
for (let i = 0; i < 1000; i++) {
    seed = (seed * 1103515245 + 12345) % 2147483648;
    data.push(seed % 10000);
}
let sorted = quicksort(data, 0, 999);
let is_sorted = sorted.every((v, i) => i === 0 || sorted[i-1] <= v);
console.log(`sorted: ${is_sorted}`);
console.log(`min: ${sorted[0]}, max: ${sorted[999]}`);

// 4. Sieve
function sieve(limit) {
    let flags = new Array(limit).fill(true);
    for (let i = 2; i*i < limit; i++) {
        if (flags[i]) for (let j = i*i; j < limit; j += i) flags[j] = false;
    }
    let primes = [];
    for (let i = 2; i < limit; i++) if (flags[i]) primes.push(i);
    return primes;
}
let primes = sieve(10000);
console.log(`primes count: ${primes.length}`);
console.log(`largest prime < 10000: ${primes[primes.length-1]}`);
console.log(`prime sum: ${primes.reduce((a,b) => a+b, 0)}`);

// 5. Strings
let words = ["the", "quick", "brown", "fox", "jumps", "over", "the", "lazy", "dog"];
console.log(`sentence: ${words.join(" ")}`);
console.log(`sentence len: ${words.join(" ").length}`);
console.log(`reversed: ${[...words].reverse().join(" ")}`);

// 6. Iterative fib
function fib_iter(n) {
    if (n <= 1) return n;
    let a = 0, b = 1;
    for (let i = 2; i <= n; i++) { [a, b] = [b, a+b]; }
    return b;
}
console.log(`fib_iter(50) = ${fib_iter(50)}`);

// 7. Collatz
function collatz_len(n) {
    let steps = 0;
    while (n !== 1) { n = n % 2 === 0 ? n/2 : n*3+1; steps++; }
    return steps;
}
let max_c = 0, max_s = 0;
for (let i = 1; i < 10000; i++) {
    let c = collatz_len(i);
    if (c > max_c) { max_c = c; max_s = i; }
}
console.log(`longest collatz < 10000: start=${max_s} steps=${max_c}`);

// 8. Enum
console.log("status 2 = done");

// 9. Lambda
function apply_twice(f, x) { return f(f(x)); }
console.log(`triple twice 2 = ${apply_twice(x => x*3, 2)}`);

// 10. Students
let students = [
    {name: "Alice", grades: [90, 85, 92, 88]},
    {name: "Bob", grades: [78, 82, 75, 80]},
    {name: "Charlie", grades: [95, 98, 92, 97]},
    {name: "Diana", grades: [88, 91, 85, 90]},
];
for (let s of students) {
    let avg = Math.floor(s.grades.reduce((a,b) => a+b, 0) / s.grades.length);
    console.log(`${s.name}: avg = ${avg}`);
}

// 11. Error handling
function safe_div(a, b) { if (b === 0) throw "division by zero"; return Math.floor(a/b); }
try { safe_div(10, 0); console.log("unexpected"); } catch(e) { console.log("caught division by zero"); }
try { console.log(`10 / 2 = ${safe_div(10, 2)}`); } catch(e) {}

// 12. Nested loops
let total = 0;
for (let i = 0; i < 200; i++)
    for (let j = 0; j < 200; j++)
        if ((i+j) % 3 === 0) total += i*j;
console.log(`nested computation = ${total}`);

// 13. Object mutation
let counter = {value: 0, increments: 0};
for (let i = 0; i < 10000; i++) { counter.value++; counter.increments++; }
console.log(`counter = ${counter.value}, increments = ${counter.increments}`);

// 14. Even squares
let squares = Array.from({length: 100}, (_, i) => i*i);
let even_squares = squares.filter(s => s % 2 === 0);
console.log(`even squares count = ${even_squares.length}`);
console.log(`last even square = ${even_squares[even_squares.length-1]}`);

console.log("=== STRESS TEST COMPLETE ===");
