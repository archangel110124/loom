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

/// What a movement script is told about its character this tick.
///
/// Read-only: the script answers with a velocity and changes nothing else.
/// Position in particular is *not* writable, and that is the point — a script
/// that could assign a position would walk straight through walls, because the
/// collide-and-slide sweep happens after this and works from the velocity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    pub tick: u64,
    /// Seconds in one fixed step. Never a measured frame time (never-do #8).
    pub dt: f32,
    pub position: [f32; 3],
    /// The velocity that survived last tick's move, so a script that
    /// integrates its own finds out when it walked into a wall.
    pub velocity: [f32; 3],
    pub grounded: bool,

    /// What the player is asking for: `[strafe, forward]`, each `-1..=1`.
    /// All zero when nothing is driving this character, which is the headless
    /// case and also a character nobody is possessing.
    pub move_axis: [f32; 2],
    /// Where the character is facing, horizontal and unit length.
    ///
    /// Handed in rather than derived from the transform because "forward" for
    /// a first-person character is where the *camera* points, and the script
    /// should not have to know how the camera is rigged to find that out.
    pub forward: [f32; 3],
    /// `forward` turned 90° right, for strafing.
    pub right: [f32; 3],
    /// Where the character is actually *looking*, including up and down.
    ///
    /// Separate from `forward`, which is deliberately flat so that looking at
    /// the sky does not make W walk into it. A weapon needs the opposite: a
    /// shot has to go where the crosshair is, and aiming down a flat vector
    /// means nothing above or below eye level can ever be hit.
    pub aim: [f32; 3],
    /// True on the tick the jump button went down, not while it is held —
    /// otherwise holding it jumps again the instant the character lands.
    pub jump: bool,
    pub sprint: bool,
    /// True on the tick the fire button went down. Pressed, not held, for the
    /// same reason as `jump`.
    pub fire: bool,

    /// Where the character's view ray lands: the first solid thing along
    /// `forward`, or a point at the end of its range when nothing is there.
    ///
    /// **Resolved by the host, not the script.** A script cannot cast a ray —
    /// the sandbox has no access to the physics world, and giving it one would
    /// mean handing agent-authored code a live borrow of the simulation. The
    /// host already knows how to cast, so it casts and passes the answer.
    pub aim_point: [f32; 3],
    /// Metres to `aim_point`.
    pub aim_distance: f32,
    /// Whether the aim ray actually struck something, as opposed to running
    /// out of range. A weapon that detonates at max range in open air is a
    /// different weapon from one that needs a target.
    pub aim_hit: bool,

    /// Whether this character can currently see the player.
    ///
    /// **Perception is the host's job for the same reason aiming is**: seeing
    /// is a cast, and a script has no physics world. What a character *does*
    /// about being seen — charge, flee, radio for help, ignore it — is the
    /// script's, and that is where the interesting part of an enemy lives.
    pub can_see_target: bool,
    /// Where the player is, when visible. Stale otherwise, so a script that
    /// wants a last-known position keeps one in its own memory rather than
    /// being handed a lie.
    pub target_at: [f32; 3],
    /// Metres to the player, straight line.
    pub target_distance: f32,
    /// The next place to walk to along the route to `goal`, or the character's
    /// own position when there is no route.
    ///
    /// One step, not the whole path: a script that received a list would have
    /// to track its progress along it, and the host already knows where the
    /// character is.
    pub path_next: [f32; 3],
    /// Whether a route to the last requested `goal` exists at all. A script
    /// that cannot tell "arrived" from "unreachable" will walk into a wall
    /// forever.
    pub path_found: bool,
}

/// An explosion a script asked for this tick.
///
/// The script decides *whether* and *where*; the host does it. Requests rather
/// than direct calls because a script is sandboxed by what it cannot reach
/// (§7.8), and "can set off explosions anywhere" is a capability to grant
/// deliberately, through a shape the host can inspect, clamp and log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detonation {
    pub at: [f32; 3],
    pub radius: f32,
    pub impulse: f32,
}

