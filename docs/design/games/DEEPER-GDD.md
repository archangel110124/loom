> **STATUS: CANDIDATE, NOT COMMITTED WORK.** Saved 2026-08-18 at the human's request as a
> possible game to build on Loom. Nothing here is scheduled; nothing in the engine roadmap
> depends on it. Engine-readiness was assessed against the code the same day — the gaps are
> networking (zero dependencies), skeletal animation (`import_gltf` returns a static `Mesh`),
> and voice transport. The water, weather, scatter, buoyancy, physics, scripting, acoustics
> and game-layer scaffolding all exist.

# GAME DESIGN DOCUMENT — “DEEPER” (working title)

### A co-op horror fishing game for the Loom engine

-----

## TL;DR

- **The pitch:** A 1–4 player co-op fishing game that starts cozy and curdles into deep-water horror as you descend. Catch impossible alien fish, upgrade your rods, and haul your catch back to the surface before the dark below claims you. It fuses Dredge’s dread-escalation and cargo-Tetris with Lethal Company’s proximity-voice extraction loop — built to show off the Loom engine’s water rendering.
- **Why it works:** Research shows co-op horror’s biggest hits (R.E.P.O., PEAK, Lethal Company, Content Warning) sell on proximity voice chat, emergent comedy, and viral shareability, while fishing/horror’s gold standard (Dredge, 1M+ copies) proves the cozy-to-eldritch tonal pivot has a high commercial ceiling. Nobody has yet combined an interactive-water fishing loop with co-op extraction horror.
- **The risk to manage:** The genre is crowded and its recurring failure mode is thin late-game and quota-grind fatigue. The design must front-load progression, variety, and horror escalation, and the custom Rust/Vulkan engine must nail physics state-sync and proximity voice or the whole appeal collapses.

-----

## 1. HIGH CONCEPT / ELEVATOR PITCH

You and up to three friends are contract anglers working the waters of a dead alien world. You start on a calm morning lagoon — sun on the water, cozy music, weird but harmless little fish tugging your line. You buy better rods. You go a little deeper. The water gets darker, the fish get stranger, and something down there starts noticing you. By the time you’re fishing the abyss, the cozy game you signed up for is gone, and the only question is whether you can haul your catch back to the surface before it hauls you down.

**DEEPER** is a session-based, room-code co-op game that hybridizes three structures:

1. an **open world** of stacked depth zones (a vertical ocean),
1. a **persistent hub** (the Rig) you return to between expeditions, and
1. an **extraction layer** — everything you catch is worthless until you physically bring it back up, and the deeper you go, the harder the trip home.

**Working title options** (ranked):

1. **DEEPER** — clean, thematic, one word, storefront-friendly.
1. **THE LURE** — double meaning (fishing lure / being lured deeper).
1. **BOTTOMFEEDERS** — comedic-cozy surface, sinister undertone; leans into the “friend-slop” comedy audience.
1. **FATHOMS** — evocative, thalassophobic.
1. **CHUM** — short, ugly, memorable; “chum” as bait and as buddies.
1. **THE STILL DEEP** — atmospheric, horror-forward.
1. **DEADRIFT** / **UNDERTOW** — backups.

Recommendation: ship as **DEEPER** with **BOTTOMFEEDERS** as the fallback if the comedy-forward marketing angle tests better.

-----

## 2. DESIGN PILLARS

Every feature must serve at least one; features that serve none get cut.

**Pillar 1 — The water is the star.** This is the showcase title for Loom’s water system. Every mechanic should create a reason to look at, disturb, or fear the water: splashes when you cast, foam trails on the reel, caustics dancing on the hull, bioluminescent wakes at depth, the flat black stillness of the abyss. If a feature doesn’t touch the water, question it.

**Pillar 2 — Cozy is a setup, not a genre.** The game must be genuinely relaxing at the top so the descent genuinely hurts. The horror is earned by contrast, never by constant pressure. (Research: Dredge, Subnautica, and “dark cozy” games work because the daytime/surface loop is legitimately pleasant even when you know what waits below.)

**Pillar 3 — Fear is shared, and so is the laughter.** This is pure co-op. The design manufactures moments where players split up, lose contact, warn each other, and rescue each other. Proximity voice is not a feature; it is the core emotional engine. (Research: this is the single most reliable predictor of viral co-op success.)

**Pillar 4 — Every catch is a decision.** Fishing should never be a slot machine. Casting, hooking, fighting, and hauling each catch is a risk/reward choice about depth, time, cargo space, and how much you’re willing to provoke what’s below.

**Pillar 5 — The descent is the difficulty curve, the story, and the horror — all at once.** Depth is the single legible axis of progression. Going deeper means better fish, more money, more terror, and a harder extraction. One axis, three payoffs.

-----

## 3. TARGET AUDIENCE, COMPARABLES & MARKET POSITIONING

### 3.1 What the research says players want

**From co-op horror (Lethal Company, R.E.P.O., Content Warning, Phasmophobia, Devour):**

- Proximity voice chat is the beating heart. The signature moment across every source is “a friend’s voice cuts out mid-sentence and you don’t know if they’re hiding, lost, or dead.” (Lethal Company sources repeatedly cite this exact experience.)
- Emergent comedy-horror: the game provides tools, threats, and a stage, then steps back. Players author their own stories.
- Escalating quota/pressure that “always rises faster than you can keep up, so every run starts calm and ends in panic.” 
- Viral shareability — R.E.P.O., PEAK, and Content Warning all “leaned heavily into proximity voice chat and mechanics that generated organic, shareable moments.”

**Common failure modes to avoid (from negative reviews):**

- **Thin late-game / repetitiveness.** Content Warning’s most-cited criticism: “an idea can carry a first session on pure novelty… it cannot carry a tenth session when the underlying gameplay loop is thin.” Lack of monster/content variety kills longevity.
- **Quota-grind fatigue** — “upgrades take forever,” “gets repetitive after a while,” full-reset-on-failure loops that erase progress feel bad.
- **Weak progression** — directed/story-driven co-op experiences “cannot create endless new incidents.”
- **Bad netcode** ruining the one thing that matters (co-op presence).

**From fishing games (Dredge, Webfishing, Stardew, Fisch, Subnautica):**

