// A weather ramp with nothing else on it, for `assets/test/weather_ramp.loom`.
//
// **The only thing this script does is move `dread` on the fixed tick.** That
// is deliberately the whole of it: the wind, the sea and the rain are authored
// in `Environment.stages` and blended by the engine, so what this proves is
// that a rules script writing one scalar is *sufficient* to escalate the
// weather. `deeper_rules.rhai` already writes `state.dread` from the boat's
// distance offshore; nothing here duplicates that and nothing here should grow
// toward it.
//
// **Linear in the tick, not eased.** How fast weather should arrive is a feel
// decision only a human at the helm can judge, and an easing rate here would
// make this instrument measure the rate as well as the ladder — two tools
// measured as one, which is this repository's signature mistake. Thirty
// seconds of steady rise and then held, so a measurement at a given tick reads
// a known rung and a 3600-tick drift run spends its whole second half at the
// top one.
//
// **On the fixed tick and nowhere else** — the rule `dread` has carried since
// ADR 0069, binding harder here because this `dread` derives waves and waves
// push rigid bodies (ADR 0045 clause 2). `run.rs` advances its own
// `wind_seconds` by a *frame* delta; a ramp riding that would pass the image
// gate, pass `cargo xtask repeat`, and be wrong only in the window the human
// judges water in.

let RAMP_TICKS = 1800.0;

let d = tick.to_float() / RAMP_TICKS;
state.dread = if d > 1.0 { 1.0 } else { d };
