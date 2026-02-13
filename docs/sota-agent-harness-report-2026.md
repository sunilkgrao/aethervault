# State-of-the-Art Agent Harness Architecture Report (February 2026)

## For AetherVault: Actionable Intelligence for Harness Evolution

---

## Executive Summary

The AI agent landscape in early 2026 has crystallized around the concept of the
**agent harness** -- the infrastructure wrapping a model that manages tools,
context, memory, lifecycle, and human-in-the-loop gates. The harness, not the
model, is the primary determinant of agent reliability. This report synthesizes
findings from Claude Code, OpenAI Codex, Anthropic Agent SDK, OpenAI Agent SDK,
Google ADK, Devin, Manus, LangGraph, CrewAI, and open-source frameworks into
actionable recommendations for AetherVault.

**Key takeaways:**

1. **Context engineering is the new moat.** KV-cache hit rate is the single most
   important production metric. Append-only context, stable prompt prefixes, and
   progressive disclosure are non-negotiable for cost and latency.

2. **Progressive disclosure has won.** Both Anthropic (Agent Skills) and Cursor
   implement tool/skill loading where only metadata is in the initial prompt, and
   full details load on demand. AetherVault's `tool_search` pattern is already
   aligned with this, but should be deepened.

3. **The harness is the product.** Claude Code v2.1.41 has ~110 prompt strings,
   40 system reminders, 18 builtin tools, and 3 sub-agent types. The system
   prompt is not a single string -- it is a living, modular document assembled at
   runtime from dozens of conditional components.

---

## 1. System Prompting Best Practices

### What the Leaders Do

**Claude Code** (v2.1.41, Feb 12 2026):
- Uses a modular prompt architecture with ~110 separate prompt strings
- The core identity prompt is only ~269 tokens
- 18 builtin tool descriptions range from 294 to 2,610 tokens each
- ~40 system reminders (12-1,500 tokens) inject real-time context: file
  modifications, token usage, plan mode status, hook execution results
- Sub-agent prompts (Explore: 516 tokens, Plan: 633 tokens, Task: 294 tokens)
  are separate from the main prompt
- Key behavioral instructions address: parallel tool calls, permission modes,
  git/PR workflows, and security review

**Cursor Agent**:
- Syncs MCP tool descriptions to a folder on disk
- Gives the agent a short list of available tools
- Agent reads full descriptions only if needed based on the task
- Implements progressive disclosure at the tool-description level

**OpenAI Codex**:
- Uses AGENTS.md files (analogous to CLAUDE.md) for per-project instructions
- Active plans, completed plans, and known technical debt are versioned and
  co-located in the filesystem
- Agents start with a small, stable entry point and are "taught where to look
  next"

**Devin 2.0**:
- Operates as a "junior coding partner" -- system prompt sets expectations for
  unreliable decision-making
- For complex tasks, the architecture and logic must be provided upfront
- Self-assessed confidence evaluation asks for clarification when uncertain

### Research Findings on Prompt Design

A recent study (Feb 10, 2026) demonstrated that **swapping system prompts between
agents running the same model produced dramatically different workflows**:
- Codex-prompted agents used documentation-first, methodical approaches
- Claude-prompted agents demonstrated iterative, test-and-fix strategies

This confirms: the system prompt defines the agent UX as much as the model
itself. Two functions are at play:
1. **Model Calibration**: Fighting against undesired training artifacts (e.g.,
   excessive comments, sequential tool calls when parallel would be better)
2. **UX Definition**: Shaping personality, autonomy level, and interaction style

### Common Cross-Agent Patterns

Nearly all SOTA agents include these prompt patterns:
- **Restrict unnecessary code comments** ("avoid adding comments unless the user asks")
- **Enforce parallel tool calls** ("CRITICAL: DEFAULT TO PARALLEL")
- **Set clear behavioral boundaries** for when to ask vs. act
- **Inject real-time context** via system reminders (not just a static prompt)
- **Progressive context loading** -- workspace files, memory, KG entities loaded
  conditionally

### AetherVault Assessment

