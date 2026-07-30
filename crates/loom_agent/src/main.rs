//! `loom-mcp` — an MCP server over stdio, wrapping the `loom` CLI.
//!
//! **CLI first, MCP second** (brief §7.10). Every tool here shells out to a
//! `loom` subcommand that a shell and `cargo test` already drive. That is not
//! laziness: building the agent interface *with* the agent is circular, and an
//! MCP layer is awkward to test without an agent driving it. Shelling out makes
//! the dependency one-way and provable — if the CLI cannot do it, neither can
//! this.
//!
//! **Catalog mode** (design doc §2.8). A server exposing hundreds of flat tools
//! dumps tens of thousands of tokens of schema into every session before the
//! agent does anything. This exposes a small always-loaded set.

use std::io::{BufRead, Write};

use loom_agent::TOOLS;
use serde_json::{Value, json};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request): Result<Value, _> = serde_json::from_str(&line) else {
            respond(&mut stdout, error(Value::Null, -32700, "invalid JSON"));
            continue;
        };

        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));

        // A notification has no id and takes no response, per JSON-RPC.
        if request.get("id").is_none() {
            continue;
        }

        let response = match method {
            "initialize" => ok(id, json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") },
            })),
            "tools/list" => ok(id, json!({ "tools": tool_schemas() })),
            "tools/call" => call_tool(id, &params),
            _ => error(id, -32601, &format!("unknown method `{method}`")),
        };
        respond(&mut stdout, response);
    }
}

fn respond(out: &mut std::io::Stdout, value: Value) {
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Tool schemas, derived from the catalog so the two cannot drift.
fn tool_schemas() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|(name, command)| {
            json!({
                "name": name,
                "description": format!("Wraps `{command}`."),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "args": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Arguments passed straight to the loom CLI.",
                        }
                    },
                    "required": ["args"],
                },
            })
        })
        .collect()
}

fn call_tool(id: Value, params: &Value) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error(id, -32602, "missing tool name");
    };
    let Some((_, command)) = TOOLS.iter().find(|(t, _)| *t == name) else {
        return error(id, -32602, &format!("unknown tool `{name}`"));
    };
    let Some(subcommand) = command.split_whitespace().nth(1) else {
        return error(id, -32603, "malformed catalog entry");
    };

    let args: Vec<String> = params
        .get("arguments")
        .and_then(|a| a.get("args"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    // The CLI binary sits beside this one, so a checkout works without
    // anything on PATH.
    let loom = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("loom")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| std::path::PathBuf::from("loom"));

    let output = std::process::Command::new(&loom)
        .arg(subcommand)
        .args(&args)
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            // Exit code carried through as `isError`, so a failed validation is
            // a tool result the model can read and retry from rather than a
            // transport-level failure it cannot see.
            ok(
                id,
                json!({
                    "content": [{ "type": "text", "text": if stdout.is_empty() { stderr } else { stdout } }],
                    "isError": !out.status.success(),
                }),
            )
        }
        Err(e) => error(id, -32603, &format!("could not run `{}`: {e}", loom.display())),
    }
}