- Fishing hits three emotional axes: **serenity, surprise, excitement.** A good fishing game modulates between them; a tedious one flattens to one.
- The minigame must have skill expression. Research taxonomy: reaction-test and tension-reeling mechanics create engagement but cause fatigue if they’re the *only* mechanic; rhythm input (Dredge) stays fresh; downtime between hooks needs to be filled with another mechanic.
- Dredge’s **spatial cargo-Tetris inventory** turned “the dullest-sounding Steam tag” into the core mechanic — aberrant fish have awkward shapes (1×1, 1×2, 2×2, L-shaped) that force real packing decisions, and the developers confirmed inventory management became “the core mechanic for our game.”
- Rod/gear progression in Fisch is a long ladder of Lure Speed / Luck / Control / Resilience / Max Weight stats gated by zones; the lesson is *meaningful* upgrades over incremental ones (“never buy a rod you’ll replace within 10 levels”).
- Webfishing proves the cozy-social-hangout loop has huge appeal on its own — the fishing is “secondary to just existing together,” and it built a 98%-positive following as “part game, part room, part mood.” 

**From thalassophobia / deep-water horror (Subnautica, Iron Lung, Dredge):**

- Deep water is scary because of the **unknown below**, darkness, low visibility, and the fact that threats “come from any direction, including behind, above and below.”
- Best-in-class dread is built *without* jump scares — through atmosphere, sound, oxygen/resource pressure, and the psychology of the unseen. Subnautica’s developers named their studio “Unknown Worlds” precisely to protect “that intoxicating shiver of the unknown.” 
- On an alien world specifically: “you can be pretty sure there WILL NOT be a giant 100-meter leviathan… but on an alien planet? Who knows what will be in the water?” This is our license to escalate.
- Bases/safe spaces provide emotional relief that makes the danger bearable and re-enterable.

### 3.2 Positioning

**Market reality (2025–2026):** Co-op is the single most reliable viral driver on Steam — co-op games grossed **$4.1 billion on Steam in H1 2025, an all-time high and ~11% year-over-year growth** (Alinea Analytics).  The biggest 2025 hits (R.E.P.O., PEAK, Elden Ring Nightreign, ARC Raiders) all had strong co-op.  But the space is crowded with “friend-slop” horror — nearly half the top viral co-op games are horror, and clones are landing closer and closer together. **99.9% of Steam releases are indie**, and indie made up **48% of Steam revenue and 58% of copies sold** in 2024 (Video Game Insights) — a democratized but brutal market where **48.6% of 2025 releases (9,370 of 19,267 games) got fewer than 10 reviews, and 2,229 got none** (SteamDB via PC Gamer).  Differentiation and production polish are survival requirements.

**Comparable sales (for expectation-setting; studio-confirmed and analytics estimates flagged):**

- **Dredge** — **over 1 million copies** (studio-confirmed by producer Nadia Thorne of Black Salt Games / Team17 at PAX Australia 2023; hit 100K in the first 24 hours),  **$24.99**, premium single-player horror-fishing. Proves the tonal-pivot fishing concept has a seven-figure ceiling.
- **Lethal Company** — **~10 million copies** (Push to Talk / Boxleiter estimate; solo dev never disclosed),  **$9.99**. The blueprint.
- **R.E.P.O.** — **15.4 million copies by end of June 2025** (Alinea Analytics estimate, ~$121M gross;  developer semiwork never officially disclosed — early estimates ranged 1.5M–3.1M, so treat as estimate), **$9.99** Early Access.
- **PEAK** — **11 million copies** (co-developer Aggro Crab, via GamesRadar+, Oct 2025;  Geoff Keighley announced 10M+ at Gamescom ONL Aug 2025),  **~$7.99**, built in a roughly month-long game jam between Aggro Crab and Landfall. 
- **Content Warning** — **8.8 million owners** (publisher Landfall confirmed: 6.6M from the free first 24 hours + 2.2M paid),  **$7.99**.

**Our wedge:** DEEPER is the first title to combine (a) a genuinely deep, skill-based **fishing loop**, (b) **co-op extraction horror** structure, and (c) **showcase-grade interactive water**. Dredge is single-player; the co-op horror hits don’t have real fishing or water tech; Webfishing is cozy-only with no threat. We sit in the empty middle of that Venn diagram.

**Price/positioning recommendation:** Launch in Early Access at **$12.99–$14.99** — above the $9.99 friend-slop floor (signaling higher production values, justified by the water tech) but well below Dredge’s $24.99 premium. This threads between “impulse co-op buy for a friend group” and “this looks more polished than the $10 clones.”

**Player count recommendation:** Ship **1–4 players.** Four is the proven sweet spot for proximity-voice co-op horror (Lethal Company, Content Warning, Phasmophobia, R.E.P.O. base all target 4). Solo must be viable (Dredge/Subnautica audience, and streamers who play alone), but the game should be balanced and marketed around 3–4. Do **not** exceed 4 at launch: physics-heavy water state-sync cost scales with player count, and intimacy/voice-legibility degrades past 4.

-----

## 4. CORE GAMEPLAY LOOP

### 4.1 Moment-to-moment (on the water)

1. **Read the water** — surface tells, shadows, bioluminescent flickers, foam, bird/creature analogues indicate what’s biting where.
1. **Cast** — aim and power your cast; it *splashes* (water VFX showcase moment), creating ripples that propagate and can attract or spook fish.
1. **Wait / lure** — jig the lure to attract specific species; downtime is filled with reading the environment and chatting (proximity voice).
1. **Hook** — a bite tell (rod bend, line twitch, audio); time the hook-set.
1. **Fight** — the tension minigame (see §5): manage line stress vs fish behavior.
1. **Land** — the catch breaches with a splash; you identify it, and slot it into the shared cargo hold (spatial Tetris).
1. **Decide** — keep fishing (greed), go deeper (more greed, more danger), or turn back (safety).

### 4.2 Session-level loop

**HUB (the Rig) → DESCEND into a zone → FISH & explore → EXTRACTION run back up → SELL/BANK at the Rig → UPGRADE → DESCEND deeper.**

A session is a series of **expeditions**. Each expedition: leave the Rig on your boat/diving bell, choose how deep to go, fish until your hold is full or your nerve breaks or a deadline looms, then make the extraction run home. Bank your catch, spend at the Rig’s shops, and go again — deeper each time. The persistent Rig means progress carries across a session and (partially) across sessions.

-----

## 5. THE FISHING SYSTEM (mechanical detail)

### 5.1 Design goal

