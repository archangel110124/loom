# Briefing an agent on this repository

**For the coordinator, not for the agent.** Paste §1 into every brief verbatim;
fill §2 from what you already know; keep §3 in mind while writing the task.

The problem this solves, measured on 2026-08-30: eleven agents in one session,
100–380 tool calls each, 40–130 minutes each, and **45,868 words of reports** of
which about five numbers per report were ever used. Most of that was not the
work. It was re-reading context the coordinator already had, re-establishing
baselines already measured, hunting a repro the coordinator could have handed
over, and running a forty-minute gate suite between experiments.

**What is NOT to be cut is probing.** On that same day the coordinator's
diagnosis was wrong four times — the foam field, the rotating frame, "an FFT sea
costs you foam and spray", and which table `lucent` was in. Every one was caught
because an agent measured instead of trusting the brief. An agent that skips
straight to the fix ships the coordinator's wrong guess. Cut the overheads;
never cut the instrument.

---

## §1 — Paste this into every brief

    # Standing rules

    **Gates are fast. Use them scoped.**

        cargo xtask image  --only 'a,b,c'     seconds
        cargo xtask ablate                    ~6s
        cargo xtask validate --only 'a,b'     ~9s per scene
        full suite (all four)                 ~4 min

    `--only` takes a comma-separated set and matches a row's name or its scene
    path. **Run the full suite exactly once, at the end.** Running it between
    experiments is the single largest waste there is.

    **Never poll for a background job.** No `until ! pgrep …; do sleep …; done`
    — that pattern matches the waiter's own command line and deadlocks forever.
    Start it and let it finish.

    **Report: 800 words maximum, table first.** Numbers in a table, then what
    you changed, then what you are unsure about. Long reports are not read; the
    coordinator extracts about five numbers from each. If something genuinely
    needs more, put it in a code comment where the next person will find it.

    **Do not read other reports unless this brief names one.** Everything you
    need is in the FACTS block. If a fact you need is missing, measure it and
    say you had to.

    **Never `git add -A`** — stage explicit paths. **Never run `cargo xtask
    image --bless`.** **Never edit `assets/shaders/generated/*.slang`.** No new
    dependencies. Noise comes from the engine's frozen hash only. No barrier
    outside the render graph. If a CPU path duplicates something you change, it
    moves in the same commit.

    **Verify every number by running this engine.** A formula that shares a name
    with the code is a hypothesis about the code.

---

## §2 — The FACTS block: fill it, do not make them find it

Every brief carries one. Anything the agent would otherwise re-derive goes here.

    # FACTS — do not re-establish these, only verify what you change

    Repro:        <exact command, or the scene + camera edit that shows it>
    Instrument:   <the probe that answers this question — see §3>
    Baselines:    <every number the acceptance compares against, with its command>
    Ruled out:    <what has already been tested and found not to be the cause>
    Touches:      <the files, with line numbers where known>

**The repro is the highest-value line.** Agents have spent 20–40 tool calls
hunting a camera the coordinator already had. When it is not known, say so and
give the search shape — the scene, roughly where, what the artifact looks like.

**"Ruled out" prevents the most expensive kind of repeat**: an agent
re-testing a hypothesis a previous one already killed.

---

## §3 — Instruments that work here

Name the right one in the brief; do not make the agent invent it.

| question | instrument |
|---|---|
| is this effect drawing at all | `LOOM_ABLATE=<effect>` on the same frame |
| which foam term is responsible | `float4(instant, field, in.foamHist, 1)` at `scene.slang`'s foam composite |
| is it the lace or the coverage | pin `fnoise` to 0.5; pin `wetc` to 0.5 |
| is it the rotating frame | pin the frame to `float2(1,0)` |
| stipple / sparkle | `loom salt`, worst channel |
| is foam legible | `.superpowers/sdd/foam-density/foam_legibility.py` (Weber contrast against local surround) |
| twinkle at rest | `cargo xtask shimmer` — too coarse for foam, right for geometry |
| does it repeat / pop / swim | `--frames N --spin 0 --step 3`; **a still cannot show a motion artifact** |
| is the surface breaking | fold on the 401×401 grid, not a handful of points |

**A twelve-point sample of a random field is not a measurement.** That mistake
produced a retracted finding on 2026-08-30.

---

## §4 — Scope

**One defect per agent.** The 12-hour run of that session was "fix the lobe,
then build a scene" — two jobs, the second idle until the first finished.

**Ask for one thing.** Sweeps, alternates and extra probes are worth asking for
when the answer is a judgement the human will make by eye; they are pure cost
when the answer is a number.

**Say what a negative result looks like**, and that it is acceptable. "The
texture scored worse and I kept the ladder" is a real result and saves the next
agent from rebuilding it.
