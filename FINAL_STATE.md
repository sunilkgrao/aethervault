# AetherVault Final State

This file is the architectural north star for the repository. It is not the user's runtime `workspace/STATE.md`; it is the product-level statement of what AetherVault should become.

## Objective

AetherVault should be an executive assistant first and an agent harness second.

Success means:
- It keeps durable track of priorities, open loops, waiting-fors, deadlines, contacts, and commitments.
- It can decompose work into as many workers as needed without hard-coded orchestration patterns.
- It is proactive across inbox, calendar, follow-up, briefing, and delegation workflows.
- It stays fast because context is structured, progressive, and state-driven instead of re-reading giant blobs.
- It remains auditable because the capsule is still the durable event log and memory substrate.

## Current Diagnosis

The repo still has product drift:
- `src/main.rs` is much smaller than before, but it still carries too much CLI dispatch, tool runtime, and scheduling logic.
- `scripts/` contains lifecycle automation that historically invented a second and third state model beside the core runtime.
- `services/embedding-service/` is useful infrastructure, but it should be treated as a replaceable service, not part of the assistant brain.
- the runtime seams are clearer now (`agent_runtime`, `agent_logs`, `executive_state`, `host_tools`, `policy`, `bridge_runtime`), but the tool/runtime surface still needs more extraction.

The major product risk has been fragmented truth:
- `MEMORY.md` held durable facts.
- session logs held recent interactions.
- the knowledge graph held entities.
- cron scripts inferred "active work" from the knowledge graph.
- the harness had no single durable executive state for open loops.

That fragmentation is what makes the assistant feel forgetful, slow, and ragtag.

## Target Architecture

### 1. State Plane

The assistant needs one explicit state plane with clear responsibilities:

- Capsule (`.mv2`): append-only event log, content store, retrieval substrate, approvals, reflections, skills, and search traces.
- Workspace files: human-readable working set for identity and planning.
- `workspace/STATE.json` + `workspace/STATE.md`: the live executive state for priorities, tasks, projects, follow-ups, waiting-fors, drafts, meetings, and closures.

Rules:
- `MEMORY.md` is for durable personal/org facts.
- `STATE` is for live commitments and strategic operating state.
- session logs are for replay and auditing, not the primary planning source.
- the knowledge graph enriches entities and relationships, but it does not define what is currently important.

### 2. Control Plane

The core agent loop should orchestrate:
- prompt assembly
- memory retrieval
- strategic state maintenance
- tool loading
- worker delegation
- approval routing
- recovery and reflection

This control plane should decide when to spawn zero, one, or many workers. Delegation policy must come from state, task shape, and config, not fixed names or fixed fan-out.

### 3. Execution Plane

Execution should be decomposed into explicit subsystems even if the binary stays monolithic for now:
- planner
- policy engine
- memory manager
- strategic state manager
- tool runtime
- bridge adapters
- scheduled jobs
- worker runtime

The code can remain one binary in the short term, but the seams need to be clear enough that each subsystem could be extracted later without changing behavior.

### 4. Interface Plane

All user-facing surfaces should share the same state and policy:
- chat bridges
- morning briefing
- evening check-in
- nightly consolidation
- trigger/watch flows
- future email/calendar automation

No script should invent its own model of "active projects" or "current tasks."

## Product Behaviors Required For An Excellent EA

The assistant should consistently do these well:
- convert ambiguous intent into explicit next actions
- maintain and close loops instead of merely discussing them
- surface deadlines and blockers proactively
- notice when a reply, reminder, or delegation is owed
- produce concise briefings from strategic state plus supporting context
- preserve continuity across sessions without bloating the prompt
- escalate for approval only where policy genuinely requires it

## Repository Direction

The repo should converge toward this shape:
- `src/`: core Rust assistant runtime
- `scripts/`: thin operational jobs that consume the same shared state contract
- `docs/`: a small set of canonical docs, with historical reports archived
- `services/embedding-service/`: retained only as optional infrastructure

If a subsystem does not clearly support the executive-assistant product, it should be split, archived, or demoted from the main path.

## Migration Priorities

### Phase 1: Stop State Fragmentation
- Keep `STATE` durable and synchronized.
- Feed `STATE` into prompt assembly, briefings, check-ins, and consolidation.
- Treat KG as enrichment, not task truth.

### Phase 2: Make Delegation Elastic
- Remove hard-coded worker assumptions.
- Let worker runtime inherit policy and budgets from config or per-task overrides.
- Add reviewer/adjudicator patterns only when the task warrants them.

### Phase 3: Separate Subsystems
- Extract policy, state, and bridge code paths into clearer internal modules.
- Keep the runtime behavior stable while reducing the single-file blast radius.

### Phase 4: Deepen EA Workflows
- inbox triage
- follow-up tracking
- meeting preparation
- outbound draft review
- reminder generation
- waiting-for nudges
- recurring executive summaries

## Definition Of Done

AetherVault is "excellent EA" grade when:
- the assistant always knows the current top priorities and open loops
- the scheduled jobs and interactive chat behavior agree on the same reality
- delegation is dynamic and configurable rather than scripted
- prompt growth is bounded by structured state and compaction
- the repo has one obvious product architecture rather than several competing ones