Hit all three emotional axes (serenity/surprise/excitement) and make each catch a *decision*, not a slot pull. Avoid single-mechanic fatigue by layering three interacting subsystems: the **cast**, the **fight**, and the **cargo**.

### 5.2 The cast

- **Aim + power**: a short aim-and-hold; overpowering splashes loud (attracts predators at depth), underpowering falls short. This is a primary water-VFX moment — ripple propagation, splash crown, foam ring.
- **Lure jigging**: after the cast, small stick inputs “jig” the lure. Different species respond to different jig patterns (slow drags, sharp twitches, dead-stick). This is the “luring” mechanic and adds skill to the downtime.
- **Bait/lure type**: consumable bait and equippable lures bias which species bite (see economy).

### 5.3 The bite & hook-set

- A **tell** precedes the bite (rod bend, line ping, a shape rising in the water). Reaction-test hook-set: a timing window to set the hook. Miss it and the fish spooks.
- Alien fish have **deceptive tells** at depth — false bites, tells that mean “something big is watching,” etc.

### 5.4 The fight — tension & line stress (the core minigame)

A hybrid of **tension-reeling** (Sea of Stars/Stardew lineage) and **rhythm cues** (Dredge), because research says pure reaction-tests fatigue but rhythm stays fresh:

- **Line Stress meter** (0–100%). Reeling raises it; the fish’s runs raise it; slack lowers it. Snap the line at 100% → lose the fish (and the hook/lure).
- **Fish stamina bar** — depletes as you keep tension in a “fight zone.” When empty, the fish is landable.
- **Directional pull**: the fish lunges in directions; you counter-steer (opposite the pull) to keep it in the fight zone while watching stress. This is the tension-tracking skill.
- **Rhythm windows**: periodically the fish “tires” and a reel-window opens; hitting the rhythm reels fast with low stress. This is the fresh, Dredge-like layer.
- **Drag control**: an active resource — loosen drag to bleed stress during a big run (fish gains distance/stamina recovers slightly), tighten to reel faster (stress climbs). Deep-water bosses require deliberate drag play.

### 5.5 Fish AI / behavior archetypes

Each species is built from composable behaviors so catches feel distinct:

- **Sipper** (cozy zone) — gentle, forgiving, slow stamina drain. Teaches the loop.
- **Runner** — long fast lunges; drag management matters.
- **Thrasher** — erratic direction changes; punishes lazy counter-steering.
- **Diver** — repeatedly tries to sound (go deeper); stress spikes if you let it descend.
- **Sulker** — goes dead-weight; you must *not* reel (slack management) or the line snaps.
- **Lurer** — mimics a small easy fish, then reveals itself mid-fight (tonal-shift tool).
- **Retaliator** (deep) — fighting it too aggressively “aggros” nearby threats via noise/panic.
- **Anchor** (boss) — so strong it can pull the *boat* or the *diver*; requires multiple players (one reels, one manages drag, one watches the water).

### 5.6 What makes each catch distinct

- Unique **tell**, **jig pattern**, **behavior archetype combo**, **breach animation/VFX**, **cargo shape**, and **codex horror-lore entry**.
- **Co-op fishing**: big fish can be **tag-teamed** — one player’s line can be “assisted” by another to share stress load, or a second player gaffs the fish at the boatside to land it. Bosses *require* multi-player coordination.

### 5.7 The cargo hold (spatial Tetris)

Directly adapting Dredge’s celebrated system, made co-op:

- A shared **grid hold** on the boat/bell. Fish occupy shapes (1×1 sippers, 2×1 runners, L-shaped and cross-shaped aberrants that waste space).
- **Equipment also takes space** (rods, lights, oxygen, extra line) — the classic Dredge tension: more gear vs more catch capacity. (In Dredge you could sell an engine or leave a rod in storage to free hold space; we preserve that agonizing trade.)
- **Fish decay/agitation**: alien catches don’t just spoil — some **thrash in the hold**, some **leak** and corrupt adjacent cells, some **must be kept apart** or they react. This is a co-op packing puzzle done live under pressure.
- Deeper fish are worth exponentially more but are **awkwardly shaped and volatile**, so hauling a full hold of abyssal fish is a high-stakes extraction.

-----

## 6. ROD & GEAR UPGRADES, ECONOMY & PROGRESSION

### 6.1 Currencies

- **Scrip** — soft currency from selling fish at the Rig. Buys rods, gear, consumables.
- **Specimens / Research** — rare currency from delivering *intact* rare/aberrant fish to the Rig’s Marine Xenobiology lab. Gates the tech tree (Dredge’s “research parts” model, which players explicitly preferred over pure cash).
- **Nerve** is not a currency but a soft session resource (see horror, §11).

### 6.2 Rod stats (Fisch-inspired, trimmed to legible axes)

- **Lure Speed** — how fast fish bite.
- **Luck** — rare-species and mutation chance.
- **Control** — size of the fight zone / counter-steer forgiveness.
- **Resilience** — max Line Stress before snap.
- **Max Weight/Depth** — deepest zone the rig can fish and biggest fish it can hold.

### 6.3 Progression curve (avoid grind fatigue)

- **Meaningful jumps, not increments.** ~6–8 rod tiers total, each a clear capability unlock (new depth zone accessible, new archetype catchable), not a +2% stat. Explicitly avoid Fisch’s “buy a rod you replace in 10 levels” trap.
- **Gear beyond rods**: reinforced line, drag systems, sonar/fishfinder (reveals shapes below — a horror double-edged sword), hull/bell lights (combat darkness & Nerve, à la Dredge), oxygen tanks (for diving depth), cargo expansions, and the **winch** (extraction speed).
- **Anti-grind design**: failing an expedition never fully resets you (see death, §12). The Rig persists. The curve is tuned so a group hits a satisfying capability unlock roughly every 30–45 minutes in the first few hours.

### 6.4 Economy pressure (the “quota,” reframed)

Rather than a punishing rising quota that resets you (the fatigue trap — Content Warning fully wipes equipment, money, and quota on failure), DEEPER uses **soft pull economics**: the Rig has a **debt / life-support cost** that ticks per in-game day, creating gentle pressure to keep earning — but missing it degrades the Rig (lights flicker, shops close) rather than game-overing you. The real pressure is *player greed* + *horror escalation*, not an external timer. This is the deliberate fix to the “quota grind fatigue” complaint.

-----

## 7. THE HUB — “THE RIG”

