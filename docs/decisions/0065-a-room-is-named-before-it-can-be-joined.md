# ADR 0065 — A room is named before it can be joined

- **Date:** 2026-08-20
- **Status:** **accepted** for the room code and the architecture; the transport
  is **decided but not built**, and M2 below is **not authorised** — it is a
  phase decision for the human, not an overnight one.
- **Sits under:** ADR 0045, whose clause 1 the room code stays outside on
  purpose (it is not a function of scene and tick and must never become one);
  ADR 0053, whose *machine-local* wording is the evidence that decides the
  lockstep question below; ADR 0060, whose boat-local `--assert` axes are the
  frame a future reconciler needs and already has.
- **Applies to:** `loom_cli::run` (`generate_code`, `normalise`,
  `room_code_panel`, `App::room_code`), `crates/loom_cli/Cargo.toml`, and every
  later networking decision.

## The request

> "Since this is gonna be a multiplayer game… can you make a workflow
> researching how to add networking so we can at very least have a room code
> style setup for our multiplayer. So the host will host, and then someone else
> will get a code from the host, and the host will see it basically when they
> hit escape. It should be at, like, the top right… And the code should be
> randomly generated every time. And it should be long enough to where there
> should never be any replicated."

## Decision

**Loom's multiplayer will be host-authoritative with input replication.**
Guests send their `Motion` — the same three-state axes, yaw, pitch and button
bits the input tape already carries — and the host runs `Runner::step`
unmodified and broadcasts the thirteen `f32` per rigid body that
`Physics::state_hash` already folds, plus a delta of `GameState.values` and the
event log. **There is one simulation**, the one `loom sim --assert` already
tests, and no second code path.

**This commit builds only the room code and its display.** It identifies a
session that nothing can yet join. That is deliberate: it is what was asked for,
it is hours rather than months, and it forces the "generated outside the tick"
boundary to be drawn on day one instead of retrofitted.

## Why not lockstep, which this engine looks built for

A deterministic fixed-step simulation is exactly what lockstep and rollback
want, and this project has one — pinned hashes, and `cargo xtask repeat`
comparing three fresh processes byte for byte.

**But that determinism is machine-local, and this project has already said so in
writing.** ADR 0053 concedes it for the cinematic water tier. The binary
concedes it too: `nm -D target/release/loom` resolves `sinf@GLIBC_2.2.5`,
`expf@GLIBC_2.27` and `acosf@GLIBC_2.43` — three glibc symbol versions of libm
in one link, which is glibc's own record that these implementations get
replaced. And `crates/loom_water/src/lib.rs:314` calls `phase.sin()` /
`phase.cos()` inside the Gerstner sum whose displacement moves the hull the
players stand on. Lockstep's failure mode across two such machines is a
permanent, silent, unattributable desync. `rapier3d`'s `enhanced-determinism`
covers rapier and covers none of Loom's own arithmetic.

So determinism is used here for the two things it is actually good at, and for
neither of the things it looks like it licenses:

- the input tape (`main.rs:3335`) is a **two-player regression harness that
  needs no socket**; and
- `state_hash` sent at 1 Hz is a **desync canary** costing eight bytes a second,
  which logs and never gates.

## Why host-authoritative works unusually well here

`loom_physics/src/lib.rs:837` gives every character body
`.solver_groups(InteractionGroups::none())`. Riders exert no force on the hull,
so the dependency graph is a tree — water → boat → riders — with **no feedback
edge**. The host can simulate the hull without knowing where any guest stands,
and there is no cross-player convergence loop. That fell out of a roll-stability
fix, not a networking one.

Two rules follow, and both are load-bearing:

1. **The hull is the host's, permanently.** No per-object authority transfer,
   ever. The helm is an input channel, routed to whoever stands at the wheel.
2. **A replicated body carries `linvel` and `angvel`, not just a pose.** The
   carry in ADR 0060 is a `velocity_at_point` query; replicate a pose alone and
   it returns zero and every remote player slides off the deck. **That is the
   first test to write, and it needs no network** — drive a hull from an
   externally written pose with velocities left at zero and assert the drift.

## The room code

**Twelve Crockford base32 symbols, `XXXX-XXXX-XXXX`, 60 bits**, from
`getrandom::u64()` masked to 60.

