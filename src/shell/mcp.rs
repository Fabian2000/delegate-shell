use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::collections::HashMap;
use std::thread;
use serde_json::{json, Value as JsonValue};
use delegate_shell::{Runtime, DebugAction};
use delegate_shell::interpreter::value::Value;
use delegate_shell::builtins::registry::{Param, Type};

// ---------------------------------------------------------------------------
// Shared output buffer
// ---------------------------------------------------------------------------
type OutputBuffer = Arc<Mutex<String>>;

fn register_mcp_overrides(engine: &mut Runtime, buf: &OutputBuffer, input_rx: Option<Arc<Mutex<mpsc::Receiver<String>>>>) {
    let b = buf.clone();
    let _ = engine.register_override("print", &[Param::Required(Type::String)], Type::Void, move |args, _| {
        b.lock().unwrap_or_else(|e| e.into_inner()).push_str(&args[0].to_string());
        Ok(Value::void())
    });
    let b = buf.clone();
    let _ = engine.register_override("println", &[Param::Required(Type::String)], Type::Void, move |args, _| {
        let mut lock = b.lock().unwrap_or_else(|e| e.into_inner());
        lock.push_str(&args[0].to_string());
        lock.push('\n');
        Ok(Value::void())
    });
    let b = buf.clone();
    let _ = engine.register_override("errprint", &[Param::Required(Type::String)], Type::Void, move |args, _| {
        b.lock().unwrap_or_else(|e| e.into_inner()).push_str(&format!("[stderr] {}", args[0]));
        Ok(Value::void())
    });
    let b = buf.clone();
    let _ = engine.register_override("errprintln", &[Param::Required(Type::String)], Type::Void, move |args, _| {
        let mut lock = b.lock().unwrap_or_else(|e| e.into_inner());
        lock.push_str(&format!("[stderr] {}", args[0]));
        lock.push('\n');
        Ok(Value::void())
    });

    // Override input() to wait for MCP-provided input
    if let Some(rx) = input_rx {
        let b = buf.clone();
        let _ = engine.register_override("input", &[Param::Required(Type::String)], Type::String, move |args, _| {
            // Write the prompt to the output buffer
            b.lock().unwrap_or_else(|e| e.into_inner()).push_str(&args[0].to_string());
            // Block until provide_input sends a value
            match rx.lock().unwrap_or_else(|e| e.into_inner()).recv() {
                Ok(input) => Ok(Value::string_from(&input)),
                Err(_) => Err("input(): session closed".to_string()),
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Session management
// ---------------------------------------------------------------------------
enum SessionState {
    /// Script running, may be waiting for input
    Running {
        output_buf: OutputBuffer,
        input_tx: mpsc::Sender<String>,
        /// Receives: (output_so_far, waiting_for_input, finished, error)
        result_rx: mpsc::Receiver<SessionResult>,
    },
    /// Debug session running
    Debugging {
        output_buf: OutputBuffer,
        input_tx: mpsc::Sender<String>,
        action_tx: mpsc::Sender<DebugAction>,
        result_rx: mpsc::Receiver<SessionResult>,
    },
}

struct SessionResult {
    output: String,
    waiting_for_input: bool,
    finished: bool,
    error: Option<String>,
    debug_stop: Option<JsonValue>,
}

struct SessionManager {
    sessions: HashMap<String, SessionState>,
    next_id: u64,
}

impl SessionManager {
    fn new() -> Self {
        Self { sessions: HashMap::new(), next_id: 1 }
    }

    fn new_id(&mut self) -> String {
        let id = format!("s{}", self.next_id);
        self.next_id += 1;
        id
    }

    fn start_run(&mut self, code: String) -> (String, SessionResult) {
        let session_id = self.new_id();
        let output_buf: OutputBuffer = Arc::new(Mutex::new(String::new()));
        let (input_tx, input_rx) = mpsc::channel::<String>();
        let (result_tx, result_rx) = mpsc::channel::<SessionResult>();
        let input_rx = Arc::new(Mutex::new(input_rx));

        let buf = output_buf.clone();
        let irx = input_rx.clone();

        thread::spawn(move || {
            let mut engine = match Runtime::new() {
                Ok(e) => e,
                Err(e) => {
                    let _ = result_tx.send(SessionResult {
                        output: String::new(),
                        waiting_for_input: false,
                        finished: true,
                        error: Some(e),
                        debug_stop: None,
                    });
                    return;
                }
            };
            let _ = engine.set_execution_mode(delegate_shell::ExecutionMode::TreeWalk);
            register_mcp_overrides(&mut engine, &buf, None);

            // Register a special input hook: when input() blocks, signal the main thread
            let signal_buf = buf.clone();
            let signal_tx = result_tx.clone();
            let _ = engine.register_override("input", &[Param::Required(Type::String)], Type::String, move |args, _| {
                signal_buf.lock().unwrap_or_else(|e| e.into_inner()).push_str(&args[0].to_string());
                let output = signal_buf.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let _ = signal_tx.send(SessionResult {
                    output,
                    waiting_for_input: true,
                    finished: false,
                    error: None,
                    debug_stop: None,
                });
                // Now wait for input
                match input_rx.lock().unwrap_or_else(|e| e.into_inner()).recv() {
                    Ok(input) => Ok(Value::string_from(&input)),
                    Err(_) => Err("input(): session closed".to_string()),
                }
            });

            let result = engine.run_source(&code);
            let output = buf.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let _ = result_tx.send(SessionResult {
                output,
                waiting_for_input: false,
                finished: true,
                error: result.err(),
                debug_stop: None,
            });
        });

        // Wait for first result (either input-wait or completion)
        let first_result = result_rx.recv().unwrap_or(SessionResult {
            output: String::new(),
            waiting_for_input: false,
            finished: true,
            error: Some("Session thread died".to_string()),
            debug_stop: None,
        });

        if !first_result.finished {
            self.sessions.insert(session_id.clone(), SessionState::Running {
                output_buf,
                input_tx,
                result_rx,
            });
        }

        (session_id, first_result)
    }

    fn start_debug(&mut self, code: String) -> (String, SessionResult) {
        let session_id = self.new_id();
        let output_buf: OutputBuffer = Arc::new(Mutex::new(String::new()));
        let (input_tx, input_rx) = mpsc::channel::<String>();
        let (action_tx, action_rx) = mpsc::channel::<DebugAction>();
        let (result_tx, result_rx) = mpsc::channel::<SessionResult>();
        let input_rx = Arc::new(Mutex::new(input_rx));

        let buf = output_buf.clone();

        thread::spawn(move || {
            let mut engine = match Runtime::new() {
                Ok(e) => e,
                Err(e) => {
                    let _ = result_tx.send(SessionResult {
                        output: String::new(),
                        waiting_for_input: false,
                        finished: true,
                        error: Some(e),
                        debug_stop: None,
                    });
                    return;
                }
            };
            let _ = engine.set_execution_mode(delegate_shell::ExecutionMode::TreeWalk);
            register_mcp_overrides(&mut engine, &buf, Some(input_rx));

            let dbg_buf = buf.clone();
            let dbg_tx = result_tx.clone();
            engine.on_debug(move |ctx| {
                let snapshot = json!({
                    "line": ctx.line,
                    "column": ctx.column,
                    "file": ctx.file,
                    "function": ctx.function_name,
                    "variables": ctx.variables.iter()
                        .map(|(name, val, type_name)| json!({"name": name, "value": val, "type": type_name}))
                        .collect::<Vec<_>>(),
                    "call_stack": ctx.call_stack,
                    "source": ctx.source_context.iter()
                        .map(|(num, content, is_current)| json!({"line": num, "content": content, "current": is_current}))
                        .collect::<Vec<_>>()
                });
                let output = dbg_buf.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let _ = dbg_tx.send(SessionResult {
                    output,
                    waiting_for_input: false,
                    finished: false,
                    error: None,
                    debug_stop: Some(snapshot),
                });
                // Wait for next action from MCP
                action_rx.recv().unwrap_or(DebugAction::Quit)
            });

            let result = engine.run_source(&code);
            let output = buf.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let _ = result_tx.send(SessionResult {
                output,
                waiting_for_input: false,
                finished: true,
                error: result.err(),
                debug_stop: None,
            });
        });

        let first_result = result_rx.recv().unwrap_or(SessionResult {
            output: String::new(),
            waiting_for_input: false,
            finished: true,
            error: Some("Session thread died".to_string()),
            debug_stop: None,
        });

        if !first_result.finished {
            self.sessions.insert(session_id.clone(), SessionState::Debugging {
                output_buf,
                input_tx,
                action_tx,
                result_rx,
            });
        }

        (session_id, first_result)
    }

    fn provide_input(&mut self, session_id: &str, input: String) -> Option<SessionResult> {
        let session = self.sessions.get(session_id)?;
        match session {
            SessionState::Running { input_tx, result_rx, output_buf, .. } => {
                // Clear the buffer for next segment
                output_buf.lock().unwrap_or_else(|e| e.into_inner()).clear();
                let _ = input_tx.send(input);
                let result = result_rx.recv().ok()?;
                if result.finished {
                    self.sessions.remove(session_id);
                }
                Some(result)
            }
            SessionState::Debugging { input_tx, result_rx, output_buf, .. } => {
                output_buf.lock().unwrap_or_else(|e| e.into_inner()).clear();
                let _ = input_tx.send(input);
                let result = result_rx.recv().ok()?;
                if result.finished {
                    self.sessions.remove(session_id);
                }
                Some(result)
            }
        }
    }

    fn debug_step(&mut self, session_id: &str, action: DebugAction) -> Option<SessionResult> {
        let session = self.sessions.get(session_id)?;
        match session {
            SessionState::Debugging { action_tx, result_rx, output_buf, .. } => {
                output_buf.lock().unwrap_or_else(|e| e.into_inner()).clear();
                let _ = action_tx.send(action);
                let result = result_rx.recv().ok()?;
                if result.finished {
                    self.sessions.remove(session_id);
                }
                Some(result)
            }
            _ => None,
        }
    }

    fn end_session(&mut self, session_id: &str) -> bool {
        self.sessions.remove(session_id).is_some()
        // Dropping the session drops the channels, which unblocks the thread
    }
}

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------
pub fn run_mcp_server() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let output_buf: OutputBuffer = Arc::new(Mutex::new(String::new()));
    let mut engine = Runtime::new().unwrap_or_else(|e| {
        eprintln!("Failed to initialize runtime: {e}");
        std::process::exit(1);
    });
    let _ = engine.set_execution_mode(delegate_shell::ExecutionMode::TreeWalk);
    register_mcp_overrides(&mut engine, &output_buf, None);

    let mut sessions = SessionManager::new();

    let reader = stdin.lock();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l.trim().to_string(),
            Err(_) => break,
        };
        if line.is_empty() {
            continue;
        }

        let request: JsonValue = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                send_error(&stdout, JsonValue::Null, -32700, &format!("Parse error: {e}"));
                continue;
            }
        };

        let id = request.get("id").cloned().unwrap_or(JsonValue::Null);
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");

        match method {
            "initialize" => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": "dgsh",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                });
                send_response(&stdout, &response);
            }
            "notifications/initialized" => {}
            "tools/list" => {
                let tools = json!([
                    {
                        "name": "run_source",
                        "description": "Execute dgsh source code. Returns output. If the script calls input(), returns partial output with waiting_for_input=true and a session_id. Use provide_input to continue.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "code": { "type": "string", "description": "The dgsh source code to execute" }
                            },
                            "required": ["code"]
                        }
                    },
                    {
                        "name": "run_file",
                        "description": "Execute a dgsh script file. Same session behavior as run_source.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string", "description": "Path to the .dgsh file" }
                            },
                            "required": ["path"]
                        }
                    },
                    {
                        "name": "provide_input",
                        "description": "Send input to a running session that is waiting for input(). Returns the next output segment.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "session_id": { "type": "string", "description": "The session ID" },
                                "input": { "type": "string", "description": "The input text to provide" }
                            },
                            "required": ["session_id", "input"]
                        }
                    },
                    {
                        "name": "debug_start",
                        "description": "Start a debug session. The script runs until the first debugger() call, then pauses and returns variables, call stack, and source context. Use debug_step to continue.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "code": { "type": "string", "description": "The dgsh source code to debug" }
                            },
                            "required": ["code"]
                        }
                    },
                    {
                        "name": "debug_step",
                        "description": "Continue a paused debug session with an action.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "session_id": { "type": "string", "description": "The debug session ID" },
                                "action": { "type": "string", "enum": ["step_over", "step_into", "continue", "quit"], "description": "Debug action" }
                            },
                            "required": ["session_id", "action"]
                        }
                    },
                    {
                        "name": "session_end",
                        "description": "End a running or debug session and free resources.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "session_id": { "type": "string", "description": "The session ID to end" }
                            },
                            "required": ["session_id"]
                        }
                    },
                    {
                        "name": "lint",
                        "description": "Parse dgsh code and check for syntax errors without executing it",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "code": { "type": "string", "description": "The dgsh source code to check" }
                            },
                            "required": ["code"]
                        }
                    },
                    {
                        "name": "list_builtins",
                        "description": "List all available built-in functions",
                        "inputSchema": { "type": "object", "properties": {} }
                    }
                ]);
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "tools": tools }
                });
                send_response(&stdout, &response);
            }
            "tools/call" => {
                let params = request.get("params").cloned().unwrap_or(json!({}));
                let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

                let result = match tool_name {
                    "run_source" => {
                        let code = arguments.get("code").and_then(|c| c.as_str()).unwrap_or("").to_string();
                        let (sid, sr) = sessions.start_run(code);
                        format_session_result(&sid, &sr)
                    }
                    "run_file" => {
                        let path = arguments.get("path").and_then(|p| p.as_str()).unwrap_or("");
                        match std::fs::read_to_string(path) {
                            Ok(code) => {
                                let (sid, sr) = sessions.start_run(code);
                                format_session_result(&sid, &sr)
                            }
                            Err(e) => json!({
                                "content": [{"type": "text", "text": format!("Cannot read '{path}': {e}")}],
                                "isError": true
                            }),
                        }
                    }
                    "provide_input" => {
                        let sid = arguments.get("session_id").and_then(|s| s.as_str()).unwrap_or("");
                        let input = arguments.get("input").and_then(|i| i.as_str()).unwrap_or("").to_string();
                        match sessions.provide_input(sid, input) {
                            Some(sr) => format_session_result(sid, &sr),
                            None => json!({
                                "content": [{"type": "text", "text": format!("Session '{sid}' not found or not waiting")}],
                                "isError": true
                            }),
                        }
                    }
                    "debug_start" => {
                        let code = arguments.get("code").and_then(|c| c.as_str()).unwrap_or("").to_string();
                        let (sid, sr) = sessions.start_debug(code);
                        format_session_result(&sid, &sr)
                    }
                    "debug_step" => {
                        let sid = arguments.get("session_id").and_then(|s| s.as_str()).unwrap_or("");
                        let action = match arguments.get("action").and_then(|a| a.as_str()).unwrap_or("step_over") {
                            "continue" => DebugAction::Continue,
                            "step_into" => DebugAction::StepInto,
                            "quit" => DebugAction::Quit,
                            _ => DebugAction::StepOver,
                        };
                        match sessions.debug_step(sid, action) {
                            Some(sr) => format_session_result(sid, &sr),
                            None => json!({
                                "content": [{"type": "text", "text": format!("Debug session '{sid}' not found")}],
                                "isError": true
                            }),
                        }
                    }
                    "session_end" => {
                        let sid = arguments.get("session_id").and_then(|s| s.as_str()).unwrap_or("");
                        let ended = sessions.end_session(sid);
                        json!({
                            "content": [{"type": "text", "text": if ended { "Session ended" } else { "Session not found" }}]
                        })
                    }
                    "lint" => tool_lint(&arguments),
                    "list_builtins" => tool_list_builtins(&engine),
                    _ => json!({
                        "content": [{"type": "text", "text": format!("Unknown tool: {tool_name}")}],
                        "isError": true
                    }),
                };

                let response = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result
                });
                send_response(&stdout, &response);
            }
            _ => {
                send_error(&stdout, id, -32601, &format!("Method not found: {method}"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Format session result as MCP response
// ---------------------------------------------------------------------------
fn format_session_result(session_id: &str, sr: &SessionResult) -> JsonValue {
    let mut result = json!({
        "session_id": session_id,
        "output": sr.output,
        "finished": sr.finished,
        "waiting_for_input": sr.waiting_for_input,
    });

    if let Some(ref err) = sr.error {
        result["error"] = json!(err);
    }
    if let Some(ref stop) = sr.debug_stop {
        result["debug_stop"] = stop.clone();
    }

    // Format as MCP content
    let text = serde_json::to_string_pretty(&result).unwrap_or_default();
    json!({
        "content": [{"type": "text", "text": text}]
    })
}

// ---------------------------------------------------------------------------
// Simple (non-session) tools
// ---------------------------------------------------------------------------
fn tool_lint(args: &JsonValue) -> JsonValue {
    let code = args.get("code").and_then(|c| c.as_str()).unwrap_or("");

    let mut check_engine = match Runtime::new() {
        Ok(e) => e,
        Err(e) => {
            return json!({
                "content": [{"type": "text", "text": format!("Init error: {e}")}],
                "isError": true
            });
        }
    };
    match check_engine.run_source(code) {
        Ok(()) => json!({
            "content": [{"type": "text", "text": "OK: no errors"}]
        }),
        Err(e) => json!({
            "content": [{"type": "text", "text": format!("Error: {e}")}],
            "isError": true
        }),
    }
}

fn tool_list_builtins(engine: &Runtime) -> JsonValue {
    let names = engine.builtin_names();
    let text = names.join(", ");
    json!({
        "content": [{"type": "text", "text": format!("{} builtins: {text}", names.len())}]
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
fn send_response(stdout: &io::Stdout, response: &JsonValue) {
    let msg = serde_json::to_string(response).unwrap_or_default();
    let mut out = stdout.lock();
    let _ = writeln!(out, "{msg}");
    let _ = out.flush();
}

fn send_error(stdout: &io::Stdout, id: JsonValue, code: i32, message: &str) {
    let response = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    });
    send_response(stdout, &response);
}