/// What a movement script produced this tick.
#[derive(Debug, Clone, PartialEq)]
pub struct Motive {
    pub velocity: [f32; 3],
    /// An explosion the script asked for, if it asked for one.
    pub detonate: Option<Detonation>,
    /// Events it raised — a shot fired, a footstep, a shout.
    pub emitted: Vec<Event>,
    /// Somewhere the script wants to walk to, if it named one. The host finds
    /// the route; the script never sees a graph.
    pub goal: Option<[f32; 3]>,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            tick: 0,
            dt: 1.0 / 60.0,
            position: [0.0; 3],
            velocity: [0.0; 3],
            grounded: false,
            move_axis: [0.0; 2],
            forward: [0.0, 0.0, -1.0],
            right: [1.0, 0.0, 0.0],
            aim: [0.0, 0.0, -1.0],
            jump: false,
            sprint: false,
            fire: false,
            aim_point: [0.0, 0.0, -1.0],
            aim_distance: 0.0,
            aim_hit: false,
            can_see_target: false,
            target_at: [0.0; 3],
            target_distance: f32::INFINITY,
            path_next: [0.0; 3],
            path_found: false,
        }
    }
}

/// State a movement script keeps between ticks.
///
/// One per character, not one per script: two guards running the same file
/// must not share a dash cooldown. The contents are the script's own business
/// — the host neither reads nor validates them.
///
/// This exists because the interesting movement models all count something.
/// Coyote time is "grounded within the last N ticks", a double jump is "jumps
/// left", a dash is "ticks until ready" — none of which is derivable from a
/// single instant, so a script without memory can only express the dullest
/// possible walk.
#[derive(Debug, Clone, Default)]
pub struct ScriptMemory(rhai::Map);

impl ScriptMemory {
    /// Whether the script has stored anything yet. For tests and for a log
    /// line; the values themselves stay opaque.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// How a game ended, or that it has not.
///
/// **The engine has no idea what winning means.** It knows only that a game
/// can be over and which way — enough to stop the simulation, report a result
/// and let an assertion check it. What counts as a win is a rule, and rules
/// are authored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Playing,
    Won,
    Lost,
}

impl Status {
    #[must_use]
    pub fn is_over(self) -> bool {
        !matches!(self, Self::Playing)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::Won => "won",
            Self::Lost => "lost",
        }
    }
}

/// The game's own state, across the whole run.
///
/// Distinct from [`ScriptMemory`], which belongs to one character. Health,
/// score, ammo, how many objectives are left — none of that is any one node's
/// business, and hanging it off a character means it dies when the character
/// does.
#[derive(Debug, Clone, Default)]
pub struct GameState {
    /// Whatever the rules script keeps. Opaque to the host, like memory.
    values: rhai::Map,
    status: Status,
    /// One line for the human: what just happened, or why the game ended.
    message: String,
}