The requirement was "a hundred thousand people play the game and no two get the
same code". **The snapshot question is the easy one and the wrong one.** For `N`
codes live at once over an alphabet of `A` symbols and `L` characters, the
birthday bound is `P ≈ N²/(2·A^L)`:

| | `A^L` | 100,000 live at once |
| --- | --- | --- |
| `L = 10` | 1.126 × 10¹⁵ | 4.4 × 10⁻⁶ — 1 in 225,180 |
| `L = 12` | 1.153 × 10¹⁸ | 4.3 × 10⁻⁹ — 1 in 230,584,301 |

Both pass. **But 100,000 players is not 100,000 live rooms, and the number that
actually matters is churn**: a code has to be unique against the rooms live *at
the moment it is issued*, and it gets issued again every time a room ends. Over
`R` rooms ever created against `N` live, exposure is `E ≈ R·N/A^L`. Read the
human's number the safe way — 100,000 rooms live at all times, each lasting an
hour, sustained for a year, so `R = 8.76 × 10⁸`:

| | one year of churn at 100,000 live |
| --- | --- |
| `L = 10` | `E = 0.078` — **1 in 12.9. It happens, repeatedly.** |
| `L = 12` | `E = 7.6 × 10⁻⁵` — 1 in 13,161 |

That is the whole argument. On the snapshot question ten symbols looks fine by a
factor of two hundred thousand; on the question that decides whether two players
ever collide, it fails. **Design for concurrent-against-lifetime, not for a
snapshot.** (A realistic figure for this game — ten million rooms ever, a
thousand live at a time — is 1 in 115,292,150 at `L = 12` and 1 in 112,590 at
`L = 10`.)

**There is a third, strictest reading, and it is answered by scope rather than
by length.** "No two people ever get the same code" could mean *any two codes
ever issued*, years apart. That one twelve symbols does **not** buy: at the same
hypothetical 100,000-live-for-a-year volume, `P(some pair of the 8.76 × 10⁸
codes ever issued matched) = 0.28`, and over ten years it is 1.0. It is
irrelevant because a room code is a **live-room namespace** — a code is
recycled the moment its room ends, and nobody can tell that last April's
lobby had the same name. **It stops being irrelevant the day a code is
persisted**: a save name, a replay key, an analytics id, a shared screenshot
someone tries to rejoin from. If a code is ever written to disk, this bound is
the one that applies and it needs its own decision. (For the realistic figure —
ten million rooms ever — it is 4.3 × 10⁻⁵ either way.)

**Twelve, and the rest of the argument is laziness rather than paranoia.**
Length is the one
number that can never change later — it is baked into every client that will
ever parse a code, and Among Us was forced through exactly that migration. At 60
bits a code is safe as a *bearer token* (146 years to a hit at an unthrottled
10,000 guesses a second against 25,000 live rooms), which **deletes rate
limiting, a ban list and a separate join password from a transport that does not
exist yet**. Two characters removes a subsystem. Twelve also divides into three
groups of four, which ten does not, and 60 bits fits a `u64` with four to spare,
so the encoder is a shift loop with no padding case.

**It is longer than the codes people will compare it to, and that is the cost
being paid.** Among Us is six, Jackbox four. Twelve is a Wi-Fi password rather
than a PIN. Three things make it tolerable: the grouping (nobody holds twelve
symbols in their head, they hold three groups of four), `normalise` accepting
any case and any separators anywhere, and the fact that a code shared over
Discord is pasted rather than typed. **If it is ever shortened, the two things
that come back are rate limiting on join and a ban list** — that is what the
extra two characters bought, and it should be traded deliberately, before the
first client ships, or never.

**The alphabet's constraint is a voice channel, not a URL.** Crockford base32
drops `I`, `L`, `O` (`1`/`0` confusion) and `U` (accidental obscenity). On input
those four are mapped rather than refused — `I`/`L` → `1`, `O` → `0`, `U` → `V`
— which can only ever rescue a typo, since none of them can appear in a
generated code. Grouping in fours is what makes a code readable aloud. The
acoustic E-set (`B`/`D`/`E`/`G`/`P`/`T`/`V`/`Z`) is **not** solved, and the fix
for it — six PGP-style words — is far worse to display in a corner and type, for
a failure that self-corrects in two seconds when the join fails.