A rusted, floating platform lashed to the surface above the trench — part oil rig, part fishing wharf, part frontier town. This is the **cozy tonal baseline** and the safe room that makes the horror re-enterable (Subnautica-base principle: a base “becomes more than shelter — it becomes safety inside a dangerous world”).

**What you do there:**

- **Sell & bank** your catch (Fishmonger).
- **Upgrade** rods and gear (Shipwright / Tackle shop).
- **Research** aberrant fish (Xenobiology lab — unlocks tech, codex lore, and the horror drip).
- **Cosmetics** vendor (see §14).
- **Socialize** — proximity voice, emotes, a jukebox, a communal campfire/galley. Deliberately Webfishing-like: a place to *just exist together* between the terror.
- **Read the water** — from the Rig you can see the weather rolling in (Loom’s wind/rain/weather systems on display) and choose your next descent.

**Why players return:** it’s safe, it’s warm, it’s where progress is banked, and — critically — as the game escalates, **the Rig itself begins to change.** Early it’s cozy and bright. Later, the lights fail intermittently, NPCs act strange, things wash up on the platform, and you’re no longer sure the Rig is safe. The one place that was home stops being home. (This is the tonal-pivot principle applied to the hub: curdle the safe space last.)

-----

## 8. THE OPEN WORLD & DEPTH ZONES

A **vertical ocean** — one continuous body of water divided into stacked depth strata, gated by rod/gear tier. Horizontal exploration exists within each zone (reefs, wrecks, trenches, kelp analogues, thermal vents), but the primary axis is **down**. Depth is difficulty, story, and horror on one slider (Pillar 5). This is grounded in the research that Subnautica scores biome scariness on brightness, depth, hostility, and visibility — all of which we can escalate monotonically by descending.

Recommended **5 zones** for the full game (VS1 ships zones 1–2; VS2 adds 3; full game 4–5):

|#|Zone                            |Depth     |Light          |Tone      |Signature                                                                                         |
|-|--------------------------------|----------|---------------|----------|--------------------------------------------------------------------------------------------------|
|1|**The Shallows / Sunlit Lagoon**|0–40 m    |Bright, warm   |Cozy, safe|Tutorial fish, gorgeous caustics, calm water. The “before.”                                       |
|2|**The Kelp Reach**              |40–150 m  |Green, dappled |Uneasy    |Bleached alien kelp forest (Subnautica ghost-forest principle), low visibility, first “wrongness.”|
|3|**The Twilight / Murk**         |150–600 m |Dim blue → dark|Tense     |Bioluminescence begins; things move at the edge of your light; first non-catchable threats.       |
|4|**The Midnight Shelf**          |600–2000 m|Near-black     |Fear      |Full darkness, pressure, oxygen matters, boss fish, the hunters.                                  |
|5|**The Still Deep / Abyss**      |2000 m+   |Black          |Terror    |The bottom. Where the game’s cosmic-horror payoff lives. Extraction from here is a gauntlet.      |

**Gating & pacing:** each zone requires a rod/depth tier + often a gear unlock (lights, oxygen, pressure hull). Tone shifts are gradual within a zone and sharp at boundaries — crossing into a new zone is a *set piece* (the light changes, the music drops out, the water goes still). The descent from bright to black *is* the horror curve.

-----

## 9. THE EXTRACTION LAYER

The hybrid’s third pillar, and the source of the game’s core tension. **Nothing you catch counts until it’s banked at the Rig.**

- **The ascent is the risk.** Going deep is optional greed; coming back is mandatory and dangerous. The deeper you fished, the longer and scarier the trip up.
- **What triggers extraction pressure:**
  - **Cargo weight/volume** — a full hold of volatile abyssal fish slows your ascent and attracts hunters.
  - **Nerve / darkness** — the longer you stay deep, the more the world turns hostile (Dredge Panic-analog, see §11).
  - **The hunters wake** — the deepest zones have non-catchable threats that begin actively pursuing you once you’ve taken enough from the water (see §13).
  - **Oxygen / life-support** — for diving-bell/diver play at depth, a hard resource clock (Subnautica/Iron Lung principle).
- **The winch / ascent run**: extraction is an active phase — you’re reeling the bell/boat upward while managing lights, fending off or evading threats, and keeping volatile cargo stable. Co-op shines: someone drives, someone watches the water, someone manages cargo.
- **Partial extraction / jettison**: you can dump cargo to ascend faster (drop the big boss fish to survive) — an agonizing greed-vs-life choice, live, over voice chat. This directly models the extraction-horror tension research praises in R.E.P.O. and Grain Rot: “if the whole crew breaks before reaching safety, the collected haul is lost.”

-----

## 10. HORROR ESCALATION — the cozy-to-terror plan

### 10.1 What research says makes a tonal pivot *land* vs feel cheap

- It must be **earned by a genuine cozy baseline** (Dredge, DDLC, Inscryption): the comfort has to be real first. Players who came to Dredge for the daytime fishing sim enjoyed it “even while knowing what waited in the dark water” — the warmth and the wrongness coexist. 
- It should **escalate gradually with spikes**, not flip a switch — Subnautica’s “pervasive dread” builds through atmosphere and uncertainty, not a single reveal. Black Salt Games deliberately made Dredge a “psychological thriller” rather than a “jump scare horror game.”
- **Player-driven pacing** lets it land differently for everyone (you go deeper *by choice*, so the horror feels self-inflicted and therefore fair).
- Avoid **telegraphing** — no “spooky” music in the cozy zone. Keep the surface sincerely pleasant.
- **Multiplayer complication (the hard part):** players may join a session mid-progression. Solution below.

### 10.2 Escalation ladder (zone-by-zone / hour-by-hour)

**Hour 0–1 (Zone 1, The Shallows) — Cozy.** Sincere comfort. Warm light, lofi-ish ambient, funny fish, jokey codex entries, proximity-voice banter. The only “tell” is subtle: an occasional too-deep shadow, a fish with one too many eyes played for whimsy not horror. Players should *forget they bought a horror game.*

**Hour 1–3 (Zone 2, Kelp Reach) — Wrongness.** The “uncanny cozy.” Vegetation is the wrong color (ghost-white kelp bleeding red sap — direct Subnautica principle, where blood-kelp works because “we expect vegetation to be green” and the conflict is unsettling). Fish tells get deceptive. First **Lurer** catches. Distant sounds with no source. Nothing hurts you yet — dread without danger (the Dredge first-night principle: “no immediate danger, but the sound design and visuals convince you there is”).

