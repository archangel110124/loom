//! Sandboxed `rhai` scripting, and the adversarial tests that make the sandbox
//! a claim worth believing.
//!
//! The agent authors behaviour here, which breaks the invariant that made
//! everything else safe: a component field can be schema-validated, and code
//! cannot. Containment comes from two places instead.
//!
//! **The registered surface *is* the sandbox.** There is no filesystem, no
//! network, no process spawning — because those functions were never
//! registered, and Rhai has no ambient access to them.
//!
//! **Hard resource limits**, which is the decisive reason Rhai over Lua here:
//! when the *agent* writes the code, an accidental `while true {}` must be a
//! caught error, not a hung engine.
//!
//! Brief §7.8 is the part that matters: "safe because we only registered safe
//! functions" is a claim about *absence*, and absence is not testable by
//! reading the code. So [`tests`] attempts each escape and asserts it fails.
//! Add a case whenever a new API surface is registered.

use std::collections::BTreeMap;

use rhai::{Dynamic, Engine, EvalAltResult, Scope, AST};
use serde::Serialize;

/// Hard limits. Every one of these is a way an agent-written script can hang
/// or exhaust the engine, so none of them is optional.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Total operations. Catches `while true {}`.
    pub operations: u64,
    /// Call depth, and expression nesting depth.
    pub call_depth: usize,
    pub expr_depth: usize,
    /// Caps on data a script can build, so it cannot exhaust memory.
    pub string_size: usize,
    pub array_size: usize,
    pub map_size: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            operations: 100_000,
            call_depth: 32,
            expr_depth: 64,
            string_size: 64 * 1024,
            array_size: 16 * 1024,
            map_size: 4 * 1024,
        }
    }
}

/// A script failure, in the same shape as every other rejection here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScriptError {
    pub error: String,
    /// Script path, when known.
    pub script: String,
    /// 1-based line, when the engine reports one.
    pub line: Option<usize>,
    pub message: String,
    /// What to do about it — a rejection is the agent's teacher (§6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// What a script may do to the node it is attached to.
///
/// Deliberately tiny. Every entry is a decision to widen the sandbox, and each
/// one needs a matching adversarial test.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeState {
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
}

/// A sandboxed script host.
pub struct ScriptHost {
    engine: Engine,
    compiled: BTreeMap<String, AST>,
    limits: Limits,
}

impl Default for ScriptHost {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

impl ScriptHost {
    /// Build a host with `limits` applied.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        let mut engine = Engine::new();
        engine.set_max_operations(limits.operations);
        engine.set_max_call_levels(limits.call_depth);
        engine.set_max_expr_depths(limits.expr_depth, limits.expr_depth);
        engine.set_max_string_size(limits.string_size);
        engine.set_max_array_size(limits.array_size);
        engine.set_max_map_size(limits.map_size);
        // Modules would let a script pull in anything the host has registered
        // elsewhere, which turns the registered surface into a moving target.
        engine.set_max_modules(0);

        Self {
            engine,
            compiled: BTreeMap::new(),
            limits,
        }
    }

    #[must_use]
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Compile a script and keep it under `name`.
    ///
    /// Compilation is where an unknown call is caught, so a typo fails at load
    /// rather than on frame 900.
    ///
    /// # Errors
    /// [`ScriptError`] describing the syntax or resolution failure.
    pub fn compile(&mut self, name: &str, source: &str) -> Result<(), ScriptError> {
        let ast = self
            .engine
            .compile(source)
            .map_err(|e| self.to_error(name, &EvalAltResult::from(e)))?;
        self.compiled.insert(name.to_owned(), ast);
        Ok(())
    }

    /// Run a compiled script's `on_tick`, and return the node it produced.
    ///
    /// # Errors
    /// [`ScriptError`] if the script is unknown, traps a limit, or throws.
    pub fn tick(&self, name: &str, tick: u64, state: &NodeState) -> Result<NodeState, ScriptError> {
        let Some(ast) = self.compiled.get(name) else {
            return Err(ScriptError {
                error: "unknown_script".to_owned(),
                script: name.to_owned(),
                line: None,
                message: format!("no compiled script named `{name}`"),
                hint: Some("Compile it before ticking it.".to_owned()),
            });
        };

        let mut scope = Scope::new();
        scope.push("tick", i64::try_from(tick).unwrap_or(i64::MAX));
        scope.push("position", to_dynamic_vec(state.position));
        scope.push("rotation", to_dynamic_vec(state.rotation));
        scope.push("scale", to_dynamic_vec(state.scale));

        self.engine
            .run_ast_with_scope(&mut scope, ast)
            .map_err(|e| self.to_error(name, &e))?;

        Ok(NodeState {
            position: from_scope(&scope, "position").unwrap_or(state.position),
            rotation: from_scope(&scope, "rotation").unwrap_or(state.rotation),
            scale: from_scope(&scope, "scale").unwrap_or(state.scale),
        })
    }

