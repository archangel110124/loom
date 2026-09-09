# 0106 — A profiler that shows the frame

Status: accepted · 2026-09-08

## Context

The status bar reported `115 fps · cpu 1.6 · draw 7.1 ms · 260 nodes · 178
draws`, and on exit the runner printed `cpu 3.703 ms/frame mean, 57.410 ms worst
over 240 frames`.

Both numbers are true and neither is usable. The mean is smoothed, which is
right for something read at a glance and hides exactly what a profiler is for.
The maximum names a number without naming a frame. Nothing said *when* the 57 ms
happened, or what was running.

`Profiler` was on `Tab`'s cut list with a good reason attached — a tab whose
body is empty is worse than no tab. It has a body now.

## Decision

A **Profiler** tab holding four seconds of unsmoothed frames: a graph first,
then a table of mean and worst for frame, CPU, the simulation's share of CPU,
and GPU submit.

- **The simulation is the one span worth naming separately.** Every other cost
  in a frame is paid whether or not a game is running; that one is the game. It
  is bracketed with `Instant`, instrumentation only, outside the simulation's
  own clock — never-do #8 is about what the simulation *reads*, and nothing here
  is read by one.
- **The graph is scaled to the p99, not the worst.** Scaled to the worst, one
  83 ms stall squashed every normal frame into a sliver one pixel high: a graph
  showing the outlier and hiding the thing it was an outlier from. Frames above
  the scale get a red cap at the top rather than being clipped away, because the
  whole point of a p99 scale is that the outliers are still there.
- **The graph comes before the table.** The shape is what you look at; the
  numbers are what you check afterwards. The Agent panel taught this the hard
  way — with the controls last, a short panel simply lost them.
- **The first frame is not a frame.** Its `dt` runs from `App::new` and contains
  window creation, Vulkan init and the scene build. The first reading this panel
  ever took was **4842 ms**, which dragged the mean from 11.6 to 32.0 and made
  the maximum meaningless. Measuring it is not wrong; calling it a frame time
  is.

## Consequences

The history is a fixed 240-sample ring — a profiler must not itself be the leak,
and four seconds is enough to see a hitch while staying about *now*.

Adding a twelfth tab was supposed to be impossible: `Tab`'s doc said the list
was "fixed at eleven, once" because a new variant would invalidate every saved
layout. It does not. Layouts are persisted as titles, an unknown title is
dropped, and `from_json` already pushes any tab this build has that the file
lacks — so a layout written before this tab existed gains it on the next load.
The comment was wrong and is now corrected; the rule that survives is that a tab
needs a body.

The first thing the graph showed was a sawtooth: alternate frames longer than
their neighbours, on a scene reporting 86 fps. That is a finding this project
had no way to make before.