**Set piece — “The First Descent Below the Light”:** the scripted moment you first drop past the point where sunlight reaches. The water goes from blue to black around the bell; the music cuts to silence + sonar ping. First encounter with a non-catchable presence: something huge passes *below* you on the fishfinder, unseen. It doesn’t attack. It just… was there.

**Hour 3–6 (Zone 3, Twilight) — Threat arrives.** Bioluminescence as the only light. The **Nerve** system now bites (§11). First hunters that can actually hurt you appear but are avoidable. Fishing now means provoking things. The **Retaliator** archetype makes aggressive fishing dangerous. Boss catch #1 (see bestiary).

**Set piece — “Lights Out”:** a zone event where all artificial light fails for ~30 seconds and the only illumination is the bioluminescent fish and something’s glowing lure in the dark that is *not* a fish.

**Hour 6–12 (Zone 4, Midnight Shelf) — Fear.** Full darkness, oxygen pressure, active hunters that pursue. Extraction becomes a genuine gauntlet. The Rig starts to change between expeditions (§7). Boss catches that can pull the boat.

**Set piece — “The Rig Isn’t Safe”:** the first time you surface and the Rig is dark, an NPC is missing, and something wet has come up onto the platform. The safe space is compromised.

**Hour 12+ (Zone 5, The Still Deep) — Terror & payoff.** The abyss. Cosmic-horror reveal: the alien fish were never the point; you’ve been feeding/waking something vast, and the whole ocean is one organism (or one entity’s body). The final boss catch is less “fish” and more “you realize what you’ve been fishing *in*.” Extraction from here is the hardest content in the game.

### 10.3 Handling drop-in / mixed-progression multiplayer

- **Horror state is tied to depth/zone, not to a global session timer.** A new player who joins and stays on the Rig or in the Shallows experiences cozy; the horror only applies where you are. This keeps the pivot coherent regardless of when someone joins.
- **The host’s world-progression** (which zones are unlocked, how far the Rig has curdled) persists; joiners inherit the host’s escalation state for the *hub*, but the *per-zone* tone is spatial, so it always reads correctly.
- **Session escalation resets softly**: the deep-water “hunters awake” aggression state cools down over time when players retreat, so the game re-breathes between panic peaks rather than staying maxed (research: pressure that never releases causes fatigue; the calm-to-panic *cycle* — “every run starts calm and ends in panic”  — is the appeal).

-----

## 11. THE NERVE SYSTEM (sanity / panic analog)

A shared and individual dread meter, the Dredge-Panic descendant, tuned for co-op. In Dredge, Panic is an eye icon that opens and reddens in darkness, spawning visual distortion, phantom rocks, and hallucinatory threats, and it falls in light or at dock — we adapt this directly:

- **Individual Nerve** rises in darkness, near threats, when a teammate dies, when hooking something horrifying. Manifests as audiovisual distortion, false tells, phantom shapes, hallucinated sounds over proximity voice.
- **Falls** in light, near teammates (proximity!), at the Rig, and on successful catches (the “excitement” release).
- **Co-op hook:** being *near a teammate* lowers Nerve — so the horror actively pushes players together, then the fishing/extraction pulls them apart. That tension is the whole game.
- **High Nerve consequences** are non-lethal but destabilizing (you can’t trust your own senses), which sets up *lethal* threats you must distinguish from hallucination. (Dredge’s phantom-rocks principle — where high panic throws “ghost sharks and demonic tornadoes” and illusory cliffs at you.)

-----

## 12. DEATH, FAILURE & RECOVERY (players can die)

Confirmed design: **full threat, players can die.** But co-op death must stay *fun*, not punishing (the Lethal Company balance, where losing equipment after a bad run “can feel punishing” is a known sore point).

- **Downed → Dead states.** A hit at depth first **downs** you (you’re in the water, sinking, can call for help over voice). A teammate can rescue within a window (drag you to the bell, use a rescue buoy). Unrescued, you drown/are taken → **dead.**
- **Dead but not out.** Following Lethal Company’s model that keeps dead players engaged: in Lethal Company a dead player is cut from the living’s voice chat and can only respawn when the ship reaches orbit — we keep the eerie disconnection but soften the dead time. A dead player becomes a **drifting light / spectral buoy** who can still see, can *ping* the water for the living (limited, spooky communication), and is revived when the crew **extracts to the Rig.** Death removes you from voice with the living (the signature “their voice is gone” horror) but you can still haunt-ping.
- **What you keep vs lose:**
  - **Banked catch & all Rig progress: always safe.** Never reset. (Directly rejecting the quota-full-reset fatigue trap that wipes progress in Content Warning.)
  - **Unbanked cargo on the boat**: at risk. If the *whole crew* wipes before extraction, the current expedition’s unbanked haul is lost (the R.E.P.O./Grain Rot “haul is lost if the crew breaks” tension) — but rods/gear/Rig persist.
  - **On solo death**: you lose the current unbanked haul and respawn at the Rig; no permadeath, no progress wipe.
- **Recovery loop:** wiping is a *story*, not a punishment — you lost *this haul*, you keep everything permanent, and you go again. The cost is the sting of the fish that got away, not hours of progress.
- **Accessibility:** offer a Dredge-style “Passive” toggle that softens or removes the lethal threats for horror-averse players who came for the fishing (Dredge shipped exactly this so anxious players could still enjoy the game).

-----

## 13. THREATS / ANTAGONISTS (non-catchable)

The things that hunt you. Distinct from catchable fish — you cannot beat them, only evade, deter (light), or outrun. Design rule (from research): threats respond to **player behavior** — light, sound, mass/greed, aggression — so no single strategy is safe (the Barotrauma/FEEDERS principle where “a survival tactic effective against one threat becomes dangerous when another is nearby”).

- **The Latcher-analog — “GRACKLE”** (Twilight+): a mass of glowing eyes in the dark; if it sees your light through the bell window, it approaches. Kill your lights to hide (Dredge miasma / Barotrauma Latcher principle — the Latcher is “a large creature with many glowing eyes, revealing its position to anyone gazing through windows or a periscope”).  Punishes the light you need to fish.
- **“THE TITHE”** (Midnight): a huge slow presence that circles the bell. Every expedition into its zone, it takes *one thing* — a fish from your hold, a piece of gear, or, if nothing’s offered, a player. Learn to leave an offering.
- **“HUSHERS”** (any deep zone): they hunt by sound. Proximity voice becomes a liability — talk and they come. Forces players into tense silence, then whispered coordination. (Turns the genre’s core mechanic into a horror mechanic.)
- **“THE ASCENT”** (extraction threat): the deeper you fished, the more likely that on the way up, *something followed the bait.* A pursuit creature that chases your winch to the surface — the reason extraction is scary.
- **The Still Deep entity** (Zone 5): not an enemy you fight — an environmental, cosmic dread. The ocean itself reacting to you (Subnautica’s Warper-style “concepts beyond your understanding”).

