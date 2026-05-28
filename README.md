# Delegate Shell (dgsh)

A hybrid shell language that combines the directness of shell scripting with the structure of a programming language. Use it as a script runner, an interactive shell, or embed it as a scripting engine in your own applications.

## Quick Install

**Linux / macOS:**

```bash
curl -sSL https://raw.githubusercontent.com/Fabian2000/delegate-shell/main/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/Fabian2000/delegate-shell/main/install.ps1 | iex
```

## Hello World

```dgsh
#!/usr/bin/env dgsh
println("Hello, World!")
```

```bash
dgsh hello.dgsh
```

Or start the interactive REPL:

```bash
dgsh
```

## Features

- **Typed scripting.** Variables are typed at first assignment. Type annotations, structural object typing, `dyn` for dynamic contexts.
- **170+ built-in functions.** Collections, strings, math, file I/O, HTTP, JSON/YAML/TOML, hashing, threads, terminal styling, and more.
- **Fast execution.** Bytecode VM with JIT compilation. Beats Python on most workloads, competitive with Node.js.
- **Embeddable.** Use as a library with a clean API. Register custom functions, sandbox scripts, control execution modes.
- **C-ABI FFI.** Call dgsh from C, Python, Zig, Go, or any language with C interop.
- **MCP Server.** Built-in support for AI tools (Claude Code, Cursor) to execute, debug, and analyze scripts.
- **Interactive REPL.** Syntax highlighting, tab completion, command history, `.dgsh` config file.
- **Debugger.** `debugger()` builtin with step-over, step-into, continue. Works in CLI and via MCP.
- **Sandbox.** Restrict file system, network, exec access, or limit to core builtins only.

## Benchmarks

| Test                  | dgsh JIT | Python  | Node.js |
| --------------------- | -------- | ------- | ------- |
| Stress (14 subtests)  | 0.05s    | 0.08s   | 0.10s   |
| Fibonacci(40)         | 1.2s     | 13.8s   | 1.1s    |
| Quicksort (5000)      | 0.06s    | 0.07s   | 0.10s   |
| Primes < 100k         | 0.09s    | 0.15s   | 0.10s   |
| String concat (50k)   | 0.03s    | 0.04s   | 0.09s   |

## Quick Example

```dgsh
# Fetch data from an API
response = http_get!!("https://jsonplaceholder.typicode.com/todos/1")
data = from_json!!(response.body)
println("Title: {data.title}")

# Functions with type annotations
add(a: int, b: int): int
    return a + b

println("Sum: {add!(3, 4)}")

# Error handling
risky(x)
    if x < 0
        throw "negative"
    return x * 2

result? = risky!(-5)
if result?
    println("Error: {result?.error}")
else
    println("OK: {result}")

# Lambda and higher-order functions
double(n)
    return n * 2

numbers = [1, 2, 3, 4, 5]
doubled = map!!(numbers, @double)
println("{doubled}")
```

## Documentation

Full documentation is available in the [Wiki](https://github.com/Fabian2000/delegate-shell/wiki):

- [Getting Started](https://github.com/Fabian2000/delegate-shell/wiki/Getting-Started)
- [Language Reference](https://github.com/Fabian2000/delegate-shell/wiki/Language-Reference)
- [System Functions](https://github.com/Fabian2000/delegate-shell/wiki/System-Functions)
- [Examples](https://github.com/Fabian2000/delegate-shell/wiki/Examples)
- [Installation & Embedding](https://github.com/Fabian2000/delegate-shell/wiki/Installation)
- [MCP Server](https://github.com/Fabian2000/delegate-shell/wiki/MCP-Server)
- [Migration Guide (experimental)](https://github.com/Fabian2000/delegate-shell/wiki/Migration-Guide)

## License

MIT