AetherVault's current system prompt is a flat string assembled by concatenation:

```
core instructions (7 lines)
  + onboarding text (conditional)
  + workspace context (SOUL.md + USER.md + MEMORY.md)
  + global context
  + memory context (hybrid search results)
  + knowledge graph context
  + session conversation history
```

**Strengths:**
- Already does conditional context injection (onboarding, KG, memory)
- Has workspace context (SOUL.md / USER.md / MEMORY.md) similar to CLAUDE.md
- Dynamic tool loading via `tool_search` aligns with progressive disclosure

**Gaps vs. SOTA:**
- No system reminders during the agent loop (Claude Code injects ~40)
- Core personality prompt is very thin (7 lines vs. Claude Code's 269-token
  identity + modular attachments)
- No per-tool behavioral guidance in the system prompt
- Session history is crammed into the system prompt rather than structured as
  conversation turns
- No explicit instructions for error recovery patterns, verification steps, or
  self-correction behavior

### Recommendations

1. **Modularize the system prompt.** Break it into named sections that can be
   independently versioned: identity, behavioral constraints, tool usage policy,
   error recovery policy, workspace context, memory context, session context.

2. **Add system reminders.** Inject mid-loop reminders when context is getting
   long, when tools fail, or when the agent seems stuck. Claude Code does this
   with ~40 reminder types.

3. **Enrich the core identity.** The 7-line default prompt undersells the agent.
   Add explicit guidance on: when to use tools vs. respond directly, how to
   handle ambiguity, when to ask for clarification, how to structure multi-step
   reasoning.

4. **Move session history out of the system prompt.** Session turns should be
   proper `user`/`assistant` messages in the message array, not text crammed into
   the system prompt. This improves cache hit rates and model comprehension.

5. **Add model-specific calibration.** Different models have different failure
   modes. The system prompt should adapt based on which model is being used (e.g.,
   "Do not add code comments" for models that over-comment).

---

## 2. Acknowledgement / Streaming UX Patterns

### What the Leaders Do

**Claude Code**:
- The agentic loop is visible: users see "gather context -> take action ->
  verify results -> repeat"
- Users can interrupt at any point to steer the agent
- Shows tool usage in real-time (file reads, command execution)
- Compaction is automatic but transparent

**Telegram-Specific Patterns**:
- `sendMessageDraft` allows streaming partial text as "draft bubbles"
- Two modes: "partial" (frequent draft updates) and "block" (chunked updates)
- Typing indicators (`sendChatAction: typing`) for operations taking >2 seconds
- Message queuing: wait briefly for additional user messages before processing

**Smashing Magazine's 2026 UX Framework** defines three phases:

**Pre-Action:**
- Intent Preview (plan summary before execution)
- Autonomy Dial (user sets independence level per task type)

**In-Action:**
- Explainable Rationale ("Because you said X, I did Y")
- Confidence Signal (visual uncertainty indicators)
- Step Visibility: show intent, not inner thoughts ("Searching policy...",
  "Drafting response...", "Waiting for approval...")

**Post-Action:**
- Action Audit with Undo capability
- Escalation Pathway (acknowledge limits rather than guess)

**Key Metrics:**
- Target >85% plan acceptance rates
- Target <5% action reversions
- Target >0.8 calibration correlation between stated and actual confidence

### AetherVault Assessment

AetherVault's Telegram bridge currently uses `streamMode: "block"` with a
`sendChatAction: typing` indicator. This is functional but minimal.

**Gaps:**
- No plan preview before complex operations
- No streaming of partial responses during generation
- No visibility into which tool is being used or why
- No structured acknowledgement of receipt before processing begins
- No confidence signaling

### Recommendations

1. **Immediate acknowledgement.** When a message arrives, send a brief
   acknowledgement within 500ms: "Got it -- working on this now." This is
   separate from the typing indicator and gives the user confidence the message
   was received.

2. **Step-by-step status updates.** During multi-tool agent loops, send Telegram
   messages for each major phase: "Searching memory...", "Found 3 relevant notes",
   "Drafting response..." This maps to Claude Code's visible agentic loop.

