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

        respond(&mut stdout, dispatch(method, id, &params));
    }
}

/// One request in, one response out. Split from the read loop so the protocol
/// can be tested without a transport.
fn dispatch(method: &str, id: Value, params: &Value) -> Value {
    match method {
        "initialize" => ok(id, json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") },
        })),
        "tools/list" => ok(id, json!({ "tools": tool_schemas() })),
        "tools/call" => call_tool(id, params),
        // Every MCP revision requires a receiver to answer ping with an empty
        // result. Answering `-32601 unknown method` makes a client's liveness
        // check look like a dead server.
        "ping" => ok(id, json!({})),
        _ => error(id, -32601, &format!("unknown method `{method}`")),
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

/// The CLI arguments for a tool call.
///
/// **Nothing is dropped silently.** This used to `filter_map` over `as_str`,
/// so `{"args": ["sim", "scene.loom", "--ticks", 300]}` ran
/// `loom sim scene.loom --ticks` — a different command than the one asked
/// for — and reported it as a success. Writing a number unquoted is an
/// ordinary mistake for a model generating JSON, and it has to either work or
/// say why not.
///
/// Numbers and booleans have one obvious spelling as an argument, so they are
/// accepted (§7's "liberal in what we accept"). A null, object or array does
/// not, so it is refused by index.
///
/// # Errors
/// A message naming the offending index, for `-32602 invalid params`.
fn tool_args(params: &Value) -> Result<Vec<String>, String> {
    let Some(raw) = params.get("arguments").and_then(|a| a.get("args")) else {
        return Ok(Vec::new());
    };
    let Some(list) = raw.as_array() else {
        return Err("`args` must be an array of strings".to_owned());
    };

    list.iter()
        .enumerate()
        .map(|(i, v)| match v {
            Value::String(s) => Ok(s.clone()),
            Value::Number(n) => Ok(n.to_string()),
            Value::Bool(b) => Ok(b.to_string()),
            _ => Err(format!(
                "`args[{i}]` is {}, which has no argument spelling — pass a string",
                match v {
                    Value::Null => "null",
                    Value::Array(_) => "an array",
                    _ => "an object",
                }
            )),
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

    let args = match tool_args(params) {
        Ok(a) => a,
        Err(e) => return error(id, -32602, &e),
    };

    // The CLI binary sits beside this one, so a checkout works without
    // anything on PATH.
    let loom = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("loom")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| std::path::PathBuf::from("loom"));

    match run_bounded(&loom, subcommand, &args) {
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

/// Read a child pipe to the end on its own thread.
///
/// Separate from the wait loop on purpose: a child that fills a pipe buffer
/// blocks until someone reads it, so polling `try_wait` without draining would
/// hang exactly the long-running commands the timeout exists to bound.
fn drain<R: std::io::Read + Send + 'static>(
    pipe: Option<R>,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(mut pipe) = pipe {
            let _: std::io::Result<usize> = pipe.read_to_end(&mut buffer);
        }
        buffer
    })
}

/// How often to check whether the child has finished.
const POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// How long any one tool call may take before it is killed.
///
/// The server answers one request at a time, so a command that never returns
/// wedges the whole thing: no further tool call is served and the client has
/// no way to cancel. A render or a long sim is legitimately slow, so the bound
/// is generous — it exists to catch "never", not "slow".
const TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Run the CLI, killing it if it outlives [`TOOL_TIMEOUT`].
///
/// `Command::output` waits forever. This spawns instead and polls, draining
/// both pipes on their own threads — a child that fills a pipe buffer blocks
/// until someone reads it, so polling without draining would hang exactly the
/// commands most worth bounding.
fn run_bounded(
    loom: &std::path::Path,
    subcommand: &str,
    args: &[String],
) -> std::io::Result<std::process::Output> {
    let mut child = std::process::Command::new(loom)
        .arg(subcommand)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());

    // Counted in poll intervals rather than read off the clock: never-do #8
    // bans the wall clock workspace-wide and this needs no exemption. It
    // measures time spent sleeping, so a slow `try_wait` makes the bound
    // longer, never shorter — it can only ever wait too long, not kill early.
    let polls_allowed = TOOL_TIMEOUT.as_millis() / POLL.as_millis();
    let mut polls: u128 = 0;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if polls >= polls_allowed {
            let _ = child.kill();
            let status = child.wait()?;
            let _ = out.join();
            let _ = err.join();
            return Ok(std::process::Output {
                status,
                stdout: Vec::new(),
                stderr: format!(
                    "`loom {subcommand}` exceeded {}s and was killed",
                    TOOL_TIMEOUT.as_secs()
                )
                .into_bytes(),
            });
        }
        std::thread::sleep(POLL);
        polls += 1;
    };

    Ok(std::process::Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model generating JSON writes `300`, not `"300"`, more often than not.
    /// Dropping it ran a different command and called it a success.
    #[test]
    fn a_numeric_argument_is_kept_not_dropped() {
        let params = json!({ "arguments": { "args": ["scene.loom", "--ticks", 300] } });

        let args = tool_args(&params).expect("numbers have one obvious spelling");

        assert_eq!(args, ["scene.loom", "--ticks", "300"]);
    }

    #[test]
    fn an_unrepresentable_argument_is_refused_by_index() {
        let params = json!({ "arguments": { "args": ["scene.loom", null] } });

        let e = tool_args(&params).expect_err("null is not an argument");

        assert!(e.contains("args[1]"), "should name the index: {e}");
    }

    #[test]
    fn ping_is_answered_with_an_empty_result() {
        let response = dispatch("ping", json!(1), &json!({}));

        assert_eq!(response["result"], json!({}));
        assert!(response.get("error").is_none(), "ping is not an error: {response}");
    }

    #[test]
    fn an_unknown_method_is_still_an_error() {
        let response = dispatch("nope", json!(1), &json!({}));

        assert_eq!(response["error"]["code"], -32601);
    }
}