**Dropping `U` is not the whole obscenity story, and the residue was
measured rather than shrugged at.** Losing `I`, `L`, `O` and `U` makes most of
the English list unspellable — that is why `CODE_DENY` in `run.rs` is twelve
entries and not two hundred. What survives is what a vowel-poor alphabet can
still manage, and it is not negligible: `32⁻⁴` per aligned group over three
groups is **1 in 18,396** codes carrying one as a whole group, and 1 in 6,132
anywhere across the twelve symbols (measured 1 in 6,431 over four million
draws, against a closed form of 1 in 6,132). A code is read aloud and
screenshotted; one in six thousand is rare enough never to surface in testing
and common enough to happen to somebody, which is the worst of both. So
`generate_code` rerolls, bounded at eight attempts, checking the **ungrouped**
symbols so a word straddling a dash goes too — over-rejecting costs one more
syscall and under-rejecting costs a screenshot.

**Guessability is not a concern for a co-op fishing game** at this length; see
the bearer-token figure above. It would start to matter if a room ever held
something a stranger could ruin or steal, and the answer then is a join
confirmation on the host, not a longer code.

### Where it sits, and why

**Outside the deterministic core, and that is correct placement rather than a
compromise.** A room code names a session, not a simulation. It never enters a
`.loom` file, never enters `state_hash`, and is never readable by
`loom sim --assert` or by rhai. A scene that carried its own room code would
render differently in three fresh processes and `cargo xtask repeat` would fail
it — correctly. Entropy comes from the OS, once, at window start, on the
presentation thread — never from the tick.

Because it touches neither a scene nor the simulation, **no golden image and no
reference hash can move.** Not by luck, and by two independent arguments:
`crate::hud::pause_menu` and `room_code_panel` each have exactly one non-test
call site, inside the windowed draw closure in `run.rs`, and `cargo xtask image`
shells `loom render`, which never opens a window. And the two xtask paths that
*do* shell `loom run` (`xtask/src/main.rs:1511`, `:1574`) both pass `--edit`, so
`session.is_some()` and `App::room_code` is `None` there regardless.

**`getrandom = "=0.4.3"`, not `rand`.** The syscall is the entire requirement,
and `getrandom v0.4.3` was already in `loom_cli`'s tree via `uuid` via
`loom_asset` — so the `Cargo.lock` diff is **one line and no new `[[package]]`
entry**, and nothing new compiles.

### Where it is drawn

Its **own** `egui::Area` at `Order::Foreground`, anchored to the top right of
`available_rect_before_wrap`. Not a row inside `pause_menu`: that is a 180-pixel
column centred in the viewport, so appending to it would put the code in the
middle of the screen. Anchored off the available rect rather than
`Area::anchor`, for the reason `hud.rs` already learned — `anchor` measures the
whole window, which under `--edit` is on top of the inspector.

`App::room_code` is `None` when a `loom_scene::Session` is open (`--edit`),
because an authoring session hosts nothing, and `None` if the OS refuses
entropy — a viewer should not die over an ornament.

**Two things about that corner were wrong when this shipped, and both are worth
recording because neither was visible to the test that was written for it.**

`CODE_WIDTH` was 200 points and every code in the game wrapped onto two ragged
centred lines. Bisected through a real `egui::Context` on the widest code the
alphabet can spell (`MMMM-WWWW-QQQQ`): it wraps at ≤ 218.484 and fits at
≥ 218.485, at every viewport and every `pixels_per_point`, because egui lays out
in points. It is 240 now. **The layout test could not see it**: `Galley::text()`
returns the *source* string, so an assertion on the drawn text passes just as
happily on two fragments. The row count is the assertion, and with it the old
width reddens.

And **the top right is not vacant.** `proving_ground.loom:386` anchors its kill
counter there at `offset = [22, 16]`, and `set_pause_menu` does not clear
`playing`, so the HUD keeps painting it while the menu is up — 101 × 17 pixels
of text on text, straight across the `ROOM CODE` label, identically at every
resolution because both are fixed pixel offsets from the same corner. The panel
now **steps below** anything the HUD painted in its column: `hud::draw` was
already returning those rects for its own tests and the call site was discarding
them, so the fix costs one binding. Dodging rather than the two alternatives —
suppressing `only_in_play` rows during a pause contradicts the scrim's stated
job (it *dims* the score rather than deleting it), and "a scene must not author
`top_right`" would be the seventh unwritten scene-authoring rule this project
has accumulated, alongside "a `Grass` field owes its ground a green colour".

