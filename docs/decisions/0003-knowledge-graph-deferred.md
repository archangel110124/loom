# ADR 0003 — The knowledge graph has no milestone; defer it past M9

- **Date:** 2026-07-30
- **Status:** proposed — needs a human decision
- **Decision touched:** LOOM-BUILD-BRIEF.md §3 (repo layout) and §6 (milestones)

## Context

`ai-native-engine-design.md` §2.7 specifies a SQLite index over the project — nodes and edges
derived from files on disk, rebuilt incrementally by a file watcher — and argues it is "the answer
to full context": the agent gets a two-hop neighborhood on demand rather than a dump that will never
fit in a context window. §2.8 lists `graph_query` among the ten always-loaded MCP tools. §2.13 lists
`loom_graph/` as a crate. Design-doc Phase 4 builds it, with the exit criterion *"what would break if
I changed the desk prefab?"* answering correctly on a project with 200+ files.

**None of that survived into the build brief.** Verified by grep across all six design docs:

- `loom_graph` appears in the brief's §3 crate layout: **zero times**
- `graph_query` appears in the brief's §6 milestone list: **zero times**
- `SQLite` appears in the brief: **zero times**
- The only trace is one clause in M12 — "the asset browser, the knowledge-graph view" — a UI panel
  over an index that no milestone builds

Design-doc Phase 4 was "import pipeline + `.meta` + manifest + hash cache **and** SQLite indexer +
watcher + `graph_query`". The brief's M5 kept the first half and dropped the second. This looks like
an editing casualty rather than a decision, since nothing anywhere argues against the graph.

## Decision

**Proposed: leave it out until after M9, deliberately, and record that here rather than leaving a
silent hole.**

Two reasons, and the second is the real one.

**It is not needed for the gate.** M9's exit criterion is a computer lab: six desks, monitors, a
teacher desk, overhead lights, a trigger script. That is one scene file and a handful of prefabs.
`scene_query` reads the tree directly. There is no cross-file impact question to answer because
there are barely any cross-file references yet.

**Building it before M9 is precisely the trap §7.16 names.** The graph is attractive infrastructure
— a clean schema, a satisfying recursive CTE, a force-directed view at the end of it — and it is
worth roughly a week that M9 does not get. §7.16 says nothing beyond a forward renderer ships before
the gate; the same logic applies to indexing infrastructure for a project that currently has one
crate and no assets. The design doc's own exit criterion assumes 200+ files. The project will not
have 200 files at M9.

The design doc is right that retrieval beats a context dump. It is right *at scale*, and the scale
arrives later than the gate does.

**Proposed placement: M9.5, immediately after the agent loop is proven.** By then real scenes,
prefabs, and assets exist, the 200-file exit criterion is testable, and — the actual signal — the
agent will have spent M9 demonstrating whether it loses track of cross-file references. Build it in
response to that evidence rather than ahead of it. If M9 shows the agent has no trouble, the graph
may stay deferred indefinitely and become the visualization it partly always was.

## Consequences

- `graph_query` is not among the MCP tools at M9. The tool set ships as nine, not ten. Nothing else
  in the design depends on it — the two-hop context pack is an optimization over reading files, not
  a correctness requirement.
- `loom_graph/` does not appear in the crate layout until M9.5. The brief §3 layout stays as-is.
- M12's knowledge-graph view acquires a real dependency on M9.5. If M9.5 never happens, that clause
  in M12 must be struck too — a view over a nonexistent index is not a feature.
- The `.meta` / content-hash work at M5 stands unchanged. It is the asset identity system, not the
  graph, and the graph would consume it rather than replace it.
- **Risk if this is wrong:** the agent starts losing track of cross-file references somewhere in
  M9's blockout task, and the fix is a week of indexer work mid-gate. Mitigation: the graph is a
  pure cache derived from files (design doc §2.7 is emphatic), so it can be added at any time
  without migrating anything. That is what makes deferring it cheap and reversible.

## Human approval

**Required — this changes the milestone plan.** Three ways to go:

1. **Defer to M9.5** (recommended, argued above)
2. **Fold the indexer into M5** and `graph_query` into M9, restoring design-doc Phase 4 as written —
   costs roughly a week before the gate
3. **Cut it permanently**, and strike the knowledge-graph view from M12 as well

Awaiting a decision. Until one is recorded here, the brief stands as written and no graph work
happens — which is behaviourally identical to option 1.
