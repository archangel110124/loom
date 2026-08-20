// **The blank loads up when the fish pulls.** One line of arithmetic, on all
// twenty-four joints of the chain.
//
// `fishing_rod.loom`'s header states the shape: the blank is a chain
// Butt -> S1 -> ... -> S24, every joint takes the *same* pitch, and the
// rotations compose into one curve. Fast action, 110 degrees of tip angle at
// stress 100, so each joint carries 110/24 = 4.583 degrees at full load. The
// baked rest pose is 0.292 per joint (7 degrees of tip), and this reproduces
// it **exactly** at stress 0 — which is what keeps every blessed reference PNG
// of a rod scene byte-identical, because those scenes have no fight in them.
//
// `state` is the rules script's numbers, read-only and one tick stale: node
// scripts run before the rules pass. A tick of lag on a bend is invisible and
// reordering two passes to remove it would not be.
//
// A scene with no fight has no `stress`, so this reads the rest pose and the
// rod is a rod. That is the whole reason the component can live in the shared
// prefab rather than in a second copy of it.
let stress = if "stress" in state { state.stress } else { 0.0 };
rotation = [0.292 + 0.042917 * stress, 0.0, 0.0];
