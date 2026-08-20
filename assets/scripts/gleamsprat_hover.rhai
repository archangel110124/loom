// A gleamsprat holding station: the nose trims up and down while the fringe
// does the work. `place`, in ADR 0062's division — the shape is the `Deform`
// on the same node and this never touches it.
//
// **One file for the whole school, and the phase comes out of the node.** A
// fixed offset would be choreography and five fish nodding together is the
// unison-sway failure grass already paid for once, so each animal needs its own
// phase and its own rate. There is nowhere to put them: a node script sees
// `tick`, `position`, `rotation` and `scale`, and no identity.
//
// `position[2]` is the identity. Nothing in this script writes it, so it is
// still the z the scene authored, it is different for every fish, and it is
// stable for the whole run. Seeding off it means one file serves five animals
// and a sixth needs no edit here at all.
let seed = position[2] * 7.31;

// **And `rotation[0]` is the only channel this trick works in.** The idle has
// to write an *absolute* value or it integrates and drifts, and an absolute
// write needs the authored base — which the script cannot recover for any
// channel it overwrites. Every fish in the school authors `rot_euler[0] = 0`,
// so for pitch alone the base is known without being read. `position[1]` and
// `rotation[1]` differ per fish and are therefore closed to a shared script;
// that is a real limit and the fix, if a second creature ever wants one, is a
// rest transform in `NodeState` rather than five near-identical script files.
//
// Intrinsic Y-X-Z, so `rot_euler[0]` is applied *before* the yaw: it is the
// animal's own pitch, nose up and nose down, whichever way it is facing.
//
// 0.2 to 0.4 Hz against the fringe's 3 Hz beat, deliberately unrelated to it. A
// trim that kept time with the stroke would be a recoil, and a recoil has to
// know the `Deform`'s frequency — which is authored per fish and which this
// file cannot see. Slow and independent is what station-keeping looks like
// anyway; the propelled version of this is `gleamsprat_cruise.rhai`, where the
// frequency is written down twice on purpose.
let rate = 0.30 + 0.10 * sin(seed * 3.7);
rotation[0] = 4.0 * sin(6.28318 * rate * tick.to_float() / 60.0 + seed);
