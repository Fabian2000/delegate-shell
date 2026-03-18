// JS: loop, string ops, arithmetic
let s = 0;
for (let i = 1; i <= 10000; i++) {
    s += i;
}
console.log(`Sum: ${s}`);

// String building
let result = "";
for (let i = 1; i <= 100; i++) {
    result = result + "x";
}
console.log(`Len: ${result.length}`);

// Array ops
let arr = [];
for (let i = 1; i <= 1000; i++) {
    arr.push(i);
}
console.log(`Array len: ${arr.length}`);