-----

## 14. MULTIPLAYER DESIGN

- **Room codes.** Host creates a session, shares a short code, up to 3 others join. Private-by-default (the Webfishing/We-Were-Here friends-first model). Optional public matchmaking later.
- **Pure co-op.** No PvP, no sabotage, no hidden roles. Confirmed. All incentives are aligned; the enemy is the deep.
- **Proximity voice chat — the priority-one feature.** Spatialized 3D voice that falls off with distance and is muffled underwater / through the bell hull. This is *the* emotional engine (§2, Pillar 3). Research on the pipeline: capture mic input, encode/transmit audio packets at ~20 ms intervals to nearby positions, then spatialize on the receiving end as a 3D point source relative to the listener. Implementation must be first-class, not bolted on.
- **Emergent roles (not classes).** Like Lethal Company — where “the ship operator, the reckless explorer, and the person carrying all the scrap are not official classes; your friends create those roles through their own habits” — players self-assign: the Angler (fights the big fish), the Spotter (watches the water/sonar), the Winch-hand (drives extraction), the Cargo-master (packs the hold).
- **What co-op adds mechanically:** tag-team fishing (shared line stress), multi-crew bosses (Anchor fish need 2–3 people), the rescue loop, Nerve-reduction-by-proximity, and the Husher sound dilemma.
- **Communication tools as progression/horror:** buyable radios/sonar-pings extend communication range — and dead players get the spooky haunt-ping. Losing voice contact (distance, death, Hushers) is engineered constantly — the research-identified magic of “splitting up to cover more ground means losing contact with the people who could warn you.” 

-----

## 15. COSMETICS & SECOND-SLICE PROGRESSION

- **Character cosmetics**: diving suits, helmets, hats, the crude-funny Webfishing-style items that make great screenshots (shareability = marketing; Webfishing’s fishable “I love peeing” hat is exactly the tone that goes viral). Cosmetic-only, no stats.
- **Boat/bell cosmetics**: hull paint, light colors (bioluminescent-reactive), trophy mounts of boss catches on the Rig.
- **Rig customization**: decorate the communal hub (Webfishing campsite principle — “personalize your campsite… a cozy gathering spot for friends”) — a shared social space players personalize together.
- **Codex/bestiary completion**: catching every species and photographing threats is the completionist meta-goal (Dredge’s 128-fish encyclopedia, Fisch’s bestiary requirements for rod unlocks). Rewards cosmetics + lore.
- **Earned, not (initially) monetized**: cosmetics unlock via play in VS2. A later cosmetic-DLC or battle-pass-lite is a *possible* post-launch revenue lever but not a launch feature (keep launch clean; the friend-slop audience is price-sensitive and MTX-wary).

-----

## 16. SOUND DESIGN & AUDIO DIRECTION

Critical for this genre — research repeatedly cites sound as the primary dread engine (Dredge’s night, where “there’s no immediate danger, but the sound design and visuals will convince you there absolutely is”; Iron Lung’s ambient isolation; Subnautica’s distant creature calls).

- **Cozy layer (surface/Shallows):** warm, melodic, sparse ambient; gentle water lapping, creaking Rig, birdsong-analogs. Sincerely pleasant.
- **The descent = subtraction.** As you go deeper, music *removes* layers rather than adding scary stings. By the abyss it’s near-silence + sonar ping + your own breathing + the hull groaning under pressure. Silence is the scariest instrument.
- **Diegetic dread:** distant unexplained creature calls, the winch cable straining, sonar pings that return *too many* contacts, hydrophone chatter.
- **Water as sound showcase:** casts, splashes, breaches, rain on the hull, wind across the water — the audio partner to Loom’s water visuals.
- **Proximity voice is part of the mix** — muffled by water/hull, cutting out with distance/death. The Husher threat weaponizes it.
- **Nerve distortion:** at high Nerve, audio hallucinations — a teammate’s voice that isn’t real, a splash behind you.
- **Recommendation:** integrate a spatial-audio middleware layer early (an FMOD-style engine driving both proximity voice and 3D ambient, as used in the reference proximity-chat implementations). This is not a polish-phase task; it’s core tech.

-----

## 17. ART DIRECTION & VISUAL IDENTITY

**Identity:** “Cozy postcard that rots.” Start with a warm, slightly stylized, painterly realism — inviting, saturated, golden-hour surface water. Descend into desaturation, bioluminescent accents against black, and finally near-total darkness pierced by lures.

**Showcasing the Loom water/weather/vegetation tech (Pillar 1):**

- **Surface (Zone 1):** the hero shot — crystal water, dynamic caustics on the seabed and hull, refraction, interactive ripples from casts and wakes, foam on the boat’s trail, sun glints. Reference technique stack (from research): FFT/spectral wave surfaces, GPU-computed refractive caustics, foam/spray on wave crests and object interaction, and buoyancy-driven wakes. This is the marketing screenshot and the engine demo.
- **Weather & wind:** rolling storms visible from the Rig, rain hammering the water surface (interactive splashes), wind driving waves and bending the alien kelp/grass — all Loom systems Joe has built, on display.
- **Vegetation & scatter:** procedural kelp forests, reef scatter, the ghost-white bleached kelp of Zone 2 — using the grass/scatter systems.
- **Depth transition:** volumetric light shafts fading to god-rays fading to black; particulate “marine snow” drifting up; pressure haze.
- **Bioluminescence:** the primary light-design tool in Zones 3–5 — creatures, lures, and the players’ own gear glowing against the dark (research: ~90% of animals below 500 m are bioluminescent, and it’s “the predominant source of light in the largest fraction of the habitable volume of the earth”). Every light is also a risk (attracts the Grackle).
- **Creatures** as living VFX: translucent, glowing, wet, wrong (see §18).

**Reference vibe:** Dredge’s approachable stylization + Subnautica’s biome dread + Abzû’s water beauty, curdling toward the black of Iron Lung.

