let text = "the quick brown fox jumps over the lazy dog the fox the dog the the quick quick";
let words = text.split(" ");

let keys = [];
let vals = [];

function find_idx(keys, w) {
    for (let i = 0; i < keys.length; i++) {
        if (keys[i] === w) return i;
    }
    return -1;
}

for (let w of words) {
    let idx = find_idx(keys, w);
    if (idx === -1) {
        keys.push(w);
        vals.push(1);
    } else {
        vals[idx] = vals[idx] + 1;
    }
}

for (let i = 0; i < keys.length; i++) {
    console.log(`${keys[i]}: ${vals[i]}`);
}