    /// Turn a Rhai failure into the project's structured error shape.
    fn to_error(&self, name: &str, err: &EvalAltResult) -> ScriptError {
        let position = err.position();
        let message = err.to_string();

        // The hint is chosen from the failure kind, because "operations
        // exceeded" without "you probably wrote an unbounded loop" is a
        // message the agent cannot act on.
        let (code, hint) = match err {
            EvalAltResult::ErrorTooManyOperations(_) => (
                "script_op_limit",
                Some(format!(
                    "Exceeded {} operations — usually an unbounded loop. \
                     Scripts run every tick; do the work incrementally.",
                    self.limits.operations
                )),
            ),
            EvalAltResult::ErrorStackOverflow(_) => (
                "script_depth_limit",
                Some("Call depth exceeded — usually unbounded recursion.".to_owned()),
            ),
            EvalAltResult::ErrorFunctionNotFound(f, _) => (
                "script_unknown_function",
                Some(format!(
                    "`{f}` is not registered. The script API is deliberately \
                     small: there is no filesystem, network, or process access."
                )),
            ),
            EvalAltResult::ErrorParsing(..) => ("script_parse_error", None),
            _ => ("script_error", None),
        };

        ScriptError {
            error: code.to_owned(),
            script: name.to_owned(),
            line: if position.is_none() {
                None
            } else {
                position.line()
            },
            message,
            hint,
        }
    }
}

fn to_dynamic_vec(v: [f32; 3]) -> Dynamic {
    Dynamic::from_array(
        v.iter()
            .map(|c| Dynamic::from_float(f64::from(*c)))
            .collect(),
    )
}

fn from_scope(scope: &Scope, name: &str) -> Option<[f32; 3]> {
    let value = scope.get_value::<rhai::Array>(name)?;
    let mut out = [0.0_f32; 3];
    for (i, slot) in out.iter_mut().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        {
            *slot = value.get(i)?.as_float().ok()? as f32;
        }
    }
    Some(out)
}

/// Watches script files and reports which changed.
///
/// `ponytail:` polls modification times rather than using `notify`. A handful
/// of scripts polled once a frame costs nothing, and it is a dependency and an
/// OS-specific event stream avoided. Swap to `notify` when watching a whole
/// asset tree, which is a real reason and not this one.
#[derive(Debug, Default)]
pub struct ScriptWatcher {
    seen: BTreeMap<std::path::PathBuf, std::time::SystemTime>,
}