-----

## 18. ALIEN FISH BESTIARY

Design principle (from creature-design research): **don’t do “earth fish + extra eyes.”** Make them alien through *function and wrongness* — hidden or absent eyes (Giger/Ken Barthelmey principle: designers deliberately “hide the eyes” because “it’s strange and scary to look at something that doesn’t have eyes,” a key reason Giger’s Alien terrifies), impossible anatomy that still implies a survival strategy (avoid the “shrink-wrapped” lazy-alien look; give weird faces a feeding rationale), biomechanical and bodily-autonomy horror (Alien: Earth’s creator built new monsters by asking “what upsets me most — bodily autonomy, parasites”), and real deep-sea biology pushed past plausibility (anglerfish sexual parasitism where males fuse into the female’s body and share a circulatory system; barreleye transparent heads with rotating tubular eyes; dragonfish that hunt with red bioluminescence invisible to their prey). Each fish should imply an *ecology* you don’t want to think about.

### Zone 1 — The Shallows (cozy, whimsical-wrong)

- **Gleamsprat** — a palm-sized fish that’s *slightly too reflective*, like wet chrome. Schools mirror your boat. Harmless. Sipper. (Teaches the loop. The “extra eye” is played for cuteness.)
- **Puddlejack** — inflates with swallowed light; glows faintly when held. Kids-cartoon adorable. Foreshadows bioluminescence.
- **Warblefin** — hums a pleasant note when landed. Codex: “the note matches no fish we know. It matches a voice.” (First tiny wrongness.)

### Zone 2 — Kelp Reach (uncanny)

- **The Kelphusk** — looks like a strand of kelp until it bites; a **Lurer**. Its “leaves” are feeding filaments.
- **Palegazer** — utterly transparent flesh (barreleye principle); you can see its last meal, and its last meal has *your boat’s shape* in its gut. Thrasher.
- **Sapfish** — bleeds the same red “sap” as the ghost-kelp; leaks in the hold and corrupts adjacent cells (packing hazard).
- **Choruseels** — caught in tangled knots; each one you land makes the others in the water “sing” — raises Nerve.

### Zone 3 — Twilight (bioluminescent dread)

- **Lantern-Widow** — an anglerfish-analog whose lure is a *tiny perfect human hand* holding a light (bodily-autonomy horror). **Retaliator** — fight it hard and its glow summons the Grackle. Sexual-parasitism lore: the “males” are the barnacle-like growths on your own hull.
- **Barrowlight** — transparent domed head (barreleye), eyes that track *you specifically* through the glass, even in the cargo hold. Diver.
- **Redtongue Dragon** — hunts with red bioluminescence invisible to other fish (real dragonfish biology); in the hold it can “see” in the dark and agitates other catches.
- **The Gasp** — a fish made mostly of a single enormous mouth and a breathing sac; when landed it exhales a lungful of air that *sounds like a human gasp.* Sulker.

### Zone 4 — Midnight Shelf (fear)

- **Choirspine** — a colonial organism; what you hook is one “voice” of a larger body still in the dark. Landing it makes the rest of the colony pull back on your line — a mini-boss tug-of-war.
- **The Hollow Catch** — appears as a common Zone-1 Gleamsprat on the fishfinder, but at the surface it’s a hollow, fish-shaped *lure* — and something used it to bait *you.* (Weaponizes player pattern-recognition.)
- **Meatpearl** — grows pearls that are clearly *teeth*; high value, volatile in hold (thrashes).

### Boss catches (the memorable set pieces)

1. **THE FOUNDLING** (Zone 3 boss) — an **Anchor** fish the size of the bell; a bioluminescent juvenile of something *much* larger (its parent is the shadow you saw in “The First Descent”). Requires a 2–3 player fight. Landing it triggers the parent’s attention for the rest of that expedition.
1. **THE CARTOGRAPHER** (Zone 4 boss) — a creature covered in glowing markings that map the trench; players realize the “map” is a chart of *every crew that came before, and where they died.* Fighting it, its markings rearrange to show *your* boat.
1. **MOTHER OF LURES / “THE ALLGLOW”** (Zone 4–5) — a colossal anglerfish whose lure field looks like a whole friendly village of little glowing fish; players fish “the village” for minutes before the lures retract and reveal the mouth they were all attached to. The ultimate Lurer, played across a whole area.
1. **THE STILL DEEP / “IT WAS NEVER WATER”** (Zone 5 finale) — not landed so much as *survived.* You hook the abyss floor and it moves. The reveal: the ocean is the entity; every fish was a cell of it; you’ve been fishing inside a god. Extraction becomes an escape.

-----

## 19. VERTICAL SLICE 1 — SCOPE (fishing + rod upgrades)

**Goal:** prove the core fishing loop is fun, the water tech is stunning, and co-op + proximity voice sing. This is the demo that gets wishlists.

**IN:**

- 1–4 player co-op, room-code join, host-authoritative session.
- **First-class proximity voice chat** (non-negotiable — it’s the pillar).
- **Zone 1 (Shallows) + Zone 2 (Kelp Reach)** only.
- Full **fishing system**: cast/jig/hook/fight (tension + rhythm + drag), 3–4 behavior archetypes.
- **~12–15 fish** across the two zones + **1 boss catch** (The Foundling, tuned as VS1 climax) + **1 Lurer** to plant the first horror seed.
- **Spatial cargo Tetris** (shared hold), basic fish-shape variety, one volatile-fish hazard (Sapfish leak).
- **The Rig hub** (minimal): sell, one upgrade shop, campfire/social space.
- **Rod upgrades**: 3–4 tiers with the 5 legible stats; ~4–5 gear items (line, light, cargo expansion, sonar).
- **Extraction (light version)**: haul back from Zone 2, with the first “descent below the light” set piece and one non-catchable presence (unseen pass on sonar).
- **Nerve system v1** (darkness/proximity).
- **Loom water showcase**: interactive cast splashes, foam wakes, caustics, one weather event (a rolling storm on the surface).
- **Death/downed/rescue** loop (basic).

**CUT from VS1 (deferred):**

- Zones 3–5, active hunters (Grackle/Tithe/Hushers), oxygen/diving-bell depth mechanics, the Rig curdling, cosmetics, most bosses, research/Xenobiology tree (use simple scrip only), public matchmaking, host migration, the cosmic-horror narrative payoff.

