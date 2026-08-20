// DEEPER — the fight. One rod, one fish, one file.
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
    if      "Root/Pilot/lazy"    in positions { policy = 1; }
    else if "Root/Pilot/skilled" in positions { policy = 2; }
    else if "Root/Pilot/sloppy"  in positions { policy = 3; }
    else if "Root/Pilot/idle"    in positions { policy = 4; }
    else if "Root/Pilot/masher"  in positions { policy = 5; }

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
        phase = 6; status = "lost"; message = "the line snapped";
        state.fight_ticks = tick - at;
        emit.push(#{ kind: "snap" });
    } else if slack >= THROW_TICKS {
        phase = 6; status = "lost"; message = "it threw the hook";
        state.fight_ticks = tick - at;
        emit.push(#{ kind: "escaped" });
    } else if stam <= 0.0 {
        phase = 5; status = "won"; message = "landed";
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
