import sys

levels = ["INFO", "WARN", "ERROR", "DEBUG", "INFO", "INFO", "ERROR", "WARN", "INFO", "DEBUG"]
services = ["auth", "api", "db", "cache", "worker", "gateway", "scheduler", "monitor"]
messages = ["request completed", "connection timeout", "cache miss", "query slow", "rate limited", "health check ok", "retry attempt", "disk usage high"]

logs = []
seed = 12345
for i in range(5000):
    seed = (seed * 1103515245 + 12345) % 2147483648
    level = levels[seed % len(levels)]
    seed2 = (seed * 7 + 3) % len(services)
    service = services[seed2]
    seed3 = (seed * 13 + 5) % len(messages)
    msg = messages[seed3]
    logs.append({"level": level, "service": service, "message": msg, "ts": i})

# Count by level
level_counts = {"INFO": 0, "WARN": 0, "ERROR": 0, "DEBUG": 0}
for log in logs:
    level_counts[log["level"]] += 1

print("=== Log Level Distribution ===")
for level in ["INFO", "WARN", "ERROR", "DEBUG"]:
    print(f"{level}: {level_counts[level]}")

# Count errors by service
svc_errors = {s: 0 for s in services}
for log in logs:
    if log["level"] == "ERROR":
        svc_errors[log["service"]] += 1

print("=== Errors by Service ===")
for svc in services:
    if svc_errors[svc] > 0:
        print(f"{svc}: {svc_errors[svc]}")

# Longest streak
max_streak = 0
current_streak = 1
prev_level = logs[0]["level"]
for i in range(1, len(logs)):
    if logs[i]["level"] == prev_level:
        current_streak += 1
        if current_streak > max_streak:
            max_streak = current_streak
    else:
        current_streak = 1
    prev_level = logs[i]["level"]

print("=== Longest Streak ===")
print(f"Max consecutive same level: {max_streak}")

# Moving average error rate
window = 100
error_flags = [1 if log["level"] == "ERROR" else 0 for log in logs]
max_rate = 0
for i in range(len(error_flags) - window + 1):
    s = sum(error_flags[i:i+window])
    if s > max_rate:
        max_rate = s

print("=== Error Rate ===")
print(f"Peak error rate (per {window}): {max_rate}")

# Top error messages
err_msgs = []
err_counts = []
for log in logs:
    if log["level"] == "ERROR":
        msg = log["message"]
        if msg in err_msgs:
            idx = err_msgs.index(msg)
            err_counts[idx] += 1
        else:
            err_msgs.append(msg)
            err_counts.append(1)

paired = sorted(zip(err_counts, err_msgs), reverse=True)
print("=== Top Error Messages ===")
for i, (count, msg) in enumerate(paired[:3]):
    print(f"{i+1}. {msg} ({count}x)")

print(f"=== Done: processed {len(logs)} log entries ===")
