# AetherVault Architecture

## Thesis

The capsule is the durable substrate, but the product is not just a memory file. AetherVault is an agent runtime with one shared operating model for interactive chat, scheduled executive-assistant jobs, and delegated worker execution.

## Core planes

### State plane

- `.mv2` capsule: append-only content, logs, approvals, skills, reflections, and retrieval traces.
- Workspace files: `SOUL.md`, `USER.md`, `MEMORY.md`, and `STATE.{md,json}`.
- `STATE` is the live executive state for priorities, commitments, waiting-fors, drafts, follow-ups, and closures.
- The knowledge graph enriches entities and relationships; it does not define task truth.

### Control plane

The runtime decides:

- prompt assembly
- memory retrieval
- tool exposure
- approval routing
- worker delegation
- failure recovery and reflection

Delegation is elastic. The main loop can choose zero, one, or many workers based on task shape and policy.

### Execution plane

The binary is still large, but the internal seams are explicit:

- `agent_runtime.rs`: loop orchestration, prompt guidance, compaction, session continuity
- `agent_logs.rs`: durable session logging and log export
- `executive_state.rs`: durable executive-state model and rendering
- `workspace_state.rs`: workspace bootstrap and capsule/workspace sync
- `executive_tools.rs`: EA-focused tool handlers
- `host_tools.rs`: host I/O, filesystem, browser, webhook, and local status tools
- `policy.rs`: approval and filesystem guardrails
- `bridge_runtime.rs`: chat connector adapters
- `tool_registry.rs`: tool schema surface

## Retrieval model

Each query builds a plan:

1. Parse inline constraints and scope.
2. Expand when useful.
3. Retrieve across lexical and optional vector lanes.
4. Fuse and rerank.
5. Return human text, JSON, file lists, or a context pack.

This keeps the agent fast by loading context progressively instead of re-injecting whole files.

## Agent contract

The runtime exposes:

- context packs
- logs
- feedback
- MCP server compatibility
- an agent loop with tools, approvals, and elastic worker orchestration

Tool results are split into:

- `output`: concise LLM-facing text
- `details`: structured JSON for workflows and downstream automation

## EA contract

Interactive chat and scheduled jobs should agree on the same reality:

- morning briefing reads `STATE` and supporting context
- evening check-in reads `STATE` and unclosed loops
- nightly consolidation updates `MEMORY`, `STATE`, and the knowledge graph
- chat bridges use the same policies and state model as scheduled jobs

If a subsystem cannot speak that shared contract, it should be removed or rewritten.
