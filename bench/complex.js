const levels = ["INFO", "WARN", "ERROR", "DEBUG", "INFO", "INFO", "ERROR", "WARN", "INFO", "DEBUG"];
const services = ["auth", "api", "db", "cache", "worker", "gateway", "scheduler", "monitor"];
const messages = ["request completed", "connection timeout", "cache miss", "query slow", "rate limited", "health check ok", "retry attempt", "disk usage high"];

let logs = [];
let seed = 12345;
for (let i = 0; i < 5000; i++) {
    seed = (seed * 1103515245 + 12345) % 2147483648;
    let level = levels[seed % levels.length];
    let seed2 = (seed * 7 + 3) % services.length;
    let service = services[seed2];
    let seed3 = (seed * 13 + 5) % messages.length;
    let msg = messages[seed3];
    logs.push({level, service, message: msg, ts: i});
}

// Count by level
let level_counts = {INFO: 0, WARN: 0, ERROR: 0, DEBUG: 0};
for (let log of logs) level_counts[log.level]++;

console.log("=== Log Level Distribution ===");
for (let level of ["INFO", "WARN", "ERROR", "DEBUG"]) {
    console.log(`${level}: ${level_counts[level]}`);
}

// Count errors by service
let svc_errors = {};
for (let s of services) svc_errors[s] = 0;
for (let log of logs) {
    if (log.level === "ERROR") svc_errors[log.service]++;
}

console.log("=== Errors by Service ===");
for (let svc of services) {
    if (svc_errors[svc] > 0) console.log(`${svc}: ${svc_errors[svc]}`);
}

// Longest streak
let max_streak = 0, current_streak = 1, prev_level = logs[0].level;
for (let i = 1; i < logs.length; i++) {
    if (logs[i].level === prev_level) {
        current_streak++;
        if (current_streak > max_streak) max_streak = current_streak;
    } else {
        current_streak = 1;
    }
    prev_level = logs[i].level;
}

console.log("=== Longest Streak ===");
console.log(`Max consecutive same level: ${max_streak}`);

// Moving average error rate
let window = 100;
let error_flags = logs.map(l => l.level === "ERROR" ? 1 : 0);
let max_rate = 0;
for (let i = 0; i <= error_flags.length - window; i++) {
    let s = 0;
    for (let j = i; j < i + window; j++) s += error_flags[j];
    if (s > max_rate) max_rate = s;
}

console.log("=== Error Rate ===");
console.log(`Peak error rate (per ${window}): ${max_rate}`);

// Top error messages
let err_msgs = [], err_counts = [];
for (let log of logs) {
    if (log.level === "ERROR") {
        let idx = err_msgs.indexOf(log.message);
        if (idx === -1) { err_msgs.push(log.message); err_counts.push(1); }
        else err_counts[idx]++;
    }
}

let paired = err_msgs.map((m, i) => [err_counts[i], m]).sort((a, b) => b[0] - a[0]);
console.log("=== Top Error Messages ===");
for (let i = 0; i < Math.min(3, paired.length); i++) {
    console.log(`${i+1}. ${paired[i][1]} (${paired[i][0]}x)`);
}

console.log(`=== Done: processed ${logs.length} log entries ===`);
