// A script watching the water. The W7 exit criterion's "a script reads it".
//
// It never looks at the crate's height, which is the point: "is it in the
// water" is a question about the water, and the surface the crate is floating
// on is a moving one. The engine answers it once, out of the same submerged
// fraction the buoyancy solver multiplied by to get its force, and hands both
// the raw fraction and the debounced state over here.
//
// **Branch on `submerged`, not on `submersion`.** The fraction is the raw
// signal and it crosses any threshold you pick twice per wave; the bool is the
// same signal through two thresholds and it holds still. Thresholding the
// fraction here would put the chatter back, one layer up, where nothing tests
// for it.

let went_in = if "went_in" in state { state.went_in } else { 0 };
let came_up = if "came_up" in state { state.came_up } else { 0 };

// The edges, off the event log. Named, because a scene may float more than one
// thing and a rule that counted every splash would win on the wrong crate.
for e in events {
    if e.node == "Sea/Crate" {
        if e.kind == "submerged" { went_in += 1; }
        if e.kind == "surfaced" { came_up += 1; }
    }
}
state.went_in = went_in;
state.came_up = came_up;

// How wet it is once settled, at its wettest and its driest — reported so
// `loom sim` prints the band the thresholds have to clear rather than leaving
// it to be guessed at. From tick 600, which is well past the entry.
if tick > 600 && "Sea/Crate" in submersion {
    let wet = submersion["Sea/Crate"];
    state.wet = wet;
    state.wet_max = if "wet_max" in state { max(state.wet_max, wet) } else { wet };
    state.wet_min = if "wet_min" in state { min(state.wet_min, wet) } else { wet };
}

// In, and back out, and *still* out much later. The last two clauses are what
// make this a test of the state rather than of the counters: a crate whose
// state chattered would satisfy the first two on its way past, and ending the
// game the moment it did would stop the run before the flicker had a chance to
// show — which is the whole thing being measured.
if went_in >= 1 && came_up >= 1 && tick > 1500 && !submerged["Sea/Crate"] {
    status = "won";
    message = "the crate went under, came back up, and stayed up";
}