3. **Implement streaming via draft edits.** Use Telegram's `editMessageText` to
   progressively update a single message as the response generates. Chunk at
   paragraph boundaries to avoid malformed markdown.

4. **Plan preview for complex tasks.** When the agent detects a multi-step task,
   send a brief plan ("I'll: 1) search your notes, 2) check your calendar, 3)
   draft a summary") before executing. Let the user approve, modify, or redirect.

5. **Error transparency.** When a tool fails, tell the user: "Calendar lookup
   failed -- retrying with a different approach" rather than silently recovering.

---

## 3. Agent Loop Architectures

### The 2026 Consensus

The standard agent loop in 2026 follows the **ReAct pattern** (Thought -> Action
-> Observation -> repeat), but with important production refinements:

**Claude Code's Loop:**
```
Gather Context -> Take Action -> Verify Results -> Iterate
```
Each phase blends into the next. The agent uses tools throughout. The loop adapts
to the task: a question might only need context gathering; a bug fix cycles
through all phases repeatedly.

**OpenAI Codex's Loop (Inner/Outer):**
```
User Input -> Structured Prompt (system + tools + context)
  -> LLM Inference (streaming)
    -> Tool Call Events: invoke tool, collect output, append to prompt
    -> Reasoning Events: plan steps
  -> Done Event: response to user
```
Key insight: the inner loop runs tool calls and reasoning, while the outer loop
manages conversation turns. Prompt caching makes this "linear instead of
quadratic" in cost.

**Anthropic's Two-Agent Pattern (Long-Running):**
```
Initializer Agent (first session):
  - Creates feature list (JSON, all "passes: false")
  - Creates progress log (claude-progress.txt)
  - Creates init script (init.sh)
  - Makes initial git commit

Coding Agent (all subsequent sessions):
  1. Read progress file
  2. Read feature list
  3. Check git log
  4. Run init.sh
  5. Pick ONE incomplete feature
  6. Implement + verify end-to-end
  7. Commit + update progress
  8. Update feature status
```

### Loop Control Best Practices

| Pattern | When to Use | Max Steps |
|---------|-------------|-----------|
| Simple ReAct | Single-step queries, quick lookups | 5-10 |
| Extended ReAct | Multi-tool tasks, research | 20-40 |
| Plan-then-Execute | Known complex workflows | 50-100 |
| Long-Running (Two-Agent) | Multi-session projects | 1 feature/session |

**Termination conditions (2026 consensus):**
- Max step limit (hard ceiling)
- Model emits no tool calls (natural completion)
- Confidence threshold exceeded
- All items in a checklist marked done
- Error count exceeds threshold (fail-safe)

**Error recovery patterns:**
- Retry with modified approach (not the same call)
- Store reflection before retry (Anthropic's recommendation)
- Revert to last known good state (git-based rollback)
- Escalate to user after N failures

### AetherVault Assessment

AetherVault implements a clean ReAct loop:

```rust
for _ in 0..effective_max_steps {
    let message = call_agent_hook(&model_spec, &request)?;
    // Record final text
    let tool_calls = message.tool_calls.clone();
    messages.push(message);
    if tool_calls.is_empty() {
        completed = true;
        break;
    }
    // Execute each tool call, push results
}
```

This is solid but lacks several SOTA features.

**Strengths:**
- Clean loop with configurable max_steps (default 64)
- Proper tool result handling with error propagation
- Dynamic tool activation via `tool_search`
- Buffered log flushing to avoid lock contention

**Gaps:**
- No plan-then-execute mode for complex tasks
- No inner/outer loop distinction
- No reflection/self-correction loop (tool exists but not integrated into loop)
- No mid-loop system reminders or context management
- No confidence-based termination
- No explicit error recovery strategy (errors become tool messages but no retry logic)
- No verification step after tool use

### Recommendations

1. **Add a planning phase.** Before entering the tool loop, optionally have the
   model generate a plan. This can be as simple as injecting "Before acting,
   outline your plan in 2-3 bullet points" into the prompt for complex tasks.

2. **Implement reflection on failure.** When a tool returns an error, inject a
   system reminder: "The previous tool call failed. Reflect on why and try a
   different approach. Use the `reflect` tool to record your analysis." This
   already exists as a tool but should be wired into the loop automatically.

3. **Add mid-loop context management.** Track token usage. When approaching
   context limits, trigger compaction: summarize earlier tool results, keep recent
   ones verbatim. This mirrors Claude Code's automatic compaction.

4. **Implement the inner/outer loop.** The inner loop handles tool calls within a
   single model turn. The outer loop manages conversation turns. This distinction
   enables better streaming and reduces unnecessary API calls.

5. **Add verification steps.** After completing a multi-step task, inject a
   verification prompt: "Review your work. Did you accomplish what was asked? Are
   there any issues?" This maps to Claude Code's "verify results" phase.

---

## 4. Memory and Context Management

### The 2026 Landscape

Memory has become a core pillar of production agents. The 2026 consensus
recognizes that **context windows (even 1M+ tokens) are working memory, not
storage.**

**Manus's Lessons (the most detailed production case study):**

The KV-cache hit rate is the SINGLE most important metric. With Claude Sonnet,
cached input costs $0.30/MTok vs. $3/MTok uncached -- a 10x difference. Manus's
average input-to-output ratio is 100:1, making cache optimization critical.

**Manus's Four Rules:**
1. **Keep your prompt prefix stable.** Even a single-token difference (like a
   timestamp!) invalidates the cache from that point onward.
2. **Make context append-only.** Never modify previous actions or observations.
   Ensure deterministic JSON serialization (key ordering matters).
3. **Mark cache breakpoints explicitly.** At minimum, ensure the breakpoint
   includes the end of the system prompt.
4. **Mask, don't remove tools.** Rather than removing tools from the schema,
   mask token logits during decoding. This preserves the cache prefix.

**Dual-Layer Memory Architecture (2026 pattern):**
- **Hot Path**: Immediate context window (working memory)
- **Cold Path**: Retrieval from vector stores / knowledge graphs (long-term)

**Leading Production Solutions:**
- Zep: Temporal Knowledge Graphs (critical for accuracy in complex reasoning)
- Mem0: User preference memory for personalization
- PostgresSaver: Production standard for LangGraph checkpointing
- Anthropic Agent Skills: Progressive disclosure from filesystem

**Anthropic's Progressive Disclosure for Memory:**
Three levels of information loading:
1. Metadata Level: Just enough for the agent to know when each skill is relevant
2. Instructions Level: Full SKILL.md loads when the skill is activated
3. Resources Level: Additional files load only as needed

Key insight: "The amount of context that can be bundled into a skill is
effectively unbounded. There's no context penalty for bundled content that isn't
used."

**Claude Code's Memory Model:**
- CLAUDE.md for persistent per-project instructions
- Auto-memory for learning across sessions
- Skills for on-demand domain expertise
- Sub-agents for isolated context windows
- Automatic compaction when context fills up
- `/compact` with focus for manual compaction

### AetherVault Assessment

AetherVault has a sophisticated memory architecture:

```
SOUL.md (personality/behavior)
USER.md (user profile)
MEMORY.md (long-term notes)
Daily memory files (rolling logs)
Knowledge Graph (auto-injected entities)
Capsule-based hybrid search (BM25 + vector)
Session turns (last 8, persisted to disk)
```

**Strengths:**
- Hybrid search (BM25 + vector) for memory retrieval
- Knowledge graph auto-injection
- Temporal filtering (before/after/asof)
- Capsule-based audit trail
- Feedback loop for ranking improvement

**Gaps:**
- All memory context goes into the system prompt, hurting cache hit rates
- No progressive disclosure -- everything loads at once
- Session history in system prompt rather than message turns
- No compaction during long agent runs
- No explicit cache breakpoint management
- Context assembled by string concatenation (fragile for cache stability)
- Knowledge graph loaded from a fixed path (`/root/.aethervault/data/...`)

### Recommendations

1. **Stabilize the system prompt prefix.** Move all dynamic content (memory
   search results, KG entities, session history) AFTER a stable prefix. Never
   include timestamps or non-deterministic content before the cache breakpoint.

2. **Implement progressive disclosure for memory.** Instead of injecting all
   memory search results into the system prompt, provide a `memory_search` tool
   that the agent can call on demand. Only inject the most critical context
   (SOUL.md, USER.md) into the system prompt.

3. **Move session history to message turns.** Instead of cramming history into
   the system prompt, inject it as proper `user`/`assistant` message pairs.
   This improves model comprehension and cache hit rates.

4. **Add mid-session compaction.** Track approximate token usage. When approaching
   a configurable threshold (e.g., 80% of context window), summarize earlier
   messages and replace them with a compact summary. Preserve the system prompt
   and recent messages verbatim.

5. **Implement Manus's append-only rule.** Ensure the messages array is never
   mutated -- only appended to. When compaction is needed, insert a summary
   message rather than modifying existing messages.

6. **Add explicit cache breakpoints.** When calling Anthropic's API, use
   `cache_control` markers at the end of the system prompt and after tool
   definitions to maximize KV-cache reuse.

---

## 5. Tool Orchestration

### What the Leaders Do

**Claude Code's Tool Architecture:**
- 18 builtin tools across 4 categories (File, Search, Execution, Web)
- Each tool description is between 294 and 2,610 tokens
- Tools are "prominent in Claude's context window" -- they ARE the primary
  actions the model considers
- Tool results split into: output (LLM-facing text) and details (structured
  JSON for UI/workflows)

**Progressive Tool Discovery (the 2026 winner):**
- Static toolsets: 405K tokens for 400 tools (context explosion)
- Progressive search: 1,600-2,500 tokens regardless of toolset size
- The "meta-tool pattern": register only a discovery tool and an execution tool;
  full schemas load on demand

**OpenAI Agent SDK's Primitives:**
- Agents (LLMs + instructions + tools)
- Handoffs (agents delegate to other agents)
- Guardrails (validation of inputs/outputs)
- Built-in tracing for debugging and monitoring

**Google ADK's Event-Driven Architecture:**
- The Runner coordinates sessions and agent activities
- Every action is logged as a permanent Event
- Session Services manage memory and state
- Agent types: LLM Agents, Sequential/Parallel/Loop workflow agents

**Approval Gates (2026 consensus):**
- Human approval for any output going to external systems
- Progressive autonomy: observe -> suggest -> act with confirmation -> act
  autonomously
- Approval gates are "quality control points, not bottlenecks"
- Sandbox execution for tool testing before production

**Parallel Tool Execution:**
- Multiple agents/tools work simultaneously on different aspects
- Results collected and merged at the end
- Requires careful handling of partial failures
- Claude Code's system prompt: "CRITICAL: DEFAULT TO PARALLEL"

### AetherVault Assessment

AetherVault has a rich tool surface:

```
Core: put, search, query, context, log, feedback
Filesystem: fs_list, fs_read, fs_write
External: gmail_*, gcal_*, ms_*, http_request, browser_request
Agent: tool_search, session_context, reflect, skill_store, skill_search
Orchestration: subagent_list, subagent_invoke, compact, doctor
```

**Strengths:**
- Dynamic tool loading via `tool_search` (progressive disclosure pattern)
- Tool results split into output + details (matches Claude Code pattern)
- Approval gates for sensitive tools
- Subagent orchestration for parallel work
- Tool activation tracking (tools added dynamically from search results)

**Gaps:**
- No parallel tool execution within a single turn (sequential only)
- No tool-level tracing or observability
- No guardrails/validation on tool inputs or outputs
- Tool definitions loaded as a static catalog, not progressively described
- No timeout management per-tool (uses global hook timeout)
- No tool result truncation for large outputs

### Recommendations

1. **Implement parallel tool execution.** When the model requests multiple tool
   calls in a single turn, execute them concurrently (Rust's thread model is
   ideal for this). Collect results and report them together.

2. **Add tool-level tracing.** Log tool name, arguments, duration, output size,
   and success/failure for every call. This enables debugging and cost analysis.

3. **Implement output truncation.** Large tool outputs (filesystem reads, search
   results) should be truncated to a configurable max size with a message like
   "[truncated, {N} bytes omitted -- use a more specific query]".

4. **Add per-tool timeouts.** Different tools have different latency profiles.
   An HTTP request should timeout at 30s; a subagent invocation at 5 minutes.

5. **Implement Manus's tool masking.** Instead of removing tools from the schema
   between turns, keep them but mask their logits. This preserves the cache
   prefix. For AetherVault's hook-based architecture, this means keeping the
   `tools` array stable and using system reminders to guide tool selection.

6. **Add guardrails.** Validate tool inputs (e.g., fs_write paths must be within
   allowed roots) and outputs (e.g., HTTP responses must be valid JSON) before
   passing results back to the model.

---

## 6. Open-Source Harness Comparisons

### Framework Comparison Matrix

| Framework | Architecture | Best For | Production Ready | Key Pattern |
|-----------|-------------|----------|-----------------|-------------|
| **LangGraph** | Graph-based workflow | Complex orchestration with branching, conditional logic | Yes (most battle-tested) | State machines, checkpointing |
| **CrewAI** | Role-based agents | Multi-agent collaboration, role-play | Yes (enterprise features) | Crews of specialists |
| **AutoGen** | Conversation-driven | Rapid prototyping, human-in-the-loop | Moderate | Dynamic role-playing |
| **Anthropic Agent SDK** | Two-agent + skills | Long-running tasks, progressive disclosure | Yes | Initializer/Coder split |
| **OpenAI Agent SDK** | Primitives-based | Multi-agent handoffs, tracing | Yes | Agents + Handoffs + Guardrails |
| **Google ADK** | Event-driven runtime | Workflow automation, sequential/parallel pipelines | Yes | Runner + Events + Sessions |

### What Each Does Best

**LangGraph** (Industry leader for production):
- Graph-based workflow design with nodes and edges
- Exceptional flexibility for conditional logic and parallel processing
- Persistent workflows with checkpointing (PostgresSaver)
- Steepest learning curve but most powerful

**CrewAI** (Best for role-based collaboration):
- Intuitive agent-as-employee model
- Built-in observability and enterprise control plane
- Beginner-friendly
- Strong for "researcher + writer" style workflows

**AutoGen** (Best for rapid prototyping):
- Flexible conversation-driven architecture
- Natural language interaction is paramount
- Less strict output consistency (flexibility vs. reliability tradeoff)
- Strong human-in-the-loop support

**Anthropic Agent SDK**:
- Progressive disclosure via Agent Skills
- Two-agent pattern for long-running tasks
- Sub-agents for context isolation
- Automatic compaction
- Skills are an open standard adopted by Microsoft, OpenAI, Atlassian, Figma

**OpenAI Agent SDK**:
- Python-first design (native loops, conditionals)
- Built-in tracing and observability
- AgentKit for visual workflow building
- Handoff pattern for agent delegation

**Google ADK**:
- Event-driven runtime (every action is a permanent Event)
- Model-agnostic, deployment-agnostic
- CLI and Developer UI for debugging
- Workflow agents (Sequential, Parallel, Loop)

### Emerging Consensus Patterns

These patterns appear across all successful frameworks:

1. **The harness is separate from the model.** It manages lifecycle, tools, context.
2. **Progressive disclosure.** Load information on demand, not upfront.
3. **Filesystem as context store.** Use files for state management, not just memory.
4. **Sub-agents for isolation.** Fresh context windows prevent bloat.
5. **Append-only state.** Never mutate; always append. Enables audit and rollback.
6. **Cache-first design.** KV-cache hit rate drives cost and latency.
7. **Human-in-the-loop is a spectrum.** From "observe" to "act autonomously."

### AetherVault's Position

AetherVault is architecturally most similar to the **Anthropic Agent SDK pattern**:
a Rust binary that serves as the harness, with tool execution, memory management,
and sub-agent orchestration. Key differentiators:

- Single-file capsule architecture (unique -- no other framework does this)
- Hybrid retrieval with feedback loops (more sophisticated than any framework)
- Time-travel queries (unique capability)
- Hook-based model abstraction (model-agnostic like Google ADK)

**What to adopt from each framework:**

From **LangGraph**: State machine concepts for complex multi-step workflows.
Add a `workflow` tool type that defines explicit state transitions.

From **CrewAI**: Role-based agent specialization. AetherVault's subagents could
be given explicit roles (researcher, writer, reviewer) with role-specific prompts.

From **Anthropic Agent SDK**: Progressive disclosure of skills (already partially
implemented). Deepen the SKILL.md pattern. Adopt the two-agent pattern for
long-running tasks.

From **OpenAI Agent SDK**: Built-in tracing and observability. Add structured
trace logging for every agent turn, tool call, and decision point.

From **Google ADK**: Event-driven architecture. AetherVault's capsule frames are
already events -- expose them as a real-time event stream for UI consumption.

---

## 7. Prioritized Recommendations for AetherVault

### Tier 1: High Impact, Immediate (1-2 sprints)

**R1. Stabilize the system prompt prefix for cache optimization.**
Move all dynamic content after a stable prefix. Add explicit `cache_control`
markers at the breakpoint. This alone can reduce API costs by up to 10x for
Anthropic models.

Implementation:
```
[STABLE PREFIX - never changes between turns]
  Core identity (fixed)
  Tool definitions (stable within session)
  SOUL.md content (stable within session)
  USER.md content (stable within session)
  --- cache breakpoint ---
[DYNAMIC SUFFIX - changes each turn]
  Memory search results
  Knowledge graph context
  Session history
  System reminders
```

**R2. Move session history out of the system prompt.**
Inject previous turns as proper `user`/`assistant` messages in the messages
array, between the system message and the current user message. Remove the
"Recent Conversation History" section from the system prompt entirely.

**R3. Add immediate acknowledgement in Telegram bridge.**
Send a brief "Got it" or reaction emoji within 500ms of receiving a message.
Follow with typing indicator during processing. Send step-by-step status
updates during multi-tool operations.

**R4. Implement tool result truncation.**
Cap tool outputs at a configurable max (e.g., 4,000 chars). Append a truncation
notice. This prevents a single large tool result from consuming the entire
context window.

### Tier 2: Significant Impact, Medium Effort (2-4 sprints)

**R5. Add mid-loop system reminders.**
Inject context-aware reminders during the agent loop:
- When token usage exceeds 60%: "Context is filling up. Be concise."
- When a tool fails: "Previous tool call failed. Reflect and try differently."
- When steps exceed 50% of max: "You've used {N} of {M} steps. Focus on completing the task."

**R6. Implement mid-session compaction.**
When approximate token count exceeds a threshold, summarize older messages:
1. Count tokens (approximate: chars / 4)
2. If > 80% of context window, summarize all messages except last 4
3. Replace summarized messages with a single summary message
4. Keep system prompt and recent messages verbatim

**R7. Implement parallel tool execution.**
When the model returns multiple tool_calls, execute them concurrently using
Rust threads. Collect results and return them in order.

**R8. Add structured tracing.**
For every agent turn, log: turn number, model used, input token count, output
token count, tool calls (name, duration, success), total latency. Store in the
capsule as a trace frame.

### Tier 3: Strategic, Longer-Term (4-8 sprints)

**R9. Implement the two-agent pattern for complex tasks.**
Add a `plan` mode where the agent generates a structured plan before executing.
For truly long-running tasks, implement the initializer/coder split: first
session creates a plan + progress file, subsequent sessions pick up where the
last left off.

**R10. Deepen progressive disclosure.**
Extend `tool_search` into a full progressive disclosure system:
- Tools start with name + 1-line description in context
- `tool_search` returns full schema only for matched tools
- Skills load metadata first, full instructions on activation
- Memory loads summaries first, full documents on request

**R11. Add an autonomy dial.**
Let users configure per-tool autonomy levels:
- Level 0: Suggest only (plan mode)
- Level 1: Act with confirmation (current approval gate behavior)
- Level 2: Act autonomously (trusted tools like search, read)
- Level 3: Act and report (background tasks, briefings)

**R12. Implement event streaming.**
Expose the agent loop as a Server-Sent Events (SSE) stream:
```json
{"event": "tool_call", "data": {"name": "search", "query": "..."}}
{"event": "tool_result", "data": {"name": "search", "count": 3}}
{"event": "thinking", "data": {"summary": "Found relevant notes, drafting..."}}
{"event": "text", "data": {"content": "Here's what I found..."}}
```
This enables rich UIs beyond the Telegram bridge.

---

## 8. Architecture Diagram: Target State

```
User (Telegram/WhatsApp/CLI/HTTP)
  |
  v
[Bridge Layer]
  - Immediate acknowledgement (500ms)
  - Message queuing and debouncing
  - Streaming via draft edits
  |
  v
[Harness Layer]
  - System prompt assembly (modular, cache-optimized)
  - Progressive tool disclosure
  - Autonomy level enforcement
  - Token tracking and compaction triggers
  |
  v
[Agent Loop]
  - Optional planning phase
  - ReAct loop with parallel tool execution
  - Mid-loop system reminders
  - Reflection on failure
  - Verification before completion
  - Structured tracing
  |
  v
[Tool Layer]
  - Dynamic discovery via tool_search
  - Per-tool timeouts and truncation
  - Approval gates for sensitive ops
  - Parallel execution where possible
  |
  v
[Memory Layer]
  - Hot path: system prompt context (SOUL/USER, critical memories)
  - Warm path: on-demand search (memory_search tool)
  - Cold path: capsule archive (time-travel, audit)
  - Knowledge graph: auto-injected entities
  - Session state: proper message turns
  |
  v
[Capsule (.mv2)]
  - Append-only frames
  - Hybrid index (BM25 + vector)
  - Trace frames (observability)
  - Feedback frames (self-improvement)
```

---

## 9. Key Metrics to Track

Based on industry best practices:

| Metric | Target | Why |
|--------|--------|-----|
| KV-cache hit rate | >80% | 10x cost reduction |
| Time to first acknowledgement | <500ms | User trust |
| Tool call success rate | >95% | Reliability |
| Average turns to completion | <8 for simple, <20 for complex | Efficiency |
| Compaction frequency | <1 per 10 turns | Context health |
| Plan acceptance rate | >85% | Prompt quality |
| Action reversion rate | <5% | Trust calibration |

---

## Sources

- Anthropic, "Effective harnesses for long-running agents" (2026)
- Anthropic, "Building agents with the Claude Agent SDK" (2026)
- Anthropic, "Equipping agents for the real world with Agent Skills" (2025)
- Drew Breunig, "System Prompts Define the Agent as Much as the Model" (Feb 10, 2026)
- Claude Code documentation, "How Claude Code works" (2026)
- Piebald-AI, "Claude Code System Prompts" repository (updated Feb 12, 2026)
- Manus, "Context Engineering for AI Agents: Lessons from Building Manus" (2025)
- OpenAI, "Unrolling the Codex Agent Loop" (2026)
- OpenAI, "Harness Engineering: Leveraging Codex in an Agent-First World" (2026)
- Philipp Schmid, "The Importance of Agent Harness in 2026" (2026)
- Lance Martin, "Agent Design Patterns" (Jan 9, 2026)
- Smashing Magazine, "Designing for Agentic AI: Practical UX Patterns" (Feb 2026)
- Google, "Agent Development Kit" documentation (2026)
- DataCamp, "CrewAI vs LangGraph vs AutoGen" comparison (2026)
- Speakeasy, "100x Token Reduction: Dynamic Toolsets" (2026)
- Synaptic Labs, "The Meta-Tool Pattern: Progressive Disclosure for MCP" (2026)