**Success metrics:** a 3–4 player group plays 45+ min without prompting “what now?”; the fishing fight is described as “satisfying”; at least one organic “did you hear that?” proximity-voice moment per session; the water gets screenshotted.

-----

## 20. VERTICAL SLICE 2 — SCOPE (depth progression + cosmetics)

**Goal:** prove the descent/horror-escalation curve and the return-loop that drives long-term play.

**IN:**

- **Zone 3 (Twilight)** added, with bioluminescence lighting, the “Lights Out” set piece, and the **first real non-catchable hunter** (the Grackle — light-based hiding).
- **Nerve system v2** (full audiovisual distortion, hallucination tells).
- **Oxygen / life-support** resource for deeper fishing; the winch-based **full extraction gauntlet**.
- **Research/Specimens currency + Xenobiology lab**; the tech tree replacing flat scrip upgrades.
- **~8–10 more fish** incl. Zone-3 species and **boss #2 direction**; the Retaliator archetype.
- **Cosmetics v1**: character + boat + Rig customization, bestiary/codex completion rewards.
- **The Rig begins to change** (first curdle events between expeditions).
- **Rod tiers 5–6**, deeper gear (pressure hull, better lights, drag systems).
- **Netcode hardening**: state-sync for the added physics/water interactions at depth; ideally host migration.

**CUT / deferred to full launch:**

- Zones 4–5, the cosmic-horror finale, the Tithe/Hushers/Ascent hunters, boss catches 3–4, monetized cosmetics, public matchmaking polish.

-----

## 21. TECHNICAL CONSIDERATIONS & RISKS (custom Rust/Vulkan engine)

**Engine context:** Loom — custom Rust engine, Vulkan-native via `ash`, ~33.5k LOC, 16 workspace crates, has an editor, with in-progress water, grass, procedural scatter, rain, and wind systems.

**Opportunities:**

- The water rework directly *is* the game’s central showcase — tech and design are aligned, which is rare and powerful.
- Rust’s safety/perf is well-suited to networked simulation.

**Risks & mitigations:**

1. **Multiplayer netcode is the single biggest risk.** A custom engine has no off-the-shelf Netcode-for-GameObjects/Vivox. Recommendation: **host-authoritative** model (one player hosts, à la Lethal Company/R.E.P.O. room-code co-op), which fits 1–4 players and avoids dedicated-server cost. Physics-heavy water and cargo must be **host-simulated with client interpolation**; do NOT try to deterministically sync full fluid sim — sync *results* (fish state, cargo state, boat transform, key splash events) and let each client render water locally. This is the make-or-break engineering call.
1. **Proximity voice from scratch** is hard but essential. Options: integrate an existing voice transport (e.g., a Discord Social SDK-style voice layer — which “handles everything about the voice call itself” and lets the game “put the audio in the right place” via its own 3D audio system — or an open voice library) and route decoded audio into Loom’s 3D audio system for spatialization, rather than building voice networking from zero. Budget real time for this — it’s a pillar, not a nicety.
1. **Water state-sync cost scales with player count** — another reason to cap at 4.
1. **Interactive-water performance**: interactive ripples, foam, caustics, and spray at the fidelity promised are GPU-heavy (research notes stable-60fps GPU caustics require careful shader budgeting). Needs aggressive LOD (the Zone 1 hero water can be expensive; the black abyss is cheap — lean into that; darkness is both horror and optimization).
1. **Content pipeline for a solo/small dev**: the bestiary and zones are content-hungry. Mitigate with the composable-behavior fish system (archetypes + skins) and procedural scatter/kelp so zones aren’t all hand-placed.
1. **Scope discipline**: research’s clearest lesson is that thin content kills co-op horror. But a solo dev cannot build 5 zones + 30 fish + 5 hunters for a first release. **Recommendation:** ship VS1 as a free demo → Early Access with Zones 1–3 → expand. The engine’s showcase quality is the differentiator that buys patience the pure clones don’t get.
1. **Editor/tooling**: Loom having an editor is a major asset for zone/fish authoring — invest in fish-behavior and encounter-scripting tooling early.

-----

## 22. OPEN QUESTIONS & DESIGN RISKS

1. **First-person, third-person, or top-down-ish boat view?** Dredge is top-down; horror and proximity-voice intimacy favor first/third person on the water and inside the bell. **Leaning first-person on the boat/bell, with diving segments** — but this needs a prototype gut-check.
1. **Boat-fishing vs diving-bell vs free-diving** — how much of the game is on the surface vs submerged? Proposal: surface boat for Zones 1–2, diving bell for 3–5, optional risky free-dive for treasure. Needs validation.
1. **How punishing should full-crew wipe be?** Losing the unbanked haul only may be too soft for tension or too harsh for cozy players. Consider a difficulty toggle (Dredge’s “Passive” mode precedent — it let horror-averse players enjoy the fishing).
1. **Does the cosmic-horror narrative payoff survive repeat play / drop-in co-op?** A one-time story reveal can’t carry replay (the directed-story-game problem). The *systemic* horror (hunters, Nerve, extraction) must carry longevity; the narrative is a first-playthrough bonus.
1. **Monetization tail:** cosmetics-only DLC vs paid expansions vs none. Recommend paid content expansions (new zones/bosses) over MTX, matching the premium-ish positioning.
1. **Balancing solo vs 4-player** across every fishing/threat system — bosses that *require* co-op need a solo alternative or must be gated to co-op.
1. **Horror fatigue vs cozy dilution** — the central tonal tightrope. If escalation is too slow, the horror crowd bounces; too fast, the cozy hook is lost. This is the #1 thing to playtest.
1. **Proximity-voice moderation** for public lobbies (a known safety issue for this genre) — private-by-default room codes mitigate at launch.

-----

*This GDD synthesizes research on co-op horror (Lethal Company, R.E.P.O., Content Warning, Phasmophobia), horror-fishing (Dredge), thalassophobia/deep-water design (Subnautica, Iron Lung), cozy-to-horror tonal pivots, alien creature design, indie co-op market conditions (2025–26), and proximity-voice netcode, mapped onto the specific capabilities of the Loom engine. Throughout, “what research says players want” (§3.1) is kept separate from “the specific design proposal.” Sales figures blend studio-confirmed numbers (Dredge, PEAK, Content Warning) with third-party analytics estimates (R.E.P.O., Lethal Company), flagged as such — treat estimates as directional, not precise.*