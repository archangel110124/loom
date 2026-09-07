# ADR 0091 — The simulation is already the network protocol

- **Date:** 2026-09-07
- **Status:** **accepted** — ninth of the nine subsystems scoped as "what is
  missing for a full-blown game engine", at the general-engine bar.
- **Depends on:** ADR 0045 (the determinism line), ADR 0088 (the save format).
- **Adds:** `loom_net`, a crate with **no dependencies**.
- **Human decision this records:** co-op, hybrid lockstep with the host owning
  what lockstep cannot.

## 1. Why this is small

Networking a game is normally large because the world has to be described over
a wire: entities replicated, state interpolated, authority arbitrated, updates
prioritised against a bandwidth budget.

**None of that is necessary here, because the simulation is already bit-identical
across processes.** `cargo xtask repeat` proves it on 62 scenes, three fresh
processes each. ADR 0088 proves it survives a save and a load, byte for byte,
600 ticks past the join.

So two machines that step the same scene with the same inputs *have the same
world*, and the only thing that has to cross the wire is what the players asked
for: about 45 bytes per player per tick. The world is never sent. It cannot
drift, because there is nothing to drift from.

This is not a shortcut. It is what a determinism contract is *for*, and it is
why ADR 0045 was worth the trouble it caused.

## 1b. Addendum, 2026-09-07 — the premise is per-build

**Corrected by ADR 0096.** The sentence above should read "two machines
*running the same build*". `cargo xtask repeat` proves reproduction across
processes of one binary on one machine; nothing had compared two platforms,
because until the Windows cross-build there was only one.

Measured: a Linux and a Windows build of the same commit agree for the first few
ticks and have drifted through 37 values of simulation state by tick 300, at
about 1e-8 relative — the platform math library, which is not required to round
`sin` and `cos` identically.

Same-platform lockstep is unaffected. **Cross-platform lockstep will desync**,
and the hash exchange in §5 will say so. See ADR 0096 §3 for the options.

## 2. Decision

Deterministic lockstep with an input delay of 8 ticks (133 ms at 60 Hz). Each
peer sends its intent for tick `T + 8`; a tick is simulated only when every
peer's intent for it has arrived.

**A missing input is a wait, not a skip.** Simulating a tick without a peer's
input is simulating a different game from the one they are playing, and the
whole premise collapses the moment that is allowed once.

## 3. TCP, deliberately

The usual argument for UDP in a game is that stale data should be dropped rather
than waited for. **Lockstep is the case where that argument is false.** A tick
cannot proceed until every input for it is in, so a lost input must be
retransmitted, and inputs behind it must not be applied first.

Reliable, ordered delivery is not a cost lockstep pays for nothing — it is
precisely the guarantee it needs. `std::net::TcpStream` has it, with
`set_nodelay(true)` so Nagle does not hold a tick's 45 bytes waiting for
company. That is why this crate has no dependencies.

## 4. What the host owns

Lockstep distributes everything except the things that are not a function of the
inputs. Those are the host's:

- **Joining.** A peer arriving mid-game gets the world as a snapshot — the
  ADR 0088 save format, down the socket. It restores, and from the next tick it
  is simulating like everybody else.
- **The roster**, and this is the subtle one. Membership changes take effect at
  a **named future tick**, announced by the host, not on arrival. Applied on
  arrival, two machines would start expecting a new peer's input on different
  ticks: one stalls waiting, the other runs on, and the symptom looks exactly
  like a network fault rather than the logic error it is.
- **Peer ids**, so two joiners cannot pick the same one.

## 5. Desync is reported, not survived

Every peer publishes `World::state_hash` for the ticks it has simulated, and a
mismatch raises `Event::Desync` naming the tick and both values.

There is no rollback and no resynchronisation. **A desync in a deterministic
engine is a bug in the engine, not a network condition** — the inputs were the
same, so the divergence came from something that should not have been able to
diverge. Silently repairing it would hide exactly the class of defect this
project spends its gate budget trying to surface. It is reported, loudly, with
the tick to reproduce from.

The engine already has the tooling for that report to be actionable: the tick
number plus a save is a reproduction.

## 6. What this does not do

- **No prediction, no rollback.** Input delay hides latency instead. For 2–4
  player co-op on domestic links that is the right trade; a competitive shooter
  would need rollback, and rollback needs the save format to be cheap enough to
  take every tick, which it is not.
- **No NAT traversal, no matchmaking, no relay.** Somebody port-forwards, the
  way the Reforger server on this network already does.
- **No encryption or authentication.** A peer that connects is trusted. This is
  for playing with friends, and saying so is better than implying otherwise.
- **No bandwidth adaptation.** At 45 bytes per peer per tick — 2.7 kB/s each at
  60 Hz — there is nothing to adapt.
- **Protocol version is checked at the door.** A peer speaking another version
  is refused rather than allowed to desync in the third minute.