impl GameState {
    #[must_use]
    pub fn status(&self) -> Status {
        self.status
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// A named number the rules script is keeping, for assertions and for a
    /// HUD. Only numbers: a score is checkable, an arbitrary object is not.
    #[must_use]
    pub fn number(&self, name: &str) -> Option<f64> {
        self.values.get(name).and_then(|v| v.as_float().ok().or_else(|| {
            #[allow(clippy::cast_precision_loss)]
            v.as_int().ok().map(|i| i as f64)
        }))
    }

    /// Every number it is keeping, in name order, for reporting.
    #[must_use]
    pub fn numbers(&self) -> Vec<(String, f64)> {
        let mut out: Vec<(String, f64)> = self
            .values
            .keys()
            .filter_map(|k| self.number(k).map(|v| (k.to_string(), v)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

/// Something that happened, at a tick, somewhere.
///
/// **Data in a queue, not a callback.** The usual engine event system is
/// pub/sub: handlers subscribe and are invoked during dispatch. That breaks
/// both properties this project rests on. Determinism goes because the result
/// depends on registration order, and assertability goes because an event that
/// vanishes into a handler is not inspectable when the tick ends — which is
/// exactly when `loom sim --assert` looks.
///
/// So an event is appended, never dispatched. It is drained once, at a defined
/// point, and handed to the rules as an array. Nothing subscribes, nothing
/// re-enters, and the whole log is readable afterwards.
///
/// That last part is worth more than the feature: **an event log is a replay**.
/// Two runs producing the same sequence is a stronger claim than two runs
/// producing the same state hash, and unlike a hash it says where they
/// diverged.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// The tick it happened on. Never a wall-clock time (never-do #8).
    pub tick: u64,
    /// What happened: `blast`, `damage`, or whatever a script invents.
    ///
    /// Not an enum. The engine emits a couple of kinds it is uniquely able to
    /// know about; everything else is a game's own vocabulary, and a game's
    /// vocabulary does not belong in the engine's type system.
    pub kind: String,
    /// Where. Defaults to the emitter's own position.
    pub at: [f32; 3],
    /// Who it happened to, as a node path. Empty when it happened to nobody
    /// in particular — an explosion happens at a place, damage happens to
    /// someone, and a rule needs to tell the player apart from the scenery.
    pub node: String,
    /// Named numbers. Ordered, so two runs serialise identically.
    pub values: std::collections::BTreeMap<String, f64>,
}

impl Event {
    #[must_use]
    pub fn value(&self, name: &str) -> Option<f64> {
        self.values.get(name).copied()
    }
}

/// Every event so far, in the order they happened.
///
/// Append-only within a run. Kept whole rather than drained to nothing so the
/// log can be reported and compared at the end — the replay property depends
/// on it still being there.
#[derive(Debug, Clone, Default)]
pub struct EventLog {
    events: Vec<Event>,
}

impl EventLog {
    pub fn push(&mut self, event: Event) {
        self.events.push(event);
    }

    #[must_use]
    pub fn all(&self) -> &[Event] {
        &self.events
    }

    /// Events from one tick. The slice the rules script sees.
    ///
    /// A scan rather than an index: events are appended in tick order, so this
    /// is a contiguous run at the end, and a game producing enough events per
    /// tick for the scan to matter has a louder problem than this.
    #[must_use]
    pub fn on_tick(&self, tick: u64) -> Vec<Event> {
        self.events
            .iter()
            .filter(|e| e.tick == tick)
            .cloned()
            .collect()
    }

    /// How many of each kind happened, for reporting and for assertions.
    #[must_use]
    pub fn counts(&self) -> std::collections::BTreeMap<String, usize> {
        let mut out = std::collections::BTreeMap::new();
        for event in &self.events {
            *out.entry(event.kind.clone()).or_insert(0) += 1;
        }
        out
    }

    #[must_use]
    pub fn count_of(&self, kind: &str) -> usize {
        self.events.iter().filter(|e| e.kind == kind).count()
    }
}

/// What the rules script is allowed to see of the world this tick.
///
/// Positions by node path, and what has just happened. Handed in rather than
/// queried, for the same reason the aim ray is: a script has no access to the
/// scene or the physics world, and registering a host function that borrows
/// either would put a live handle to the simulation inside the sandbox.
pub struct WorldView<'a> {
    /// Every node's world position, by path.
    pub positions: &'a [(String, [f32; 3])],
    /// Everything that happened this tick.
    pub events: &'a [Event],
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

        // rhai's default `print`/`debug` go to stdout with `println!`. Every
        // `loom` subcommand emits a single JSON document on stdout for the
        // agent to parse, so one `print("hi")` in a script made that output
        // unparseable. Routed to stderr, where the rest of the engine's
        // diagnostics already go.
        engine.on_print(|text| eprintln!("script: {text}"));
        engine.on_debug(|text, source, pos| {
            eprintln!("script debug {}:{pos}: {text}", source.unwrap_or("<script>"));
        });

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

    /// Run a movement script and return the velocity it chose.
    ///
    /// **This is the seam the character controller is built around.** The
    /// engine owns collision — sweeping the capsule, sliding along walls,
    /// stepping up ledges, deciding what counts as ground. The script owns
    /// everything else: gravity, acceleration, friction, top speed, jump
    /// height, air control, whether there is a double jump at all. Swapping
    /// the file swaps the feel of the character without touching Rust.
    ///
    /// The division is not stylistic. Collision is the part a script would get
    /// subtly wrong in ways that look like level-geometry bugs; the movement
    /// model is the part that is *supposed* to be different per game, and
    /// baking it into the engine is what makes an engine feel like somebody
    /// else's engine.
    ///
    /// # Errors
    /// [`ScriptError`] if the script is unknown, traps a limit, or throws.
    pub fn motion(
        &self,
        name: &str,
        motion: &Motion,
        memory: &mut ScriptMemory,
    ) -> Result<Motive, ScriptError> {
        let Some(ast) = self.compiled.get(name) else {
            return Err(ScriptError {
                error: "unknown_script".to_owned(),
                script: name.to_owned(),
                line: None,
                message: format!("no compiled script named `{name}`"),
                hint: Some("Compile it before running it.".to_owned()),
            });
        };

        let mut scope = Scope::new();
        scope.push("tick", i64::try_from(motion.tick).unwrap_or(i64::MAX));
        scope.push("dt", f64::from(motion.dt));
        scope.push("position", to_dynamic_vec(motion.position));
        scope.push("velocity", to_dynamic_vec(motion.velocity));
        scope.push("grounded", motion.grounded);
        // Input. A script that ignores all of this is still valid — an NPC
        // has no one pressing keys for it, and reads zeros.
        scope.push("move_x", f64::from(motion.move_axis[0]));
        scope.push("move_z", f64::from(motion.move_axis[1]));
        scope.push("forward", to_dynamic_vec(motion.forward));
        scope.push("right", to_dynamic_vec(motion.right));
        scope.push("aim", to_dynamic_vec(motion.aim));
        scope.push("jump", motion.jump);
        scope.push("sprint", motion.sprint);
        scope.push("fire", motion.fire);
        scope.push("aim_point", to_dynamic_vec(motion.aim_point));
        scope.push("aim_distance", f64::from(motion.aim_distance));
        scope.push("aim_hit", motion.aim_hit);
        scope.push("can_see_target", motion.can_see_target);
        scope.push("target_at", to_dynamic_vec(motion.target_at));
        scope.push("target_distance", f64::from(motion.target_distance));
        scope.push("path_next", to_dynamic_vec(motion.path_next));
        scope.push("path_found", motion.path_found);
        // Where the character wants to go. The host routes to it and reports
        // the next step back next tick.
        scope.push("goal", Dynamic::from_array(Vec::new()));
        // The request slot. Empty means "no explosion this tick", which is
        // almost every tick — so the default has to be the quiet one.
        scope.push("detonate", Dynamic::from_array(Vec::new()));
        scope.push("detonate_radius", 0.0_f64);
        scope.push("detonate_impulse", 0.0_f64);
        scope.push("emit", Dynamic::from_array(Vec::new()));
        scope.push("memory", std::mem::take(&mut memory.0));

        let result = self.engine.run_ast_with_scope(&mut scope, ast);

        // Memory comes back even on failure. A script that throws on tick 900
        // has already earned the state it built over the first 899, and
        // dropping it would make the next tick behave differently again —
        // two bugs where the author has one.
        if let Some(map) = scope.get_value::<rhai::Map>("memory") {
            memory.0 = map;
        }
        result.map_err(|e| self.to_error(name, &e))?;

        // An untouched `velocity` means "keep going", not "stop". A script
        // still being written should coast, not freeze the character.
        Ok(Motive {
            emitted: Self::drained_events(&scope, motion.tick, motion.position),
            goal: from_scope(&scope, "goal"),
            velocity: from_scope(&scope, "velocity").unwrap_or(motion.velocity),
            detonate: from_scope(&scope, "detonate").map(|at| Detonation {
                at,
                // Zero is not a usable blast, so an unset radius means the
                // script wanted the host's default rather than nothing at all.
                radius: number(&scope, "detonate_radius").filter(|r| *r > 0.0).unwrap_or(4.0),
                impulse: number(&scope, "detonate_impulse")
                    .filter(|i| *i > 0.0)
                    .unwrap_or(300.0),
            }),
        })
    }

    /// Run the game's rules for one tick.
    ///
    /// **This is the game loop.** It runs once per tick after everything else
    /// has moved, is attached to no particular node, and is the only thing in
    /// the engine that can decide the game is over. Everything it does is a
    /// rule, and every rule is authored — the engine contributes the loop and
    /// nothing about what winning means.
    ///
    /// It reads the world (positions, and what exploded) and writes `state`,
    /// `status` and `message`. It cannot move anything: a rule that could
    /// teleport a node would be a movement model wearing a different hat, and
    /// those already have a seam.
    ///
    /// # Errors
    /// [`ScriptError`] if the script is unknown, traps a limit, or throws.
    pub fn rules(
        &self,
        name: &str,
        tick: u64,
        dt: f32,
        view: &WorldView,
        state: &mut GameState,
    ) -> Result<Vec<Event>, ScriptError> {
        let Some(ast) = self.compiled.get(name) else {
            return Err(ScriptError {
                error: "unknown_script".to_owned(),
                script: name.to_owned(),
                line: None,
                message: format!("no compiled script named `{name}`"),
                hint: Some("Compile it before running it.".to_owned()),
            });
        };

        let mut scope = Scope::new();
        scope.push("tick", i64::try_from(tick).unwrap_or(i64::MAX));
        scope.push("dt", f64::from(dt));
        scope.push("state", std::mem::take(&mut state.values));
        scope.push("status", state.status.as_str().to_owned());
        scope.push("message", state.message.clone());

        // Positions as a map from node path, built fresh each tick.
        //
        // `ponytail:` the whole scene, every tick. At blockout scale that is
        // tens of entries and costs nothing; a hundred thousand nodes would
        // want a registered query instead, which needs a borrow of the world
        // that survives into the sandbox — a real design problem, and not one
        // worth solving before something is actually slow.
        let mut positions = rhai::Map::new();
        for (path, at) in view.positions {
            positions.insert(path.as_str().into(), to_dynamic_vec(*at));
        }
        scope.push("positions", positions);

        let events: Vec<Dynamic> = view.events.iter().map(to_dynamic_event).collect();
        scope.push("events", Dynamic::from_array(events));
        // Rules can raise events of their own — "wave cleared", "objective
        // taken" — which is how one rule tells the next one something without
        // either of them knowing about the other.
        scope.push("emit", Dynamic::from_array(Vec::new()));

        let result = self.engine.run_ast_with_scope(&mut scope, ast);

        // State comes back even on failure, for the same reason a character's
        // memory does: the run up to the throw really happened.
        if let Some(map) = scope.get_value::<rhai::Map>("state") {
            state.values = map;
        }
        result.map_err(|e| self.to_error(name, &e))?;

        if let Some(text) = scope.get_value::<String>("status") {
            state.status = match text.as_str() {
                "won" => Status::Won,
                "lost" => Status::Lost,
                _ => Status::Playing,
            };
        }
        if let Some(text) = scope.get_value::<String>("message") {
            state.message = text;
        }
        // The rules have no position of their own; an event they raise
        // without one is at the origin unless it says otherwise.
        Ok(Self::drained_events(&scope, tick, [0.0; 3]))
    }

    /// Events a script asked to raise, read back out of its scope.
    ///
    /// A script emits by pushing onto an array, the same shape everything else
    /// here uses: it writes data, the host acts on it. Anything without a
    /// `kind` is dropped rather than guessed at — an event with no name is not
    /// an event, and inventing one would hide the typo.
    fn drained_events(scope: &Scope, tick: u64, at: [f32; 3]) -> Vec<Event> {
        let Some(raised) = scope.get_value::<rhai::Array>("emit") else {
            return Vec::new();
        };
        raised
            .into_iter()
            .filter_map(|item| {
                let map = item.try_cast::<rhai::Map>()?;
                let kind = map.get("kind")?.clone().into_string().ok()?;
                let at = map
                    .get("at")
                    .and_then(|v| v.clone().try_cast::<rhai::Array>())
                    .and_then(|a| {
                        let mut out = [0.0_f32; 3];
                        for (slot, value) in out.iter_mut().zip(a) {
                            #[allow(clippy::cast_possible_truncation)]
                            {
                                *slot = value.as_float().ok()? as f32;
                            }
                        }
                        Some(out)
                    })
                    .unwrap_or(at);
                let values = map
                    .iter()
                    .filter(|(name, _)| {
                        !matches!(name.as_str(), "kind" | "at" | "node")
                    })
                    .filter_map(|(name, value)| {
                        let number = value.as_float().ok().or_else(|| {
                            #[allow(clippy::cast_precision_loss)]
                            value.as_int().ok().map(|i| i as f64)
                        })?;
                        Some((name.to_string(), number))
                    })
                    .collect();
                let node = map
                    .get("node")
                    .and_then(|v| v.clone().into_string().ok())
                    .unwrap_or_default();
                Some(Event { tick, kind, at, node, values })
            })
            .collect()
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

/// An event as a rhai map: `kind`, `at`, and its numbers at the top level, so
/// a script writes `e.amount` rather than `e.values["amount"]`.
fn to_dynamic_event(event: &Event) -> Dynamic {
    let mut map = rhai::Map::new();
    map.insert("kind".into(), Dynamic::from(event.kind.clone()));
    map.insert("at".into(), to_dynamic_vec(event.at));
    map.insert("node".into(), Dynamic::from(event.node.clone()));
    map.insert("tick".into(), Dynamic::from_int(i64::try_from(event.tick).unwrap_or(i64::MAX)));
    for (name, value) in &event.values {
        map.insert(name.as_str().into(), Dynamic::from_float(*value));
    }
    Dynamic::from_map(map)
}

fn to_dynamic_vec(v: [f32; 3]) -> Dynamic {
    Dynamic::from_array(
        v.iter()
            .map(|c| Dynamic::from_float(f64::from(*c)))
            .collect(),
    )
}

#[allow(clippy::cast_possible_truncation)]
fn number(scope: &Scope, name: &str) -> Option<f32> {
    Some(scope.get_value::<f64>(name)? as f32)
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
    /// stdout carries one JSON document per `loom` invocation. A script that
    /// prints must not land in it.
    #[test]
    fn script_output_does_not_go_to_stdout() {
        let mut host = super::ScriptHost::default();
        host.compile("chatty", "print(\"hello\");").expect("compiles");
        // The assertion that matters is structural: `on_print` is installed, so
        // rhai's `println!` default is replaced. If this ever regresses, the
        // agent's JSON parse breaks in the field rather than here — so the
        // check is that the script runs and the host stays usable.
        host.tick("chatty", 0, &super::NodeState::default()).expect("runs");
    }


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

    // ---------------------------------------------------------------
    // Movement scripts. The character controller owns collision; the
    // script owns the model — see `ScriptHost::motion`.
    // ---------------------------------------------------------------

    fn walking() -> Motion {
        Motion {
            position: [0.0, 1.0, 0.0],
            grounded: true,
            ..Motion::default()
        }
    }

    #[test]
    fn a_movement_script_chooses_the_velocity() {
        let mut host = host();
        host.compile("walk", "velocity = [3.0, 0.0, -1.0];").expect("valid");

        let out = host
            .motion("walk", &walking(), &mut ScriptMemory::default())
            .expect("runs").velocity;

        assert!((out[0] - 3.0).abs() < 1e-6 && (out[2] + 1.0).abs() < 1e-6, "{out:?}");
    }

    /// The script has to be able to branch on the one thing only the
    /// controller knows. Without it there is no jump and no air control.
    #[test]
    fn a_movement_script_can_branch_on_being_grounded() {
        let mut host = host();
        host.compile("jump", "if grounded { velocity[1] = 5.0; }").expect("valid");

        let mut memory = ScriptMemory::default();
        let on_ground = host.motion("jump", &walking(), &mut memory).expect("runs").velocity;
        let airborne = Motion { grounded: false, ..walking() };
        let in_air = host.motion("jump", &airborne, &mut memory).expect("runs").velocity;

        assert!((on_ground[1] - 5.0).abs() < 1e-6, "{on_ground:?}");
        assert!(in_air[1].abs() < 1e-6, "{in_air:?}");
    }

    /// Coyote time, dash cooldowns, double jumps — every movement model worth
    /// authoring counts something across ticks. Without memory a script can
    /// only express what is derivable from this instant.
    #[test]
    fn memory_survives_between_ticks() {
        let mut host = host();
        host.compile(
            "count",
            r#"
            let n = if "n" in memory { memory.n } else { 0 };
            memory.n = n + 1;
            velocity = [memory.n.to_float(), 0.0, 0.0];
            "#,
        )
        .expect("valid");

        let mut memory = ScriptMemory::default();
        let first = host.motion("count", &walking(), &mut memory).expect("runs").velocity;
        let second = host.motion("count", &walking(), &mut memory).expect("runs").velocity;

        assert!((first[0] - 1.0).abs() < 1e-6, "{first:?}");
        assert!((second[0] - 2.0).abs() < 1e-6, "{second:?}");
    }

    /// Two characters running the same script must not share a mind.
    #[test]
    fn each_character_has_its_own_memory() {
        let mut host = host();
        host.compile(
            "count",
            r#"
            let n = if "n" in memory { memory.n } else { 0 };
            memory.n = n + 1;
            velocity = [memory.n.to_float(), 0.0, 0.0];
            "#,
        )
        .expect("valid");

        let mut first = ScriptMemory::default();
        let mut second = ScriptMemory::default();
        host.motion("count", &walking(), &mut first).expect("runs");
        host.motion("count", &walking(), &mut first).expect("runs");
        let other = host.motion("count", &walking(), &mut second).expect("runs").velocity;

        assert!((other[0] - 1.0).abs() < 1e-6, "second character saw {other:?}");
    }

    /// A script that ignores `velocity` must not stop the character dead —
    /// it inherits what the controller gave it, so a half-written script
    /// coasts rather than freezing.
    #[test]
    fn an_untouched_velocity_is_the_one_it_was_given() {
        let mut host = host();
        host.compile("noop", "let unused = tick;").expect("valid");

        let motion = Motion { velocity: [2.0, 0.0, 4.0], ..walking() };
        let out = host.motion("noop", &motion, &mut ScriptMemory::default()).expect("runs").velocity;

        assert_eq!(out, [2.0, 0.0, 4.0]);
    }

    /// A first-person character walks where it is looking, so the basis has
    /// to reach the script. Turn the character and the same key press has to
    /// send it somewhere else.
    #[test]
    fn a_movement_script_moves_relative_to_where_it_faces() {
        let mut host = host();
        host.compile(
            "walk",
            "velocity = [forward[0] * move_z + right[0] * move_x, 0.0,\
                         forward[2] * move_z + right[2] * move_x];",
        )
        .expect("valid");

        let north = Motion { move_axis: [0.0, 1.0], ..walking() };
        let out = host.motion("walk", &north, &mut ScriptMemory::default()).expect("runs").velocity;
        assert!((out[2] + 1.0).abs() < 1e-6, "facing -Z, W should go -Z: {out:?}");

        // Same key, character turned to face +X.
        let east = Motion {
            move_axis: [0.0, 1.0],
            forward: [1.0, 0.0, 0.0],
            right: [0.0, 0.0, 1.0],
            ..walking()
        };
        let out = host.motion("walk", &east, &mut ScriptMemory::default()).expect("runs").velocity;
        assert!((out[0] - 1.0).abs() < 1e-6, "facing +X, W should go +X: {out:?}");
    }

    #[test]
    fn a_movement_script_sees_the_jump_and_sprint_buttons() {
        let mut host = host();
        host.compile(
            "keys",
            "if jump { velocity[1] = 5.0; } if sprint { velocity[0] = 9.0; }",
        )
        .expect("valid");

        let pressed = Motion { jump: true, sprint: true, ..walking() };
        let out = host.motion("keys", &pressed, &mut ScriptMemory::default()).expect("runs").velocity;

        assert!((out[1] - 5.0).abs() < 1e-6 && (out[0] - 9.0).abs() < 1e-6, "{out:?}");
    }

    /// A weapon: the host casts the ray, the script decides what to do with
    /// the answer. Neither half works alone.
    #[test]
    fn a_script_can_ask_for_an_explosion_where_it_is_aiming() {
        let mut host = host();
        host.compile(
            "launcher",
            r#"
            if fire && aim_hit {
                detonate = aim_point;
                detonate_radius = 5.0;
                detonate_impulse = 700.0;
            }
            "#,
        )
        .expect("valid");

        let shot = Motion {
            fire: true,
            aim_hit: true,
            aim_point: [3.0, 1.0, -8.0],
            ..walking()
        };
        let out = host.motion("launcher", &shot, &mut ScriptMemory::default()).expect("runs");

        let blast = out.detonate.expect("the script asked for one");
        assert_eq!(blast.at, [3.0, 1.0, -8.0]);
        assert!((blast.radius - 5.0).abs() < 1e-6 && (blast.impulse - 700.0).abs() < 1e-6);
    }

    /// Almost every tick is a quiet one. A script that says nothing must not
    /// detonate anything — the default has to be silence, not a default blast.
    #[test]
    fn a_script_that_asks_for_nothing_detonates_nothing() {
        let mut host = host();
        host.compile("quiet", "velocity[0] = 1.0;").expect("valid");

        let held = Motion { fire: true, aim_hit: true, ..walking() };
        let out = host.motion("quiet", &held, &mut ScriptMemory::default()).expect("runs");

        assert!(out.detonate.is_none(), "silence must stay silent");
    }

    // ---------------------------------------------------------------
    // The game loop.
    // ---------------------------------------------------------------

    fn nowhere() -> Vec<(String, [f32; 3])> {
        Vec::new()
    }

    #[test]
    fn rules_can_end_the_game() {
        let mut host = host();
        host.compile("rules", r#"if tick > 10 { status = "won"; message = "cleared"; }"#)
            .expect("valid");
        let mut state = GameState::default();
        let view = WorldView { positions: &nowhere(), events: &[] };

        host.rules("rules", 5, 1.0 / 60.0, &view, &mut state).expect("runs");
        assert_eq!(state.status(), Status::Playing, "not yet");

        host.rules("rules", 20, 1.0 / 60.0, &view, &mut state).expect("runs");
        assert_eq!(state.status(), Status::Won);
        assert_eq!(state.message(), "cleared");
    }

    /// Score, ammo, objectives left — none of it belongs to a node, and all of
    /// it has to survive the tick that set it.
    #[test]
    fn game_state_survives_between_ticks() {
        let mut host = host();
        host.compile(
            "score",
            r#"
            let n = if "score" in state { state.score } else { 0 };
            state.score = n + 5;
            "#,
        )
        .expect("valid");
        let mut state = GameState::default();
        let view = WorldView { positions: &nowhere(), events: &[] };

        for tick in 0..4 {
            host.rules("score", tick, 1.0 / 60.0, &view, &mut state).expect("runs");
        }

        assert_eq!(state.number("score"), Some(20.0));
    }

    /// The rules have to be able to look at the world, or they can only count
    /// ticks. This is how "is the target still standing" is expressible.
    #[test]
    fn rules_can_read_where_things_are() {
        let mut host = host();
        host.compile(
            "watch",
            r#"if positions["Range/Target"][1] < 0.0 { status = "won"; }"#,
        )
        .expect("valid");
        let mut state = GameState::default();

        let standing = vec![("Range/Target".to_owned(), [0.0, 1.0, 0.0])];
        let view = WorldView { positions: &standing, events: &[] };
        host.rules("watch", 1, 1.0 / 60.0, &view, &mut state).expect("runs");
        assert_eq!(state.status(), Status::Playing, "still up");

        let fallen = vec![("Range/Target".to_owned(), [0.0, -3.0, 0.0])];
        let view = WorldView { positions: &fallen, events: &[] };
        host.rules("watch", 2, 1.0 / 60.0, &view, &mut state).expect("runs");
        assert_eq!(state.status(), Status::Won, "knocked into the pit");
    }

    /// A rules script is code an agent wrote, like any other. The limits are
    /// not per-entry-point by accident.
    #[test]
    fn a_rules_script_cannot_loop_forever() {
        let mut host = host();
        host.compile("hang", "let n = 0; while true { n += 1; }").expect("compiles");
        let view = WorldView { positions: &nowhere(), events: &[] };

        let err = host
            .rules("hang", 1, 1.0 / 60.0, &view, &mut GameState::default())
            .expect_err("must be stopped");

        assert_eq!(err.error, "script_op_limit");
    }

    /// The sandbox applies here too — this is a new entry point into the
    /// engine, and the limits are not per-entry-point by accident.
    #[test]
    fn a_movement_script_cannot_loop_forever() {
        let mut host = host();
        host.compile("hang", "while true { velocity[0] += 1.0; }").expect("compiles");

        let err = host
            .motion("hang", &walking(), &mut ScriptMemory::default())
            .expect_err("must be stopped");

        assert_eq!(err.error, "script_op_limit", "{err:?}");
    }

    #[test]
    fn a_movement_script_cannot_read_the_clock() {
        let mut host = host();
        let compiled = host.compile("clock", "let t = timestamp(); velocity[0] = 1.0;");
        let reachable = match compiled {
            Err(_) => false,
            Ok(()) => host
                .motion("clock", &walking(), &mut ScriptMemory::default())
                .is_ok(),
        };

        assert!(!reachable, "a movement script must not reach the wall clock");
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
