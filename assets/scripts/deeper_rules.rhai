// DEEPER — the rules. The fight above the line, the hub below it, one file.
//
// **It is one file because a scene has one `GameRules`** and `play.rs` refuses
// a second by name. The demo hub needs the hold, the helm caption and the
// fight; the five `fight_*.loom` benches and `fishing.loom` need only the
// fight. So the hub half is entirely inside `if hub`, which is
// `"Rig/Player" in positions`, and is unreachable from any of the six.
//
// The alternative was a second rules script carrying a copy of every constant
// below — the ones the benches pin and a demo would not — which is green on
// both sides right up until they disagree.
//
// **Three facts about testing this, before anything else.**
//
// 1. `state_hash` is physics-only (`play.rs`). The whole fight lives in
//    `state`, so **every determinism check in this project is blind to it**
//    and `loom sim --assert` is the sole detector. A fight that silently
//    changed would still pass `cargo test`, `xtask image` and `xtask repeat`.
// 2. `loom sim` never calls `set_input` — headless input is zeros forever.
//    So the fight is tested by **swapping the pilot, never the model**: the
//    reference policies below are closed loops that live in this file, where
//    fight state is visible, and a scene picks one by containing a node named
//    `Root/Pilot/<name>`. An input tape cannot react to an LCG-scheduled run and
//    rots the moment a constant moves; a policy cannot go stale.
// 3. `--assert` runs **once, at the end** of the run. Anything time-varying
//    has to accumulate into `state` first — hence `peak`, `hot`, `hits`,
//    `runs` and the `s120`/`s300`/`s600` tick probes, which are this fight's
//    golden image: a model change is a readable numeric diff.
//
// **On the GDD (§5.4).** It asks for four concurrent demands — stress
// tracking, counter-steer, rhythm windows and live drag. Built that way the
// player watches four things and reads none. Every element is kept, but as a
// *sequence* of phases with one dominant demand live at a time. The GDD's
// "research says rhythm stays fresh" is a designer's bet wearing a citation's
// coat; this slice is the test of that bet, and the gate is how it gets
// answered without a human in the loop.
//
// Taught in one sentence: **hold to reel — it drains the fish and loads the
// line; let go to bleed; when the fish runs, let go.** Learned in one fight:
// **windows only open on a line you kept calm.**

// --- the model. Every constant. Rates are per second; `dt` is fixed. --------
let REEL_STRESS   = 22.0;   // reeling loads the line
let RUN_STRESS    = 18.0;   // so does a fish that is running
let SURGE_STRESS  = 15.0;   // reeling *into* a run is 55/s: 1.27 s to snap
let SLACK_BLEED   = 30.0;   // letting go bleeds it off
// 22 against 30 fixes the reachable duty cycle at 58%. **The ratio is the
// difficulty dial, not the absolutes** — scale both and nothing changes.
let DRAIN_CALM    = 11.0;   // the single fight-length knob, near-linear
let DRAIN_RUN     = 15.0;   // reeling through a run pays for the surge
let RECOVER       =  2.0;   // slack line, and the fish gets its breath back
let SNAP          = 100.0;

let TELL_TICKS    = 24;     // **400 ms of warning: the fairness constant.**
                            // Shrinking it is the depth-zone difficulty axis.
                            // Removing it makes a run a hidden-state reaction
                            // test, the one thing the research damns.
let RUN_TICKS     = 90;
let RUN_GAP_MIN   = 100;
let RUN_GAP_SPAN  = 120;    // runs occupy ~30% of a fight

let WINDOW_BELOW  = 45.0;
let WINDOW_PERIOD = 90;
let WINDOW_TICKS  = 15;
let WINDOW_STAM   =  8.0;
let WINDOW_RELIEF = 10.0;
let WINDOW_CALM   = 50.0;   // **the load-bearing clause, added by measurement.**
                            // A window only opens on a line you kept calm, so
                            // a pilot who wins by brute grip earns none of them
                            // and the skill gradient becomes a number a machine
                            // can read.
