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

// **Baited, the fish comes in 0.7-1.7 s instead**, and that is the whole
// consequence of carrying bait. It is deliberately a large multiple rather
// than a nudge: a player who cannot tell the two apart is carrying a token,
// and the round before this one shipped four of those.
//
// The unbaited pair above is untouched, because the five `fight_*` benches
// pin numbers against it and **no bench can ever be baited** — `state.bait`
// is written only inside `if hub`, and a bench has no `Rig/Player`.
let BAIT_MIN      = 42;
let BAIT_SPAN     = 60;

let HOOK_TICKS    = 36;     // **600 ms to set the hook, up from 300.** The
                            // bite has no sound and, until the rod loads, no
                            // movement either, so it was a reaction test
                            // against a word appearing. Dead code for all five
                            // reference pilots — each presses on every tick of
                            // `phase == 3`, so the window's width has never
                            // been in any pinned bench number.

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

// Read off last tick's hold, which is a tick stale and cannot matter: a
// pickup is a walk, not a frame. `state.bait` exists only in a hub, so this is
// false on every bench by construction rather than by a branch.
let baited  = (if "bait" in state { state.bait } else { 0 }) > 0;

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
    // **Styled, because it leaks.** This is the fight's own idle string and the
    // hub is supposed to overwrite it, but on the tick a finished fight resets
    // to phase 0 it reached the HUD verbatim — lowercase, unspaced, in the
    // middle of a screen of small-caps. Nothing greps it; it costs nothing to
    // make it presentable in the one frame it escapes into.
    message = "CLICK TO CAST";
    if press { phase = 1; at = tick; seed = roll(seed + tick); }
} else if phase == 1 {
    let wait = if baited { BAIT_MIN + seed % BAIT_SPAN }
               else      { BITE_MIN + seed % BITE_SPAN };
    // **The caption a click used to replace the helm with was the single
    // lowercase word "waiting".** It now says what is happening, whether the
    // hook is baited, and which key ends it.
    message = if baited { "LINE OUT, BAITED   a take is close" }
              else      { "LINE OUT   no bait, so this will be a while" };
    if tick - at >= wait { phase = 3; at = tick; emit.push(#{ kind: "bite" }); }
} else if phase == 3 {
    message = "BITE!   CLICK NOW";
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
            + (if window { "  o" } else { "" })
            // **Name the key.** These strings were inherited from
            // `fishing.loom`, a bench whose operator already knew, and for the
            // whole twenty-second fight the player was never told that SHIFT
            // reels.
            + "   SHIFT reels";

    // **The two outcomes a first-timer actually gets, and they were lowercase
    // bench strings.** Every *other* ending in this file goes through `notice`
    // and comes out in caps with a next step; these two were inherited from
    // `fishing.loom`, a bench whose reader was a `--assert` expression. A
    // player who holds SHIFT flat out snaps the line at tick 660 and is told
    // `the line snapped` — which on a HUD reads as a debug print — and a player
    // who never touches SHIFT is told `it threw the hook` at 790. Those are the
    // first and second things a stranger sees.
    //
    // **And both silently spend the bait**, which is the one item the whole
    // loop is built on: it goes at the *bite* (`spend`, above), so by the time
    // the line parts it is already gone and the hold reads 3. The caption says
    // so, because the next cast's own warning — "no bait, so this will be a
    // while" — arrives seconds after he has committed to it.
    //
    // `hub` scenes are the demo; the benches keep `status` as their assertion
    // and never draw this string, so no bench row moves.
    if stress >= SNAP {
        phase = 6; if !hub { status = "lost"; }
        message = "THE LINE SNAPPED   ease the reel next time   your bait is gone";
        state.fight_ticks = tick - at;
        emit.push(#{ kind: "snap" });
    } else if slack >= THROW_TICKS {
        phase = 6; if !hub { status = "lost"; }
        message = "IT THREW THE HOOK   keep the line loaded   your bait is gone";
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
// this file grew a hub instead. (A `deeper_demo.rhai` stub used to sit beside
// it saying so, referenced by nothing; round 5 deleted it and this line.)
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
//
// ---------------------------------------------------------------------------
// **WHAT MAKES IT AN INVENTORY AND NOT A CHECKLIST**
// ---------------------------------------------------------------------------
//
// Round 4 shipped four supplies and four slots, so `hold.len() < SLOTS` could
// never be false, nothing was ever spent, and no item changed a number. Four
// things you carry and cannot use is a collectibles list, and a fifth
// checkbox would not have converted it. Four rules do:
//
//   1. **In.** Walk within `RANGE` of a supply and it is yours. They now lie
//      along the taught walk to the boat rather than behind the spawn, so the
//      one gesture the demo teaches fills all four slots — measured, at
//      `move_x` anywhere in +/-0.15.
//   2. **Spent.** `bait` divides the wait for a take by roughly three
//      (`BAIT_MIN`, above) and **the fish eats it**: the slot empties on the
//      bite. Fishing on is then the slow version until you go back for more.
//   3. **In again, and this is what makes the capacity real.** A landed fish
//      takes a slot. Four supplies plus a fish is five things and there are
//      four slots, so **the demo reaches "full" by being played** rather than
//      by anyone contriving it — and a refusal says so on the HUD for three
//      seconds instead of failing silently.
//   4. **Out.** `Rig/Boat/FishHold` is the hatch on the bait box in the
//      cockpit. Stand by it and the catch goes below: slots free, `stowed`
//      climbs. It is a **place, not a key** — the same decision the helm mat
//      is, and for the same reason: the engine has five digital channels and
//      every one of them is already spent.
//
// **How it is read from outside the game**, which is the requirement an
// inventory nobody can assert on fails: `state.carried` is the count,
// `state.bait` / `state.line` / `state.lantern` / `state.thermos` are one per
// slot, `state.infish` is fish held, `state.stowed` is fish landed below, and
// `state.full` is 1 while all four slots are taken. `events.pickup`,
// `events.spend`, `events.stow` and `events.refused` are the audit trail.
// `scripts/green.sh` asserts eight of those.

fn has(hold, name) {
    let found = 0;
    for held in hold {
        if held == name { found = 1; }
    }
    found
}

// How many, because the hold can carry more than one fish and `has` cannot
// say so.
fn has_count(hold, name) {
    let n = 0;
    for held in hold {
        if held == name { n += 1; }
    }
    n
}

// A rebuild rather than a `retain`, because the hold is at most four entries.
// Removes **every** copy, which is what the fish box wants and what spending
// the bait does not care about.
fn without(hold, name) {
    let kept = [];
    for held in hold {
        if held != name { kept.push(held); }
    }
    kept
}

// **The nearer way out of the water, as a compass word.**
//
// Round 5's swim caption named one bearing — "swim east, to the ramp at the end
// of the rig" — and there is no bearing that is right from the whole sea. From
// where the taught gesture's port miss actually drops a player, (-9.70, -0.70,
// -26.17), east is 114 m of open water: measured, sixty simulated seconds, same
// sentence on screen the whole way.
//
// So the caption is computed. Two ramp feet, whichever is nearer, and the
// eight-point word for the direction it lies in. North is -Z and east is +X,
// which is the frame the rig's slipway was described in and the frame the
// scene's own comments use.
//
// Feet, not tops: the number that matters to a swimmer is where the ramp
// enters the water, and both of these reach y = -1.80 to be under his feet.
//
// **The boat's own ladder is the third way out and it is the one that moves.**
// The two rig ramps are fixed numbers; `Rig/Boat/SternLadder` is wherever she
// happens to be, which is the whole point of it — go over the side thirty
// metres out and the nearest way back is her transom, not a rig you cannot
// see. So each entry carries the name of what it is and this returns both
// halves of the sentence: a bearing computed from a target the swimmer cannot
// reach is the same defect round 5's fixed bearing had, one target further on.
fn way_out(me, ways) {
    let best = 1.0e9;
    let dx = 0.0;
    let dz = 0.0;
    let what = "the ramp";
    for r in ways {
        let ax = r[0] - me[0];
        let az = r[1] - me[2];
        let d = ax * ax + az * az;
        if d < best { best = d; dx = ax; dz = az; what = r[2]; }
    }
    let ns = if dz < 0.0 { "north" } else { "south" };
    let ew = if dx > 0.0 { "east" } else { "west" };
    let a = if dx < 0.0 { -dx } else { dx };
    let b = if dz < 0.0 { -dz } else { dz };
    // 2.5 is a 22-degree band each side of a cardinal. Wider and "north" is
    // pointed at a ramp you can miss; narrower and almost everything is a
    // hyphenated word.
    let word = if a > b * 2.5 { ew } else if b > a * 2.5 { ns } else { ns + "-" + ew };
    word + " to " + what
}

// **WHERE A THING IS, IN THE FRAME OF THE MAN LOOKING FOR IT.**
//
// This is the round-8 fix and it is a reference-frame bug, not a wording one.
// The caption after a landing read `the fish box is to starboard`, which is
// *true about the boat* — the box is at boat-local z = +1.42 and starboard is
// +Z — and unusable by a player, who is standing in his own frame with his back
// to it. Measured: at the landing tick he is at world (-0.746, -12.304) facing
// -Z and the box is at (-1.725, -9.036), so it is 3.4 m **behind** him. Holding
// the key the sentence implies walks him away from it until he wedges.
//
// `way_out`'s compass words below have the same shape of defect one deck down —
// there is no compass on this screen either. They are **deliberately left
// alone**: a swimmer's target is a lit rig fifteen metres off that he can see,
// the crossing takes half a minute, and a world word stays put while a
// body-frame word flips every time he turns his head. A hatch flush with a dim
// deck three metres away is the opposite case in both respects.
//
// `fx`/`fz` is his flat forward, quantised and latched off `deeper_player.rhai`
// — the only script in the engine that can see which way he is looking.
// Starboard of *him* is `(-fz, fx)`: `cross(forward, up)` with up = +Y.
//
// The distance is rounded to whole metres and printed. It is what makes this a
// homing indicator rather than a hint: a player walking the wrong way round the
// bait box watches the number fail to fall, which is the one piece of feedback
// no caption can be written to give.
fn toward(me, fx, fz, tx, tz) {
    let dx = tx - me[0];
    let dz = tz - me[2];
    let ahead = dx * fx + dz * fz;
    let right = dx * -fz + dz * fx;
    let a = if ahead < 0.0 { -ahead } else { ahead };
    let r = if right < 0.0 { -right } else { right };
    let d = (dx * dx + dz * dz).sqrt();
    // 2.0 is a 27-degree band each side of dead ahead and dead astern. Inside
    // it the sideways word is noise; outside it, naming only the fore-and-aft
    // half sends a man past his target.
    let word = if ahead > 0.0 && a > r * 2.0 {
        "straight ahead"
    } else if ahead < 0.0 && a > r * 2.0 {
        "dead behind you"
    } else if ahead >= 0.0 {
        if right > 0.0 { "ahead on your right" } else { "ahead on your left" }
    } else {
        if right > 0.0 { "behind you, on your right" } else { "behind you, on your left" }
    };
    // `+ 0.5` then truncate: rhai's `to_int` truncates, and a box 2.9 m off
    // reading `2 m` is the rounding a player notices.
    (d + 0.5).to_int() + " m " + word
}

let PLAYER = "Rig/Player";
let BOAT = "Rig/Boat";
let RANGE = 2.0;   // metres, horizontal. An arm and a lean.
let SLOTS = 4;

// The two ramp feet, at the waterline, in world XZ, and what to call them.
// `Rig/Slipway` descends east off the east end; `Rig/Ladder` descends north off
// the west end of the berth. Written here rather than read from `positions`
// because a node's position is its *centre* and what a swimmer needs is the
// wet end.
let RAMPS = [[16.40, 3.40, "the ramp"], [-9.30, -11.20, "the ramp"]];

// The third one, and it is the only one that moves. `Rig/Boat/SternLadder` is
// a 45 degree slab off her port quarter reaching y = -1.80, and its *centre*
// is close enough: it is 4.8 m long and a swimmer told "swim north-west to her
// stern ladder" is being pointed at a nineteen-metre boat he can see.
let BOAT_LADDER = "Rig/Boat/SternLadder";

// The crate on the wharf, and how close the boat has to be for the catch in her
// hold to count as landed. 14.0 m from the crate to the hull's origin: she
// measures 11.5 in her berth and 35.7 where `rig_drive.loom` leaves her, so the
// number is two and a half metres of slop on a berth rather than a fine
// judgement. Measured to `Rig/Boat` and not to her fish box because the box is
// eight metres off her centre and would make the answer depend on which way she
// happened to be pointing.
let FISH_CRATE = "Rig/FishCrate";
let ALONGSIDE = 14.0;

// Node path, and the word the HUD prints. The paths are the contract with the
// scene: rename a crate and it stops being takeable, which a `--assert` catches
// on the next run.
let SUPPLY = [
    ["Rig/BaitBarrel", "bait"],
    ["Rig/LineSpool",  "line"],
    ["Rig/Lantern",    "lantern"],
    ["Rig/Thermos",    "thermos"],
];

// The hatch on the bait box, drawn in the scene. A boat child, so its world
// position moves with her and this distance is right under way as well as
// alongside.
let FISH_HOLD = "Rig/Boat/FishHold";

// The top boarding tread, on her port quarter — the one thing on this boat a
// player ashore is ever trying to reach. A boat child, so the bearing follows
// her: drive her off and it points at where she went.
let BOARD_STEP = "Rig/Boat/StepHigh";

// How long a notice — a refusal, a stow, a landing, a fish that spat the bait —
// holds the caption line. Two seconds.
//
// **It must be shorter than `OVER_TICKS`, and it was equal to it.** A landing
// stamps `notice_at` and `over_at` on the same tick; at 180 against 180 the
// notice window closed at delta 180 and the phase reset at delta 181, so the
// caption handed back to the *finished fight* for one tick rather than to the
// situation. Off by one, in the wrong direction, and invisible because the
// notice could not be drawn at all — see the caption block for that half.
let NOTICE_TICKS = 120;

// How long the hub leaves a finished fight on screen before the rod is yours
// again. The benches never reach this: they have no `Rig/Player`, so for them
// a landing is still the end of the run and `status` still latches.
let OVER_TICKS = 180;

if hub {
    let hold = if "hold" in state { state.hold } else { [] };
    let stowed = if "stowed" in state { state.stowed } else { 0 };
    let notice_at = if "notice_at" in state { state.notice_at } else { -9999 };
    let notice = if "notice" in state { state.notice } else { 0 };
    // The word a pickup notice prints. A string in `state`, which the hold
    // array already proves round-trips; the `Hud` interpolates numbers only, so
    // this is read here and never drawn directly.
    let got = if "got" in state { state.got } else { "" };

    // `me`, not `at`: `at` is the fight's phase-change tick stamp, and the
    // hub used to shadow it with a position. Spending the bait needs both in
    // the same expression, which is how the collision was found.
    let me = positions[PLAYER];

    // **The fish eats the bait, on the tick it takes the hook.** `at == tick`
    // in phase 3 is the bite instant and nothing else is: `at` is stamped on
    // every phase change and phase 3 is only entered once per take.
    //
    // **`ate` is the memory of that, and it exists because the caption lied
    // without it.** A take that spits the hook printed `your bait is gone` at a
    // player who had had no bait to lose — seen with `state.bait` already 0 and
    // two fish spooked. `baited` cannot answer it: the bait is spent at the
    // bite, so by the time the fish spits, both cases read zero. Reset on the
    // cast, set at the spend, read by notice 5.
    let ate = if "ate" in state { state.ate } else { 0 };
    if phase == 1 && at == tick { ate = 0; }
    if phase == 3 && at == tick && has(hold, "bait") == 1 {
        hold = without(hold, "bait");
        emit.push(#{ kind: "spend", item: "bait" });
        ate = 1;
    }
    state.ate = ate;

    for entry in SUPPLY {
        let path = entry[0];
        let name = entry[1];
        if has(hold, name) == 0 && path in positions {
            let it = positions[path];
            let dx = me[0] - it[0];
            let dz = me[2] - it[2];
            // **Horizontal only.** The crates sit on the deck and the player's
            // origin is the capsule's centre, 0.9 m up — a spherical range
            // would spend most of itself on a height difference that is the
            // same at every crate and means nothing.
            if dx * dx + dz * dz <= RANGE * RANGE {
                if hold.len() < SLOTS {
                    hold.push(name);
                    emit.push(#{ kind: "pickup", item: name });
                    // **A pickup used to be silent, and it is one of the four
                    // things this demo is.** Traced: `state.carried` goes 0 to
                    // 4 between ticks 20 and 80 — one and a third seconds —
                    // with the caption reading `WALK FORWARD TO THE BOAT`
                    // throughout. The numbers row does change, but nothing on
                    // screen tells a player to look at it, so he acquires an
                    // inventory before he has understood he has one.
                    notice = 8; notice_at = tick;
                    got = name;
                } else {
                    // **A refusal that says so.** Silence here is how round 4
                    // could ship a capacity that was never once reached.
                    notice = 1; notice_at = tick;
                    emit.push(#{ kind: "refused", item: name });
                }
            }
        }
    }

    // **Out: the fish hold.** A place, not a key. Everything in the hold that
    // is a fish goes below in one step, because a player standing at the hatch
    // wants the catch stowed and not a dialogue about which one.
    if FISH_HOLD in positions {
        let it = positions[FISH_HOLD];
        let dx = me[0] - it[0];
        let dz = me[2] - it[2];
        if dx * dx + dz * dz <= RANGE * RANGE && has(hold, "fish") == 1 {
            let n = 0;
            for h in hold { if h == "fish" { n += 1; } }
            hold = without(hold, "fish");
            stowed += n;
            notice = 3; notice_at = tick;
            emit.push(#{ kind: "stow", count: n });
        }
    }

    // **ASHORE AGAIN: the crate, and the close of the whole loop.**
    //
    // Everything above this line is things you can do in one place. This is the
    // only rule in the demo that needs you to have *been* somewhere and come
    // back: the catch is in a hull, the crate is on a wharf, and the two have
    // to be in the same place at the same time.
    //
    // A place again, and the same 2.0 m. The second test is the boat's own
    // distance to the crate, which is what makes this "bring her back" and not
    // "walk over here" — swim ashore with the catch still in a hull thirty
    // metres out and it says so instead.
    let delivered = if "delivered" in state { state.delivered } else { 0 };
    if FISH_CRATE in positions && stowed > 0 {
        let c = positions[FISH_CRATE];
        let dx = me[0] - c[0];
        let dz = me[2] - c[2];
        if dx * dx + dz * dz <= RANGE * RANGE {
            let near = false;
            if BOAT in positions {
                let b = positions[BOAT];
                let bx = b[0] - c[0];
                let bz = b[2] - c[2];
                near = bx * bx + bz * bz <= ALONGSIDE * ALONGSIDE;
            }
            if near {
                delivered += stowed;
                emit.push(#{ kind: "deliver", count: stowed });
                stowed = 0;
                notice = 6; notice_at = tick;
            } else {
                notice = 7; notice_at = tick;
            }
        }
    }
    state.delivered = delivered;

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
    // **Two more, and they are the two states the caption used to lie about.**
    // A player three degrees off the boarding lane walks into her side and is
    // told to walk forward to the boat; three degrees the other way he is in
    // the sea and is told the same thing. Both are stated here by the only
    // script that can see them.
    let swimming = if "swimming" in state { state.swimming } else { 0 };
    let stuck = if "stuck" in state { state.stuck } else { 0 };
    // **And which way he is looking**, latched the same way and for the same
    // reason: `positions` cannot say, and every direction this file names is
    // wrong without it. Default is -Z, the spawn heading, so the very first
    // tick has a usable frame rather than a zero vector.
    let fx = if "fx" in state { state.fx } else { 0.0 };
    let fz = if "fz" in state { state.fz } else { -1.0 };
    // And what the throttle and the wheel are set to. Both are three-valued and
    // the drawn levers are on the flybridge where the helmsman cannot see them,
    // so the caption is the only place a press can be confirmed.
    let drive = if "drive" in state { state.drive } else { 0.0 };
    let rudder = if "rudder" in state { state.rudder } else { 0.0 };
    for e in events {
        if e.kind == "station" {
            aboard = e.aboard;
            at_helm = e.helm;
            jammed = e.stalled;
            swimming = e.swim;
            stuck = e.stuck;
        }
        if e.kind == "heading" { fx = e.fx; fz = e.fz; }
        if e.kind == "helmset" { drive = e.drive; rudder = e.rudder; }
    }
    state.aboard = aboard;
    state.at_helm = at_helm;
    state.jammed = jammed;
    state.swimming = swimming;
    state.stuck = stuck;
    state.fx = fx;
    state.fz = fz;
    state.drive = drive;
    state.rudder = rudder;

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
    // **PINNED — she is at full ahead and going nowhere.**
    // -----------------------------------------------------------------------
    //
    // There is already a stall limiter in `deeper_player.rhai` and it cannot
    // see this one. It measures `deck_speed` — the *player's* world position,
    // one tick to the next — and a hull pinned on a navigation mark does not
    // stop, it pivots on it: the helmsman stands seven metres off her centre,
    // so he keeps moving at a third of a metre a second while she makes good
    // nothing at all. Measured on the tape `0:move_z=1; 500:move_z=1,move_x=-1`
    // (board, then hard port): she runs into `Mark5` at (44, -20), and from
    // tick 1250 to 2000 she reads 0.89 down to 0.58 knots with `jammed` never
    // once latching and the caption still reading `AHEAD   wheel to port`.
    //
    // **The asymmetry that found it is the scene, not the physics.** The marks
    // are a wall nine and a half metres to port and open sea is to starboard,
    // so the same input mirrored has two outcomes — and port is the side the
    // demo teaches a player to look at. A first-timer turning toward the pretty
    // things stops dead and is told nothing.
    //
    // This is the boat's own speed, which is the fact the caption needed, and
    // it is measured *here* because `positions` is the only place it exists.
    // One knot, because she is over that within ten ticks of the throttle going
    // ahead (0.06 kn at tick 356, 3.79 at 374) and never under it again while
    // she is swimming. Ninety ticks — a second and a half — because contact
    // with a buoy jitters and a caption that flickers is worse than none.
    let PINNED_TICKS = 90;
    let pinned = if "pinned" in state { state.pinned } else { 0 };
    // Clamped one past the threshold rather than counted forever: the number is
    // a latch, and an unbounded one reads like a distance in `--assert` output.
    if at_helm > 0 && drive > 0.0 && knots < 1.0 {
        if pinned <= PINNED_TICKS { pinned += 1; }
    } else {
        pinned = 0;
    }
    state.pinned = pinned;

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
            if phase == 5 {
                fish += 1;
                // **The catch is a thing you are carrying**, which is what
                // makes four slots a number rather than a decoration: four
                // supplies and a fish is five, and the demo gets there by
                // being played.
                if hold.len() < SLOTS {
                    hold.push("fish");
                    notice = 2; notice_at = tick;
                } else {
                    notice = 4; notice_at = tick;
                    emit.push(#{ kind: "refused", item: "fish" });
                }
            }
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

    // **A fish that spat the bait used to say nothing at all.** Phase 3 timed
    // out back to 0 and the helm caption came straight back with no word about
    // what had happened. The hub cannot see its own `spooked` event — a rules
    // script never can, see the wall described above — so it watches the phase
    // instead. This is also where the counted-off-the-edge trick pays for
    // itself twice.
    let prev = if "prev_phase" in state { state.prev_phase } else { 0 };
    if prev == 3 && phase == 0 { notice = 5; notice_at = tick; }
    state.prev_phase = phase;

    state.notice = notice;
    state.notice_at = notice_at;
    let noticing = notice > 0 && tick - notice_at < NOTICE_TICKS;

    // Everything the hold knows, written once, at the end, after the fish has
    // gone into it. The first draft wrote these where the pickups happen and a
    // landed fish was invisible until the next tick.
    state.hold = hold;
    state.stowed = stowed;
    state.got = got;
    state.carried = hold.len();
    let infish = has_count(hold, "fish");
    state.infish = infish;
    state.full = if hold.len() >= SLOTS { 1 } else { 0 };

    // **One number per slot, because the hold has its own `Hud`.**
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
    // **HER OWN FRAME, SOLVED FROM TWO OF HER NODES.**
    // -----------------------------------------------------------------------
    //
    // A rules script sees world positions and nothing else — no rotations — and
    // three separate things in this demo need the player's position *on the
    // deck*: the fish he is carrying (which hung at a fixed spot on the sole
    // while he walked away from it), the sentence that gets him round the bait
    // box, and any future prop that has to be placed relative to him aboard.
    //
    // Two nodes of one rigid body are enough. `FishHold` is authored at boat
    // local (-8.30, 1.42); its world offset from `Rig/Boat` is that vector
    // turned by her yaw, and a 2-D rotation is recovered from one known pair:
    //
    //     c = (L . D) / |L|^2        s = (L.x*D.z - L.z*D.x) / |L|^2
    //
    // with the inverse being the transpose. No trig, no normalisation, no
    // ambiguity of sign — and it degrades to the identity if she is bow-on,
    // which is the case every pinned number in `scripts/green.sh` was taken in.
    //
    // **The one constant here has to match the scene file**, so it is named
    // rather than inlined, and moving `FishHold` means changing it. That is the
    // price of not having rotations in the view; it is two numbers.
    let HOLD_LOCAL_X = -8.30;
    let HOLD_LOCAL_Z = 1.42;
    let aboard_frame = false;
    let plx = 0.0;   // the player, on her deck, in her frame
    let ply = 0.0;
    let plz = 0.0;
    let bfx = 0.0;   // and which way he is facing, in her frame
    let bfz = -1.0;
    // **And which way SHE is pointing, in the world.** The rotation recovered
    // below takes a world vector into her frame; its first row is therefore her
    // own bow, in world XZ. Kept because the return leg needs to name a *key*,
    // and a key is a fact about the hull rather than about the man at the
    // wheel: at the helm he faces her beam, so "behind you" and "astern" are
    // ninety degrees apart and only one of them is a throttle setting.
    let bowx = 1.0;
    let bowz = 0.0;
    if BOAT in positions && FISH_HOLD in positions {
        let b = positions[BOAT];
        let h = positions[FISH_HOLD];
        let n = HOLD_LOCAL_X * HOLD_LOCAL_X + HOLD_LOCAL_Z * HOLD_LOCAL_Z;
        let dx = h[0] - b[0];
        let dz = h[2] - b[2];
        let c = (HOLD_LOCAL_X * dx + HOLD_LOCAL_Z * dz) / n;
        let s = (HOLD_LOCAL_X * dz - HOLD_LOCAL_Z * dx) / n;
        let wx = me[0] - b[0];
        let wz = me[2] - b[2];
        plx = c * wx + s * wz;
        plz = -s * wx + c * wz;
        ply = me[1] - b[1];
        bfx = c * fx + s * fz;
        bfz = -s * fx + c * fz;
        bowx = c;
        bowz = s;
        aboard_frame = true;
    }
    state.plx = plx;
    state.ply = ply;
    state.plz = plz;

    // **THE BAIT BOX, WHICH IS THE ONLY REASON THE WALK TO THE FISH BOX IS NOT
    // A WALK.**
    //
    // `col_engine_box` (`jib_vi_decks.loom`) spans boat local x -8.40..-7.20,
    // z +/-0.78, and stands 0.50 m proud of the sole against a `step_height` of
    // 0.25 — so it is a wall, not a kerb. Measured, the taught gesture leaves a
    // player standing at local (-7.32, -1.86) at the wheel, and the fish box is
    // at (-8.30, 1.42): the bearing is dead astern and the straight line goes
    // through the box. Ten tapes from the landing: S alone, A alone and D alone
    // all wedge for five hundred ticks; A+S and D+S both stow.
    //
    // **Moving the obstruction was the first choice and it is refused.** That
    // collider mirrors hull geometry drawn in a shared prefab, four other
    // scenes stand on it, and the player's x at the wheel is set by the lane he
    // boarded in rather than by the mat — so there is no legal stow position
    // (everything within 2.0 m of the helm rectangle is excluded, which on this
    // deck leaves only z > 1.10) that lies on a clear straight line from where
    // he stands. Treads over the box were costed too: three of them, at 0.17 m
    // each, because two would be 0.25 against a 0.25 limit.
    //
    // So the level keeps its dogleg and **the sentence stops pretending it does
    // not exist** — said *before* he walks into it rather than four seconds
    // after. `+/- 0.40` on the x band is a hair over the capsule's 0.35 radius.
    let baffled = aboard_frame && plz < -0.78 && plx > -8.80 && plx < -6.80;
    // Which hand that is. Her port side is -x; the right-hand axis of a man
    // facing (bfx, bfz) is (-bfz, bfx), so the sign of `bfz` is the whole
    // answer. Facing her bow — the heading the demo teaches, and the only one
    // it ever reaches without a mouse — port is his left.
    let round_it = if bfz < 0.0 { "LEFT" } else { "RIGHT" };

    // **Where the fish box is, from where he is standing and facing.**
    // Computed every tick rather than stamped on the landing, because it is
    // steered by: the number falls as he closes and does not as he wedges.
    // The box is a boat child, so this is right under way as well as alongside.
    let box_at = if FISH_HOLD in positions {
        let h = positions[FISH_HOLD];
        toward(me, fx, fz, h[0], h[2])
    } else {
        "aft, in the cockpit"
    };

    // **AND WHERE SHE IS, for a man ashore who has walked into something.**
    //
    // The ashore arm of `BLOCKED` was a fixed string — `back up — aboard is the
    // gap between the bollards` — and it fires on any wall in the scene from
    // any heading. Reproduced two ways in one pass: hold **S** and walk
    // backwards into the north rail, and hold **D** and strafe into the shed.
    // Both print it. Telling a man who is already walking backwards to back up
    // drives him harder into the thing that has him, and the gap he is being
    // sent to may be behind him.
    //
    // The aboard arm of the same sentence has been computed since round 9
    // (`step LEFT and go round it`); this is the asymmetry, and it is the
    // asymmetry that was the bug.
    let board_at = if BOARD_STEP in positions {
        let b = positions[BOARD_STEP];
        toward(me, fx, fz, b[0], b[2])
    } else {
        "the gap between the bollards"
    };

    // **AND WHERE HOME IS, which was the one leg of the trip with no bearing.**
    //
    // `toward` was called twice — for the fish box and for the crate once he
    // was already ashore — and the leg *between* them got
    // `"N BELOW   take her alongside and step ashore"`: no direction and no
    // distance, on the one leg where the player cannot see where he is going.
    // Worse, `at_helm` outranks `aboard` in the ladder, so for the whole of the
    // drive home the line said nothing about home at all, while the one key he
    // has been taught — W — drives him further out.
    //
    // Both get it now, and the helm gets the bare distance because its line is
    // already the longest on the screen. The number falling is the confirmation
    // that W was the wrong key.
    let crate_at = if FISH_CRATE in positions {
        let c = positions[FISH_CRATE];
        toward(me, fx, fz, c[0], c[2])
    } else {
        "west along the wharf"
    };
    let crate_m = if FISH_CRATE in positions {
        let c = positions[FISH_CRATE];
        let dx = c[0] - me[0];
        let dz = c[2] - me[2];
        ((dx * dx + dz * dz).sqrt() + 0.5).to_int()
    } else {
        0
    };

    // **HOW FAR SHE IS FROM HOME, AND WHICH KEY CLOSES IT.**
    //
    // The two facts the return leg was missing, and they are facts about the
    // *hull*: `crate_at` is in the player's frame, and at the wheel he faces
    // her beam, so "behind you, on your left" and "astern" are ninety degrees
    // apart and only one of them names a throttle setting. Measured on the leg
    // the demo teaches, the helm caption carried `CRATE 12 m` and then `17`
    // and then `26` while the player held the one key he had been taught — the
    // number counting *up* was the entire signal that W was wrong.
    //
    // `boat_d` is also the distance the crate rule itself uses (`ALONGSIDE`),
    // which is what lets the caption stop saying "take her alongside" at the
    // moment she is.
    let boat_d = 999.0;
    let boat_m = 999;
    let home_key = "S";
    if FISH_CRATE in positions && BOAT in positions {
        let c = positions[FISH_CRATE];
        let b = positions[BOAT];
        let dx = c[0] - b[0];
        let dz = c[2] - b[2];
        boat_d = (dx * dx + dz * dz).sqrt();
        boat_m = (boat_d + 0.5).to_int();
        home_key = if dx * bowx + dz * bowz > 0.0 { "W" } else { "S" };
    }

    // -----------------------------------------------------------------------
    // THE ONE LINE OF TEXT, and what wins it.
    // -----------------------------------------------------------------------
    //
    // The hold has its own `Hud` now, so this line is free to be situational.
    // Priority, most urgent first: a fish on the line, a boat that has stopped
    // answering, the wheel, the rod, the walk.
    if swimming > 0 {
        // **The state the demo had no sentence for, and then had the wrong
        // one.** Round 4 printed "walk forward to the boat" at a swimmer for
        // ever; round 5 replaced it with a fixed bearing that is right from one
        // corner of the sea and 114 m wrong from where the taught gesture's own
        // port miss lands. `way_out` computes it.
        //
        // **And it counts her stern ladder, which is the round-11 half.** Go
        // over the side at the fishing ground and both rig ramps are two
        // hundred metres away behind you; the boat you fell off is five. A
        // sentence naming the rig there is the same lie round 5 told, and it is
        // the one a player meets at the moment the demo is most fun.
        let ways = RAMPS;
        if BOAT_LADDER in positions {
            let l = positions[BOAT_LADDER];
            ways.push([l[0], l[2], "her stern ladder"]);
        }
        message = "IN THE WATER   swim " + way_out(me, ways);
    } else if noticing {
        // **Above the fight, and that is the fix rather than the ordering.**
        // This branch sat *below* `phase >= 1`, and a landing sets `phase = 5`
        // — so notices 2, 3 and 4, the three sentences that say what happened
        // to a fish you just caught, could not be drawn at all. What a player
        // saw for the three seconds after landing one was the fight's own
        // bench string: the lowercase word `landed`.
        //
        // The cost, stated: a stow or a refusal during a fight now covers the
        // stress meter for two seconds. Both are things the player did with his
        // own feet a moment earlier, so telling him the outcome is the more
        // useful of the two lines.
        message = if notice == 1 {
            "HOLD FULL 4/4   stow the catch in the yellow fish box"
        } else if notice == 2 {
            // **The one instruction the demo gives after its climax, and it
            // pointed the wrong way.** See `toward` for the measurement. It is
            // now his own frame, it carries the distance, and it names a colour
            // he can pick out of the cockpit.
            if baffled {
                "LANDED   go " + round_it + " round the bait box — the YELLOW FISH BOX is aft"
            } else {
                "LANDED   put her in the YELLOW FISH BOX — " + box_at
            }
        } else if notice == 3 {
            "STOWED   the catch is below. Take her home when you are done"
        } else if notice == 4 {
            "HOLD FULL 4/4   she went back over the side"
        } else if notice == 6 {
            "IN THE CRATE   that is a trip. Take bait and go again"
        } else if notice == 7 {
            "THE CATCH IS IN HER HOLD   bring her alongside first"
        } else if notice == 8 {
            "PICKED UP " + got.to_upper() + "   " + hold.len() + "/4 slots — the row above is what you are carrying"
        } else {
            // The same sentence as the two above it: the bait went at the
            // bite, and the barrel it came from is back on the rig. **Only if
            // there was one** — `ate` remembers, because with the hold empty
            // this printed `your bait is gone` at a man who never had any.
            if ate > 0 {
                "IT SPAT THE BAIT   your bait is gone   CLICK to cast again"
            } else {
                "IT SPAT THE HOOK   bait is on the rig   CLICK to cast again"
            }
        };
    } else if phase >= 1 {
        // The fight owns the line: it is already in `message`.
    } else if jammed > 0 || pinned > PINNED_TICKS {
        // The stall limiter shut the throttle three quarters of a second ago
        // and the caption still said "W ahead". A skipper pressed on a quay
        // gets told he is pressed on a quay.
        //
        // **Two detectors, one sentence, and the second one is why.** `jammed`
        // is the movement script's, off the helmsman's own speed, and it cannot
        // see a hull pivoting on a buoy — see `PINNED_TICKS`. `pinned` is this
        // file's, off the boat's, and it only ever fires at the wheel with the
        // throttle ahead.
        message = "PUSHING ON SOMETHING   S to back off";
    } else if at_helm > 0 {
        // **Two lines, and which one you get depends on whether you have
        // touched anything.** The old single line listed four bindings and a
        // speed — 84 characters at size 22, which is longer than the numbers
        // row above it — and never once said what the controls were *set to*.
        // A player holding A had no confirmation the wheel was over except the
        // channel marks going past, and holding W into a quay had none at all
        // until the stall limiter fired three quarters of a second later.
        //
        // Hands off, it teaches the keys. Hands on, it states the two settings
        // and drops the teaching, which is both more useful and shorter — the
        // player pressing A does not need to be told that A is the wheel.
        //
        // **And with a fish below, the distance home**, appended to both. This
        // branch outranks `aboard`, so without it the whole drive home was
        // captioned by a line that said nothing about home — see `crate_at`.
        // The distance rather than the bearing because these are already the
        // two longest strings on the screen, and because at the wheel the
        // useful question is not "which way is it" but "am I closing".
        // **And it is shown with an empty hold too, which is the way a
        // stranger is actually lost.** The one gesture this demo teaches is W,
        // and W at the wheel is out to sea: held for six thousand ticks it
        // reaches x = 282 with the rig invisible astern and nothing in any
        // direction but water. `stowed > 0` gated the only number that could
        // have said so, so the player who had not yet caught anything — which
        // is every first-timer — got nothing. Sixty metres is past the far end
        // of the mark line's useful half and well outside anything the demo
        // asks for.
        //
        // **Both halves are short on purpose.** This line is already the
        // longest thing on the screen — the hands-off form is 61 characters at
        // size 22, and measured off a window screenshot that font runs about
        // 12.9 px a character, so 61 is 790 px. `HOME 122 m — hold S` adds 22
        // and `ALONGSIDE — press SPACE` 26; `— S closes it` and `— SPACE and
        // step ashore` were both written first and both push the worst case
        // past 1,200 px, which is a caption that runs off a 1280-wide window.
        let home = if stowed > 0 && boat_d <= ALONGSIDE {
            "   ALONGSIDE — press SPACE"
        } else if stowed > 0 || boat_m > 60 {
            "   HOME " + boat_m + " m — hold " + home_key
        } else {
            ""
        };
        if drive == 0.0 && rudder == 0.0 {
            message = "THE HELM   W ahead  S astern  A/D wheel  SPACE lets go   "
                    + knots.to_int() + " kn" + home;
        } else {
            message = "THE HELM   "
                    + (if drive > 0.0 { "AHEAD" } else if drive < 0.0 { "ASTERN" } else { "stopped" })
                    + "   "
                    + (if rudder > 0.0 { "wheel to starboard" }
                       else if rudder < 0.0 { "wheel to port" }
                       else { "wheel amidships" })
                    + "   " + knots.to_int() + " kn   SPACE lets go" + home;
        }
    } else if stuck > 0 {
        // **Above `aboard`, and that is round 5's defect recurring one deck
        // higher.** "Outside the lane the HUD used to lie" was that round's
        // headline; aboard it still did, because this branch sat *below*
        // `aboard` and `stuck` could not fire there in the first place. Both
        // halves are fixed: `deeper_player.rhai` measures no-progress in the
        // deck's frame, and the two situations get different sentences because
        // they are different problems.
        message = if aboard > 0 && baffled {
            // **It knows which thing and which way now.** `go round it` named
            // neither, and the two inputs that work are the only two a wedged
            // player is not already pressing.
            "BLOCKED   the bait box — step " + round_it + " and go round it"
        } else if aboard > 0 && infish > 0 {
            // Wedged aboard, somewhere other than the bait box, with a fish in
            // his hands. `stuck` outranks `aboard`, so this used to *replace*
            // the one bearing he needed with a sentence naming nothing —
            // reproduced coming home wedged aft. The obstruction is unnamed
            // because it could be any of nine colliders on this deck; the
            // destination is not.
            "BLOCKED   go round it — the YELLOW FISH BOX is " + box_at
        } else if aboard > 0 {
            "BLOCKED   something is in the way — go round it"
        } else {
            "BLOCKED   something has you — she is " + board_at
        };
    } else if aboard > 0 {
        // **The signposts the return leg needs.** A landed fish used to vanish
        // from the text two seconds after it was caught: `notice` cleared and
        // the caption went back to `CLICK to cast` while `state.infish` read 1
        // and the fish was still hanging in front of him. Now the fish outranks
        // the invitation to catch another one, and it keeps the live bearing —
        // so a player who walked the wrong way round the bait box can watch the
        // metres fail to fall.
        message = if infish > 0 && baffled {
            "FISH IN HAND   go " + round_it + " round the bait box — the YELLOW FISH BOX is aft"
        } else if infish > 0 {
            "FISH IN HAND   the YELLOW FISH BOX is " + box_at
        } else if stowed > 0 && boat_d <= ALONGSIDE {
            // **The sentence at the moment the demo pays off.** Traced on the
            // delivery tape, at the crate's own range: `the crate is 6 m dead
            // behind you — take her alongside` at tick 2470 and the same at 2 m
            // at 2560. She *was* alongside; `ALONGSIDE` is 14.0 m. The player
            // was already walking to the crate on foot and was still being told
            // to bring the boat in, and no branch anywhere said "step ashore".
            stowed + " BELOW   SHE IS ALONGSIDE — step off, the crate is " + crate_at
        } else if stowed > 0 {
            stowed + " BELOW   the crate is " + crate_at + " — take her alongside"
        } else {
            "ABOARD   CLICK to cast   SHIFT to reel   stand on the mat to steer"
        };
    } else if stowed > 0 {
        // Ashore, with a catch still in her. The other half of the signpost —
        // and "by the west rail" was the same defect as "to starboard", so it
        // is the same fix. He has just stepped off a boat and could be facing
        // any way at all.
        message = stowed + " IN HER HOLD   the crate is " + crate_at;
    } else {
        message = "WALK FORWARD TO THE BOAT";
    }
}