## The milestones, in order

| | | Cost |
| --- | --- | --- |
| **M1** room code + Escape-menu display | **this commit** | 0 new packages |
| **M2** a second player exists *locally*, driven by the input tape | not started | 0 |
| **M3** LAN socket, host-authoritative, **no prediction** | not started | see below |
| **M4** `resolve` by LAN broadcast — the code becomes functional with no server at all | not started | 0 |
| **M5** internet rendezvous, and prediction *if measured to be needed* | not started | +10 or +0 |

**M2 is the blocker and it contains no networking.** `Sim::player` is
`Option<usize>` (`play.rs:83`), `Play::player` is a second singleton (`:2606`),
and `drive_characters` clones one `Motion` into every character (`:1715`,
`:1745`, with the comment "Every character gets the same input for now" at
`:1739`). A second player is impossible today *on one machine*. M2 makes
`Runner.input` per-character and gives `--hold` a character prefix, so two
scripted players can be asserted headlessly on a heaving deck before a byte
crosses a wire. It survives every architecture on this table, including the two
rejected below.

**M3's real dependency cost is +26 packages, not +3.** `renet = "=2.0.0"` alone
pulls three (`bytes`, `log`, `octets`) and exports six types — no socket, no
handshake, no keepalive, no encryption. The connection layer is `renet_netcode`,
which is the other 22 (`chacha20poly1305` and its tree). Either take it, or hand
-write connection establishment and own it. **Say which in the commit that adds
it.** Refused outright: `matchbox_socket` (+307 packages — 83% of Loom's whole
graph, paid for a browser target the locked decisions rule out, and it still
needs a signalling server), `quinn` (+98, reliable-ordered streams are the wrong
default for 60 Hz positions), `laminar` (last shipped 2021, dead project),
`postcard` (+22 to serialize fixed-layout `f32` arrays; `to_le_bytes` is the
serializer).

**No `loom_net` crate** until there is more than one file to put in it, and
**no trait** for `publish`/`resolve` until the second implementation is actually
written (never-do #12).

## What this forecloses

- **Deterministic replay of a networked session.** A guest's world stops being a
  pure function of (scene, tick). Single-player `--assert`, `xtask repeat` and
  replay are untouched.
- **Lockstep and rollback, effectively permanently.** Rollback would also need a
  rapier snapshot; for the record, `rapier3d`'s `serde-serialize` feature adds
  **zero** new packages, so the *option* stays cheap even though the answer is
  no.
- **Competitive integrity.** A host-authoritative P2P co-op session has no
  anti-cheat and cannot get one.
- **Not** player count: host-authoritative scales 2→8 with no architectural
  change. The ceiling is the host's domestic upload, not the protocol.

## The two things most likely wrong with this

1. **Whether prediction is needed at all is unmeasured.** Nobody has checked
   whether 80–120 ms of input delay is noticeable while walking a heaving deck.
   M3 therefore ships with **no prediction**, and the measurement decides
   whether M5 gets a reconciler or a deletion. Deck-walking is the case that
   decides it, not the rod.
2. **The payoff milestone may be untestable in this house.** M3 and M4 assume a
   second machine on this LAN. Nobody has established there is one. If there is
   not, internet rendezvous (a $100 Steam AppID, or a VPS the human runs) moves
   onto the critical path months earlier than this order budgets. **Ask before
   scoping M3.**

One further note, recorded because it will be relitigated: routing
`loom_water`, `loom_field` and `play.rs` through the `libm` crate (already in
the lock at `Cargo.lock:1249`, via `glam` under `parry3d`, so zero new
packages) is a good idea
**for save files and replays that survive a distro upgrade**, and a bad reason
to reopen lockstep. It belongs in its own ADR, judged as a save-file property.
The call-site count for that job is unknown — the "~70" figure in circulation
came from a substring count and should not be quoted.