let MISPRESS      =  6.0;   // edge-detected: see `held`
let SLACK_LIMIT   =  5.0;
let THROW_TICKS   = 300;    // 5 s of slack throws the hook. An unattended rod
                            // must lose the fish and must never snap.

let BITE_MIN      = 120;    // 2-5 s of nothing: the serenity beat
let BITE_SPAN     = 180;
let HOOK_TICKS    = 18;     // 300 ms to set the hook

// **Is there a hub around this fight?** Read here, before the fight, because
// the fight's answer to "I landed it" depends on it.
//
// `Play::run` returns the moment `status` is won or lost — the whole
// simulation stops, scripts included, and the frame freezes so a human can see
// how it ended. That is exactly right for the five benches and for
// `proving_ground.loom`, and it is wrong for a hub: catching a fish must not
// end the game, and the reset at the bottom of this file was unreachable code
// until this line existed. So in a hub the fight still finishes and still says
// so, and simply never latches a terminal status.
let hub = "Rig/Player" in positions;

// --- carried state ---------------------------------------------------------
let phase   = if "phase"    in state { state.phase    } else { 0 };
let stress  = if "stress"   in state { state.stress   } else { 0.0 };
let stam    = if "stam"     in state { state.stam     } else { 100.0 };
let seed    = if "seed"     in state { state.seed     } else { 12345 };
let at      = if "at"       in state { state.at       } else { 0 };
let next_run= if "next_run" in state { state.next_run } else { 0 };
let run_end = if "run_end"  in state { state.run_end  } else { -1 };
let policy  = if "policy"   in state { state.policy   } else { -1 };
let peak    = if "peak"     in state { state.peak     } else { 0.0 };
let hot     = if "hot"      in state { state.hot      } else { 0 };
let hits    = if "hits"     in state { state.hits     } else { 0 };
let missed  = if "missed"   in state { state.missed   } else { 0 };
let runs    = if "runs"     in state { state.runs     } else { 0 };
let slack   = if "slack"    in state { state.slack    } else { 0 };
let held    = if "held"     in state { state.held     } else { 0 };

// **32-bit LCG, never float hashing.** `frac(sin(x))` is libm-dependent and
// would make a fight replay differently on another machine — the exact
// divergence `loom_field::noise`'s frozen-ABI rule exists to prevent. The
// multiplier stays 32-bit because rhai's arithmetic is checked and a 64-bit
// constant throws on the first multiply.
fn roll(s) { (s * 1103515245 + 12345) % 2147483648 }

// The pilot. A scene names it by containing an otherwise-empty node, so the
// five test scenes differ by one word and no scene needs a second script.
// A scene with no such node reads 0 and is driven by `fishing_angler.rhai`
// events instead, which is what a human at the rod looks like.
if policy < 0 {
    policy = 0;
    // **Two prefixes, because a scene has exactly one root.** The five benches
    // are rooted at `Root`; the demo hub is rooted at `Rig`, and `extends`-ing
    // it cannot add a second root — `multiple_roots` is a load error. So a hub
    // scene names its angler `Rig/Pilot/<name>` and this looks in both places.
    // A loop rather than ten `else if`s, and the index *is* the policy number,
    // so the order here is the order the branches below are written in.
    let NAMES = ["lazy", "skilled", "sloppy", "idle", "masher"];
    for i in 0..NAMES.len() {
        let marker = "/Pilot/" + NAMES[i];
        if ("Root" + marker) in positions || ("Rig" + marker) in positions {
            policy = i + 1;
        }
    }

    // The LCG, pinned — the wind-hash precedent, in the commit the model
    // lands in. Thirty-two rolls folded to a small exact integer, because an
    // assertion reads `state` as an f32 and a raw sum of 2^31s has a
    // 4096-wide ulp there. Any change to the generator moves this number.
    let s = 12345;
    let sum = 0;
    for i in 0..32 { s = roll(s); sum += s % 1000; }
    state.lcg32 = sum;
}

