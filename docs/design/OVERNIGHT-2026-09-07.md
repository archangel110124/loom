# Overnight, 2026-09-07 — what happened and what needs you

## The nine subsystems are done

| # | Subsystem | Commit | ADR |
|---|---|---|---|
| 1 | Joints | `6e7766a` | 0083 |
| 2 | Visibility culling | `05ce38c` | 0084 |
| 3 | Lamp shadows | `484c74c` | 0085 |
| 4 | Input / gamepad / rebinding | `f7575ff` | 0086 |
| 5 | Menu navigation | `5757a7b` | 0087 |
| 6 | Save games | `4b00042` | 0088 |
| 7 | Packaging | `9a43631` | 0089 |
| 8 | Audio | `c5de5bd` | 0090 |
| 9 | Networking | `cb90716` | 0091 |
| 9b | Character throughput | `f0af17d` | 0092 |

Then the editor: `877356c`, ADR 0093, plus a follow-up commit.

## Decisions I made without you

Each of these was a fork where I picked the safer branch and wrote down why.
None is hard to reverse.

1. **Analytic head model, not a measured HRTF** (ADR 0090 §4). A dataset means
   shipping and licensing someone else's measurements. The analytic version
   carries interaural delay and head shadow, which do most of the work over
   headphones. It has **no elevation cue and front-back is symmetric** — that is
   what interaural delay *is*, and it is exactly what an HRTF would add. The
   delay-and-shadow stage is where impulse responses would go.

2. **Skinning declined, on measurement** (ADR 0092). The plan attached to
   "character throughput" was skinning plus per-frame BLAS refit, which
   overturns ADR 0072 and touches the asset loader, scene schema, vertex shader
   and acceleration structure. So I measured first: **0.19 ms of CPU per
   character per frame**, linear to 32. A 16.67 ms frame holds ~85 characters
   alone, ~30 alongside everything else. The puppet is not the bottleneck.

3. **TCP, not UDP, for networking** (ADR 0091 §3). Lockstep cannot proceed until
   every input for a tick arrives, so reliable ordered delivery is the guarantee
   it needs rather than a cost it pays. This also means zero dependencies.

4. **Desync is reported, never repaired** (ADR 0091 §5). In a deterministic
   engine a desync is an engine bug, not a network condition. Silently
   resynchronising would hide the exact class of defect the gate budget exists
   to surface.

5. **A reader hook rather than a dependency edge** (ADR 0089 §5). `loom_scene`
   resolves prefabs and so opens files, but may depend on `loom_reflect` and
   nothing else. Rather than relax that rule or thread a source through every
   prefab signature, it exposes `set_reader` and whoever mounts a pack fills it
   in. `check-deps.sh` still passes.

## What needs your attention

**Nothing is blocked on you.** These are decisions, not blockers.

1. **Windows cross-build.** You said "A then C" and I did not take the Windows
   branch. The dist path exists now, so this is a well-defined next piece — but
   a Vulkan cross-build is real work and I did not want to assume your friends
   are the target.

2. **Reachability walk for the asset pack.** Every asset ships, including test
   scenes DEEPER never opens — 240 MB where a walk from the game's scene would
   be far smaller *and* would catch a scene referencing a file that is not
   there. Noted in ADR 0089 §7 rather than built speculatively.

3. **A prefab is not relocatable** (ADR 0092 §4). Scripts declared inside a
   prefab resolve against the *instantiating scene's* directory, not the
   prefab's own. Every shipped scene sits one level below `assets/`, so nothing
   here trips over it. It will bite the first prefab library kept elsewhere.
   I did not fix it because the fix changes path resolution semantics, which is
   the kind of thing you should see before it lands.

4. **The worst frame grows with crowd size** — 29.8 ms at one deckhand, 78.7 ms
   at thirty-two, while the mean stays flat. A load-time spike, almost certainly
   the first acceleration-structure build. Nothing measures it.

5. **The image gate flakes about 1 in 3 runs**, on `river`, with an empty
   reason. It passed on every run tonight after the first. Pre-existing.

## Editor: what is still missing

`docs/design/EDITOR-SCOPE.md` has the full ranked list. Top of it:

- **Viewport view modes** — wireframe, unlit, normals, overdraw. I deliberately
  did not start this: it needs shader work plus the four-site ablation registry,
  and I did not want to land render-path changes unreviewed overnight.
- Material graph, terrain sculpting, particle authoring, multi-scene editing.