impl ScriptWatcher {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Paths whose modification time changed since the last call.
    ///
    /// A file seen for the first time counts as changed, so the initial load
    /// and every reload go through exactly one code path.
    pub fn changed<'a>(
        &mut self,
        paths: impl IntoIterator<Item = &'a std::path::Path>,
    ) -> Vec<std::path::PathBuf> {
        let mut changed = Vec::new();
        for path in paths {
            let Ok(modified) = std::fs::metadata(path).and_then(|m| m.modified()) else {
                continue;
            };
            if self.seen.get(path) != Some(&modified) {
                self.seen.insert(path.to_path_buf(), modified);
                changed.push(path.to_path_buf());
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {

    /// **The wall clock must not be reachable from a script.** `Engine::new()`
    /// installs rhai's `StandardPackage`, which includes `BasicTimePackage` —
    /// so an agent-authored script could call `timestamp()` and make the
    /// simulation depend on how fast the machine is. That is never-do #8, and
    /// it would break the determinism every `--assert` rests on: two runs of
    /// the same scene could legitimately disagree.
    #[test]
    fn a_script_cannot_read_the_clock() {
        let mut host = host();
        // Compiled AND run: the first version of this test used a script that
        // also touched `pos`, which failed for an unrelated reason and made the
        // test pass while `timestamp()` was perfectly callable. A sandbox test
        // that can pass for the wrong reason is worse than none.
        let compiled = host.compile("clock", "let t = timestamp();");
        let reachable = match compiled {
            Err(_) => false,
            Ok(()) => host.tick("clock", 0, &NodeState::default()).is_ok(),
        };
        assert!(!reachable, "timestamp() must not be callable from a script");
    }
    use super::*;

    fn host() -> ScriptHost {
        ScriptHost::default()
    }

    /// **The M8 exit criterion.** A script rotates a cube.
    #[test]
    fn a_script_rotates_a_node_over_time() {
        let mut host = host();
        host.compile("spin", "rotation[1] = tick.to_float() * 1.5;")
            .expect("valid script");

        let state = NodeState {
            scale: [1.0; 3],
            ..NodeState::default()
        };
        let after = host.tick("spin", 60, &state).expect("script runs");

        assert!((after.rotation[1] - 90.0).abs() < 1e-3, "{after:?}");
        assert_eq!(after.position, state.position, "untouched fields survive");
    }

    // ---------------------------------------------------------------
    // Adversarial sandbox tests (brief §7.8).
    //
    // "Safe because we only registered safe functions" is a claim about
    // absence, and absence is not testable by reading code. Each of these
    // ATTEMPTS an escape and asserts it fails. Add one whenever a new API
    // surface is registered.
    // ---------------------------------------------------------------

    #[test]
    fn a_script_cannot_open_a_file() {
        let mut host = host();
        let result = host.compile("evil", r#"let f = open_file("/etc/passwd");"#);

        // Either it fails to compile or it fails to run — both are containment.
        let failed = result.is_err()
            || host
                .tick("evil", 0, &NodeState::default())
                .is_err();
        assert!(failed, "file access must not be reachable");
    }

    #[test]
    fn a_script_cannot_spawn_a_process() {
        let mut host = host();
        let _ = host.compile("evil", r#"system("rm -rf /");"#);

        assert!(
            host.tick("evil", 0, &NodeState::default()).is_err(),
            "process spawning must not be reachable"
        );
    }

    #[test]
    fn a_script_cannot_reach_the_network() {
        let mut host = host();
        let _ = host.compile("evil", r#"http_get("http://example.com");"#);

        assert!(
            host.tick("evil", 0, &NodeState::default()).is_err(),
            "network access must not be reachable"
        );
    }

    /// The decisive reason for Rhai over Lua: when the agent writes the code,
    /// an accidental infinite loop must be a caught error, not a hung engine.
    #[test]
    fn an_infinite_loop_trips_the_operation_limit() {
        let mut host = host();
        host.compile("hang", "let i = 0; while true { i += 1; }")
            .expect("it compiles — that is the point");

        let err = host
            .tick("hang", 0, &NodeState::default())
            .expect_err("must trap, not hang");

        assert_eq!(err.error, "script_op_limit");
        assert!(
            err.hint.unwrap().contains("unbounded loop"),
            "the message must say what to do"
        );
    }

    #[test]
    fn unbounded_recursion_trips_the_depth_limit() {
        let mut host = host();
        host.compile("deep", "fn f(n) { f(n + 1) } f(0);")
            .expect("it compiles");

        let err = host
            .tick("deep", 0, &NodeState::default())
            .expect_err("must trap");

        assert!(
            matches!(err.error.as_str(), "script_depth_limit" | "script_op_limit"),
            "unexpected: {err:?}"
        );
    }

    #[test]
    fn a_huge_allocation_is_refused() {
        let mut host = ScriptHost::new(Limits {
            array_size: 128,
            ..Limits::default()
        });
        host.compile("fat", "let a = []; for i in 0..100000 { a.push(i); }")
            .expect("it compiles");

        assert!(
            host.tick("fat", 0, &NodeState::default()).is_err(),
            "memory exhaustion must be refused"
        );
    }

    /// An unknown call must say the surface is small on purpose, not just
    /// "not found" — otherwise the agent retries variations of the same idea.
    #[test]
    fn an_unknown_function_explains_the_sandbox() {
        let mut host = host();
        let _ = host.compile("oops", "read_file(\"x\");");
        let err = host.tick("oops", 0, &NodeState::default()).unwrap_err();

        assert_eq!(err.error, "script_unknown_function");
        assert!(err.hint.unwrap().contains("no filesystem"));
    }

    #[test]
    fn a_syntax_error_reports_its_line() {
        let mut host = host();
        let err = host
            .compile("bad", "let x = 1;\nlet y = ;\n")
            .expect_err("syntax error");

        assert_eq!(err.error, "script_parse_error");
        assert_eq!(err.line, Some(2), "line must be reported: {err:?}");
    }

    #[test]
    fn the_watcher_reports_a_file_once_per_change() {
        let dir = std::env::temp_dir().join("loom_script_watch");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("spin.rhai");
        std::fs::write(&path, "rotation[1] = 0.0;").unwrap();

        let mut watcher = ScriptWatcher::new();
        assert_eq!(
            watcher.changed([path.as_path()]).len(),
            1,
            "first sight counts as changed"
        );
        assert!(
            watcher.changed([path.as_path()]).is_empty(),
            "unchanged file must not re-report"
        );

        // Touch with a distinctly later mtime so the test does not depend on
        // filesystem timestamp resolution.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        std::fs::write(&path, "rotation[1] = 1.0;").unwrap();
        let _ = std::fs::File::open(&path).and_then(|f| f.set_times(
            std::fs::FileTimes::new().set_modified(later),
        ));

        assert_eq!(
            watcher.changed([path.as_path()]).len(),
            1,
            "an edited file must re-report"
        );
    }
}