// --- the fish's schedule, advanced before anyone decides anything ----------
if phase == 4 {
    if run_end >= 0 && tick >= run_end {
        run_end = -1;
        seed = roll(seed);
        next_run = tick + RUN_GAP_MIN + seed % RUN_GAP_SPAN;
    }
    if run_end < 0 && tick >= next_run {
        run_end = tick + RUN_TICKS;
        runs += 1;
        emit.push(#{ kind: "run" });
    }
}

let running = phase == 4 && run_end >= 0 && tick < run_end;
let telling = phase == 4 && run_end < 0 && tick >= next_run - TELL_TICKS;
let window  = phase == 4 && stam < WINDOW_BELOW && !running && stress < WINDOW_CALM
              && (tick - at) % WINDOW_PERIOD < WINDOW_TICKS;

// --- what the pilot asked for ---------------------------------------------
let reel = false;
let press = false;
for e in events {
    if e.kind == "angler" {
        if "reel"  in e && e.reel  > 0.5 { reel  = true; }
        if "press" in e && e.press > 0.5 { press = true; }
    }
}

// Reference players, so a headless run has someone at the rod. Each is a
// closed loop over the same state a human reads off the meter.
if policy > 0 {
    let cast = phase == 0 || phase == 3;
    if policy == 1 {                       // lazy: hold reel, read nothing
        reel = true;  press = cast;
    } else if policy == 2 {                // skilled: reads the tell
        reel = stress < (if stam < WINDOW_BELOW { WINDOW_CALM - 10.0 } else { 70.0 })
               && !running && !telling;
        press = cast || window;
    } else if policy == 3 {                // sloppy: 12 ticks late on everything
        reel = stress < 88.0
               && !(run_end >= 0 && tick >= next_run + 12 && tick < run_end + 12)
               && !(run_end < 0 && tick >= next_run - TELL_TICKS + 12);
        press = cast || (window && (tick - at) % WINDOW_PERIOD >= 12);
    } else if policy == 4 {                // idle: nobody at the rod
        reel = false; press = cast;
    } else if policy == 5 {                // masher: reel and press flat out
        reel = true; press = tick % 8 < 4;   // ~7 presses a second
    }
}

// `fire` is edge-triggered at the binding, but the model must not depend on
// that: a pilot holding the button down would otherwise mispress sixty times a
// second and snap a line in a quarter of a second.
let edge = press && held == 0;
held = if press { 1 } else { 0 };
press = edge;

if phase == 0 {
    message = "press to cast";
    if press { phase = 1; at = tick; seed = roll(seed + tick); }
} else if phase == 1 {
    let wait = BITE_MIN + seed % BITE_SPAN;
    message = "waiting";
    if tick - at >= wait { phase = 3; at = tick; emit.push(#{ kind: "bite" }); }
} else if phase == 3 {
    message = "BITE";
    if press {
        phase = 4; at = tick;
        stress = 12.0; stam = 100.0;
        seed = roll(seed);
        next_run = tick + RUN_GAP_MIN + seed % RUN_GAP_SPAN;
        run_end = -1;
        emit.push(#{ kind: "hooked" });
    } else if tick - at > HOOK_TICKS {
        phase = 0; at = tick;
        emit.push(#{ kind: "spooked" });
    }
} else if phase == 4 {
    let d = 0.0;
    if reel { d += REEL_STRESS; }
    if running { d += RUN_STRESS; }
    if reel && running { d += SURGE_STRESS; }
    if !reel { d -= SLACK_BLEED; }
    stress += d * dt;
    if stress < 0.0 { stress = 0.0; }

    if reel {
        stam -= (if running { DRAIN_RUN } else { DRAIN_CALM }) * dt;
    } else {
        stam += RECOVER * dt;
        if stam > 100.0 { stam = 100.0; }
    }

    if press {
        if window {
            stam -= WINDOW_STAM;
            stress -= WINDOW_RELIEF;
            if stress < 0.0 { stress = 0.0; }
            hits += 1;
        } else {
            stress += MISPRESS;
            missed += 1;
        }
    }

    // A line left slack loses the fish. Without this an unattended rod sits at
    // "playing" for ever and the idle run asserts nothing at all.
    if stress < SLACK_LIMIT { slack += 1; } else { slack = 0; }

    if stress > peak { peak = stress; }
    if stress > 70.0 { hot += 1; }

    // The meter. An ASCII bar built here rather than a `Hud.bar` component,
    // because a bar cannot change the tuning and so must not delay it.
    let bar = "";
    let n = (stress / 10.0).to_int();
    for i in 0..10 { bar += if i < n { "#" } else { "-" }; }
    message = "STRESS [" + bar + "] " + stress.to_int()
            + "  FISH " + stam.to_int()
            + (if running { "  <<< RUN" } else if telling { "  ~ tell" } else { "" })
            + (if window { "  o" } else { "" });

    if stress >= SNAP {
        phase = 6; if !hub { status = "lost"; } message = "the line snapped";
        state.fight_ticks = tick - at;
        emit.push(#{ kind: "snap" });
    } else if slack >= THROW_TICKS {
        phase = 6; if !hub { status = "lost"; } message = "it threw the hook";
        state.fight_ticks = tick - at;
        emit.push(#{ kind: "escaped" });
    } else if stam <= 0.0 {
        phase = 5; if !hub { status = "won"; } message = "landed";
        state.fight_ticks = tick - at;
        state.landed_tick = tick;
        emit.push(#{ kind: "landed" });
    }
}

// Tick probes. `--assert` only ever sees the last tick, so the shape of the
// fight over time has to be sampled into `state` as it happens. These three
// are the fight's golden image: retune a constant and the diff is readable.
if tick == 120 { state.s120 = stress; }
if tick == 300 { state.s300 = stress; }
if tick == 600 { state.s600 = stress; }

state.phase = phase;
state.stress = stress;
state.stam = stam;
state.seed = seed;
state.at = at;
state.next_run = next_run;
state.run_end = run_end;
state.policy = policy;
state.peak = peak;
state.hot = hot;
state.hits = hits;
state.missed = missed;
state.runs = runs;
state.slack = slack;
state.held = held;

// ===========================================================================
// THE HUB — everything above this line is the fight and is scene-agnostic.
// ===========================================================================
//
// **Why one file and not two.** A scene has one `GameRules` (`play.rs` refuses
// a second, loudly), and the demo needs the hold, the helm caption *and* the
// fight. The three candidates were: merge, copy the fight into a second rules
// script, or add a second rules slot to the engine. A copy is two hundred lines
// of tuned constants that the five `fight_*.loom` benches pin and the demo does
// not — the exact shape that reads green while the two halves drift apart. So
// this file grew a hub instead, and `deeper_demo.rhai` is a stub that says so.
//
// **The whole block is a no-op without a hub.** Every branch below is inside
// `if hub`, and `hub` is "this scene contains `Rig/Player`". The five benches
// and `fishing.loom` do not, so they read exactly the file they always read:
// same constants, same pilots, same `s120`/`s300`/`s600` probes, same pins.
//
// ---------------------------------------------------------------------------
// WHERE THE INVENTORY LIVES, AND WHY IT LIVES HERE
// ---------------------------------------------------------------------------
//
// There is no inventory component in this engine and this is the decision not
// to add one. The hold is `GameRules` state, and the three candidates were:
//
//   * **A movement script's `memory`.** The only place that sees a key press,
//     and invisible to both `loom sim --assert` and the `Hud`. An inventory
//     nothing can read is one nobody can test and the player cannot see.
//   * **The event log.** `events.pickup >= 1` is assertable and deterministic,
//     so it is a fine record of *what happened* and a poor *container*:
//     "what am I holding now" would mean folding the whole log every tick.
//     It stays as the audit trail.
//   * **`GameRules` state.** Assertable, drawable, survives the session, and
//     is already how the fight above carries a fight. Chosen.
//
// The cost, stated plainly: `state` is **not** in `state_hash`, which is
// physics-only. `cargo test` and `cargo xtask repeat` are blind to the hold
// and `loom sim --assert` is its only detector — the fight's bargain, for the
// fight's reason.
//
// **Deliberately supplies rather than unique objects**: nothing in this engine
// removes a node at runtime, so an object that vanished into your pocket would
// still be sitting on the deck. A barrel you take a handful of bait from is
// still a barrel. The fish you land never existed as a node either, so it does
// not need to disappear.

fn has(hold, name) {
    let found = 0;
    for held in hold {
        if held == name { found = 1; }
    }
    found
}

let PLAYER = "Rig/Player";
let BOAT = "Rig/Boat";
let RANGE = 1.7;   // metres, horizontal. A pace off the crate.
let SLOTS = 4;

// Node path, and the word the HUD prints. The paths are the contract with the
// scene: rename a crate and it stops being takeable, which a `--assert` catches
// on the next run.
let SUPPLY = [
    ["Rig/BaitBarrel", "bait"],
    ["Rig/LineSpool",  "line"],
    ["Rig/Lantern",    "lantern"],
    ["Rig/Thermos",    "thermos"],
];

// How long the hub leaves a finished fight on screen before the rod is yours
// again. The benches never reach this: they have no `Rig/Player`, so for them
// a landing is still the end of the run and `status` still latches.
let OVER_TICKS = 180;

if hub {
    let hold = if "hold" in state { state.hold } else { [] };

    let at = positions[PLAYER];
    for entry in SUPPLY {
        let path = entry[0];
        let name = entry[1];
        if has(hold, name) == 0 && hold.len() < SLOTS && path in positions {
            let it = positions[path];
            let dx = at[0] - it[0];
            let dz = at[2] - it[2];
            // **Horizontal only.** The crates sit on the deck and the player's
            // origin is the capsule's centre, 0.9 m up — a spherical range
            // would spend most of itself on a height difference that is the
            // same at every crate and means nothing.
            if dx * dx + dz * dz <= RANGE * RANGE {
                hold.push(name);
                emit.push(#{ kind: "pickup", item: name });
            }
        }
    }
    state.hold = hold;
    state.carried = hold.len();

    // **One number per slot, because the hold now has its own `Hud`.**
    // `hud.rs` interpolates any `state` number into a `Hud`'s text, so four
    // numbers draw four slots — and the empty ones draw as `0`, which is the
    // whole point: a player who has found one supply can see there are three
    // more somewhere. The previous round believed `Hud` was one line and made
    // the hold and the helm caption fight over it; they are two `Hud` nodes.
    state.bait = has(hold, "bait");
    state.line = has(hold, "line");
    state.lantern = has(hold, "lantern");
    state.thermos = has(hold, "thermos");

    // -----------------------------------------------------------------------
    // WHERE HE IS — read off the player script, not guessed from a box.
    // -----------------------------------------------------------------------
    //
    // Round 3 had two different facts both called `at_helm`: this file's was
    // "standing inside a rectangle", `deeper_player.rhai`'s was "the wheel is
    // answering you". They were pronounced the same and the HUD printed the
    // wrong one, so pressing SPACE changed nothing on screen in either
    // direction. Now the movement script — the only thing in the engine that
    // sees a key press — states all three facts and this file only latches
    // them.
    //
    // **On change, not every tick.** An event a tick is a log flood for a
    // caption; a boarding is two events.
    let aboard = if "aboard" in state { state.aboard } else { 0 };
    let at_helm = if "at_helm" in state { state.at_helm } else { 0 };
    let jammed = if "jammed" in state { state.jammed } else { 0 };
    for e in events {
        if e.kind == "station" {
            aboard = e.aboard;
            at_helm = e.helm;
            jammed = e.stalled;
        }
    }
    state.aboard = aboard;
    state.at_helm = at_helm;
    state.jammed = jammed;

    // -----------------------------------------------------------------------
    // **HOW FAST SHE IS GOING, because the picture will not say.**
    // -----------------------------------------------------------------------
    //
    // Measured: from the helm at the opening yaw, a frame after forty-five
    // metres of travel differs from a moored frame by *less* than two frames of
    // the same moving boat eleven seconds apart. The mat faces her beam, the
    // bow is ninety degrees to starboard and nothing fixed is ever in shot, so
    // the only signal in the frame is ripple noise. The picture fix is to berth
    // her bow-out and it moves every pinned number in `scripts/green.sh`; this
    // is the readout in the meantime, and it is also the only form of the fact
    // a `--assert` can read.
    //
    // Smoothed, because the instantaneous delta carries the chop: she pitches
    // and heaves on a light sea and the raw number flickers by a knot.
    let knots = if "knots" in state { state.knots } else { 0.0 };
    if BOAT in positions {
        let b = positions[BOAT];
        if "bx" in state {
            let dx = b[0] - state.bx;
            let dz = b[2] - state.bz;
            let now = (dx * dx + dz * dz).sqrt() / dt * 1.94384;
            knots = knots * 0.92 + now * 0.08;
        }
        state.bx = b[0];
        state.bz = b[2];
    }
    state.knots = knots;

    // -----------------------------------------------------------------------
    // A LANDED FISH GOES IN THE HOLD, and the rod comes back.
    // -----------------------------------------------------------------------
    //
    // **Counted off the phase edge, not off the `landed` event, and the first
    // draft was the event.** `ScriptHost::rules` stamps a raised event with the
    // tick it was raised on and `play.rs` pushes it *after* `events.on_tick`
    // has already been read, so a rules script can never see its own event —
    // not this tick and not the next one, because the next tick's filter is
    // `e.tick == tick`. It is visible to `loom sim --assert events.landed` and
    // to the CLI's log, and to nothing inside this file. Written down here
    // because it read as a one-tick delay and is in fact a wall.
    //
    // `over_at` is zero on every tick of a fight and becomes the tick the fight
    // ended on, so `phase == 5 && over_at == 0` is the single instant a fish
    // was landed. It cannot double-count and it needs no second flag.
    let fish = if "fish" in state { state.fish } else { 0 };

    // **A hub is not a bench.** On the benches a fight ends the run and
    // `status` is the assertion; here landing a fish and losing one are both
    // things you do again, so the terminal phases time out back to phase 0.
    let over_at = if "over_at" in state { state.over_at } else { 0 };
    if phase >= 5 {
        if over_at == 0 {
            over_at = tick;
            if phase == 5 { fish += 1; }
        }
        if tick - over_at > OVER_TICKS {
            state.phase = 0;
            state.stress = 0.0;
            state.stam = 100.0;
            state.slack = 0;
            state.held = 0;
            over_at = 0;
        }
    } else {
        over_at = 0;
    }
    state.over_at = over_at;
    state.fish = fish;

    // -----------------------------------------------------------------------
    // THE ONE LINE OF TEXT, and what wins it.
    // -----------------------------------------------------------------------
    //
    // The hold has its own `Hud` now, so this line is free to be situational.
    // Priority, most urgent first: a fish on the line, a boat that has stopped
    // answering, the wheel, the rod, the walk.
    if phase >= 1 {
        // The fight owns the line: it is already in `message`.
    } else if jammed > 0 {
        // The stall limiter shut the throttle three quarters of a second ago
        // and the caption still said "W ahead". A skipper pressed on a quay
        // gets told he is pressed on a quay.
        message = "PUSHING ON SOMETHING   S to back off";
    } else if at_helm > 0 {
        message = "THE HELM   W ahead  S astern  A/D wheel  SPACE lets go   "
                + knots.to_int() + " kn   (bow is to starboard)";
    } else if aboard > 0 {
        message = "ABOARD   CLICK to cast   SHIFT to reel   stand on the mat to steer";
    } else {
        message = "WALK FORWARD TO THE BOAT";
    }
}
