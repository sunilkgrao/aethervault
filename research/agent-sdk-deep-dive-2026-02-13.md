# Agent SDK & Framework Deep Dive -- Research Report
## Date: 2026-02-13

---

## TABLE OF CONTENTS

1. [Anthropic Agent SDK Architecture](#1-anthropic-agent-sdk-architecture)
2. [System Prompt Patterns](#2-system-prompt-patterns)
3. [Streaming and UX](#3-streaming-and-ux)
4. [Memory Patterns](#4-memory-patterns)
5. [Multi-Agent Patterns](#5-multi-agent-patterns)
6. [Comparison: Anthropic vs OpenAI Agent SDK](#6-comparison-anthropic-vs-openai-agent-sdk)
7. [Comparison: Google ADK](#7-comparison-google-adk)
8. [LangGraph Architecture Patterns](#8-langgraph-architecture-patterns)
9. [Agent Protocols & Standards (A2A, MCP)](#9-agent-protocols--standards-a2a-mcp)
10. [Actionable Takeaways](#10-actionable-takeaways)

---

## 1. ANTHROPIC AGENT SDK ARCHITECTURE

### Overview

The Claude Agent SDK (renamed from "Claude Code SDK" in 2025) packages the same agent harness
that powers Claude Code into a programmable library available in Python and TypeScript. It is
NOT a lightweight orchestration framework like OpenAI's Agents SDK or LangGraph -- it is a
**full runtime environment** that gives your agent a sandboxed computer with file I/O, bash,
web access, and MCP integrations out of the box.

**Repos:**
- Python: https://github.com/anthropics/claude-agent-sdk-python
- TypeScript: https://github.com/anthropics/claude-agent-sdk-typescript
- Demos: https://github.com/anthropics/claude-agent-sdk-demos

### The Agent Loop (ReAct Cycle)

The SDK implements a **four-phase autonomous loop**:

```
Gather Context --> Take Action --> Verify Work --> Iterate
```

1. **Gather Context**: Read files, grep codebases, invoke subagents, call MCP tools
2. **Take Action**: Execute bash commands, write/edit files, call APIs, generate code
3. **Verify Work**: Run linters, check outputs against rules, use LLM judges, take screenshots
4. **Iterate**: Continue until task is complete or max_turns reached

This is fundamentally different from a simple ReAct loop. The SDK manages the ENTIRE loop
internally -- you do NOT implement tool execution yourself. Compare:

```python
# Client SDK (manual loop):
response = client.messages.create(...)
while response.stop_reason == "tool_use":
    result = your_tool_executor(response.tool_use)
    response = client.messages.create(tool_result=result, **params)

# Agent SDK (autonomous):
async for message in query(prompt="Fix the bug in auth.py"):
    print(message)  # Claude reads, diagnoses, edits, verifies -- all autonomously
```

### Tool System

The SDK provides **built-in tools** that work without any implementation:

| Tool           | Purpose                                    |
|----------------|--------------------------------------------|
| Read           | Read any file in working directory          |
| Write          | Create new files                            |
| Edit           | Precise edits to existing files             |
| Bash           | Run terminal commands, scripts, git         |
| Glob           | Find files by pattern                       |
| Grep           | Search file contents with regex             |
| WebSearch      | Search the web                              |
| WebFetch       | Fetch and parse web pages                   |
| Task           | Spawn subagents                             |
| TodoWrite      | Maintain task lists                         |
| NotebookEdit   | Edit Jupyter notebooks                      |
| AskUserQuestion| Ask user clarifying questions               |

**Custom tools** are defined via MCP servers using the `@tool` decorator:

```python
from claude_agent_sdk import tool, create_sdk_mcp_server

@tool("greet", "Greet a user", {"name": str})
async def greet_user(args):
    return {"content": [{"type": "text", "text": f"Hello, {args['name']}!"}]}

server = create_sdk_mcp_server(name="my-tools", version="1.0.0", tools=[greet_user])

options = ClaudeAgentOptions(
    mcp_servers={"tools": server},
    allowed_tools=["mcp__tools__greet"]
)
```

### Advanced Tool Use (Beta Features)

Three beta features for sophisticated tool orchestration:

1. **Tool Search Tool**: Dynamic discovery instead of loading all definitions upfront.
   Tools marked `defer_loading: true` stay hidden until Claude searches for them.
   Reduced tokens from ~77K to ~8.7K (85% reduction), accuracy from 79.5% to 88.1%.

2. **Programmatic Tool Calling**: Claude writes Python code that orchestrates multiple
   tools in a sandboxed execution environment. 37% token reduction on complex tasks.

3. **Tool Use Examples**: Concrete usage patterns showing parameter combinations.
   Accuracy improved from 72% to 90% on complex parameter handling.

Enable via: `betas=["advanced-tool-use-2025-11-20"]`

### Two API Entry Points

| Feature             | `query()`                     | `ClaudeSDKClient`                  |
|---------------------|-------------------------------|------------------------------------|
| Session             | Creates new session each time | Reuses same session                |
| Conversation        | Single exchange               | Multiple exchanges in same context |
| Streaming Input     | Supported                     | Supported                          |
| Interrupts          | Not supported                 | Supported                          |
| Hooks               | Not supported                 | Supported                          |
| Custom Tools        | Not supported                 | Supported                          |
| Continue Chat       | New session each time         | Maintains conversation             |

### Hooks System

Hooks allow intercepting the agent loop at specific lifecycle points:

- `PreToolUse` -- Before tool execution (can block/modify)
- `PostToolUse` -- After tool execution
- `UserPromptSubmit` -- When user submits prompt
- `Stop` -- When agent finishes
- `SubagentStop` -- When a subagent completes
- `PreCompact` -- Before context compaction
- `SessionStart` / `SessionEnd` -- Session lifecycle

```python
async def validate_bash(input_data, tool_use_id, context):
    command = input_data["tool_input"].get("command", "")
    if "rm -rf /" in command:
        return {
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "Dangerous command blocked"
            }
        }
    return {}

options = ClaudeAgentOptions(
    hooks={
        "PreToolUse": [HookMatcher(matcher="Bash", hooks=[validate_bash])]
    }
)
```

---

## 2. SYSTEM PROMPT PATTERNS

### Default System Prompt

The SDK supports three system prompt modes:

1. **Custom string**: Full control over system prompt
2. **Preset**: Use Claude Code's built-in system prompt
3. **Preset + append**: Extend the default prompt

```python
# Custom prompt
options = ClaudeAgentOptions(system_prompt="You are an expert Python developer")

# Claude Code preset
options = ClaudeAgentOptions(
    system_prompt={"type": "preset", "preset": "claude_code"}
)

# Preset + custom additions
options = ClaudeAgentOptions(
    system_prompt={
        "type": "preset",
        "preset": "claude_code",
        "append": "Always write tests for any code you create."
    }
)
```

### CLAUDE.md Memory Files

Project-level instructions are loaded from `CLAUDE.md` files (requires
`setting_sources=["project"]`). These act as persistent system prompt extensions:

- `.claude/CLAUDE.md` -- Project-specific instructions
- `~/.claude/CLAUDE.md` -- User-level global instructions

### Anthropic's Prompting Philosophy for Agents

Key principles from Anthropic's engineering team:

1. **Heuristics over examples**: Give agents principles for decision-making rather than
   rigid examples. "Give agents the heuristics and principles they need to make good
   decisions independently."

2. **Self-contained tool descriptions**: Tools should be independent, non-overlapping,
   and purpose-specific with explicit parameters and concise descriptions.

3. **Single-job subagents**: Each subagent should have one clear job. The orchestrator
   handles global planning, delegation, and state.

4. **Deny-all permissions**: Start from deny-all and allowlist only what's needed.

### Skills Framework

Skills are folders of instructions, scripts, and resources that Claude can dynamically
discover and load -- "professional knowledge packs" stored in `.claude/skills/`:

```yaml
# .claude/agents/api-developer.md
---
name: api-developer
description: Implement API endpoints following team conventions
skills:
  - api-conventions
  - error-handling-patterns
---
Implement API endpoints. Follow the conventions from the preloaded skills.
```

---

## 3. STREAMING AND UX

### Streaming Architecture

The SDK streams responses via **server-sent events (SSE)**. Both `query()` and
`ClaudeSDKClient` return async iterators that yield messages as they arrive:

```python
async for message in query(prompt="Fix the bug"):
    if isinstance(message, AssistantMessage):
        for block in message.content:
            if isinstance(block, TextBlock):
                print(block.text, end="")  # Incremental text output
            elif isinstance(block, ToolUseBlock):
                print(f"Using tool: {block.name}")
            elif isinstance(block, ToolResultBlock):
                print(f"Tool completed")
    elif isinstance(message, ResultMessage):
        print(f"Done! Cost: ${message.total_cost_usd}")
```

### Partial/Incremental Output During Tool Use

Enable `include_partial_messages=True` to receive `StreamEvent` messages with raw
Anthropic API stream events:

```python
options = ClaudeAgentOptions(include_partial_messages=True)

async for message in query(prompt="Analyze this", options=options):
    if isinstance(message, StreamEvent):
        # Raw Anthropic API stream event -- token-by-token updates
        print(message.event)
```

StreamEvent includes `parent_tool_use_id` to track which subagent produced the event.

### Message Types in the Stream

- `UserMessage` -- User input
- `AssistantMessage` -- Claude response (contains TextBlock, ToolUseBlock, etc.)
- `SystemMessage` -- System metadata (init, compaction, etc.)
- `ResultMessage` -- Final result with cost, usage, session_id
- `StreamEvent` -- Partial updates (when enabled)

### Interrupt Support

ClaudeSDKClient supports interrupting long-running operations:

```python
async with ClaudeSDKClient(options=options) as client:
    await client.query("Count from 1 to 100 slowly")
    await asyncio.sleep(2)
    await client.interrupt()  # Stop current task
    await client.query("Just say hello instead")  # Continue in same session
```

---

## 4. MEMORY PATTERNS

### In-Session Context Management

The SDK manages conversation context internally with **automatic context compaction**.
When approaching token limits (~95% capacity by default), it summarizes older turns to
preserve intent, decisions, and open threads.

Configurable via `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` environment variable.

Compaction strategy:
- Keep raw tool outputs out of history
- Keep short, precise summaries in
- Preserve design choices, file paths, and constraints
- Summarize conversation segments into bullet points with timestamps

### Session Persistence and Resumption

Sessions can be resumed across multiple queries:

```python
# First query: capture session_id
session_id = None
async for message in query(prompt="Read the auth module"):
    if hasattr(message, "subtype") and message.subtype == "init":
        session_id = message.session_id

# Resume with full context
async for message in query(
    prompt="Now find all places that call it",
    options=ClaudeAgentOptions(resume=session_id)
):
    print(message)
```

Sessions can also be **forked** (`fork_session=True`) to explore different approaches.

### Long-Running Agent Architecture (Multi-Session)

For tasks spanning hours or days, Anthropic developed a two-part solution:

1. **Initializer Agent**: First session sets up infrastructure
   - Creates `init.sh` for environment setup
   - Creates `claude-progress.txt` for cross-session state
   - Makes initial git commit as baseline

2. **Coding Agent**: Subsequent sessions make incremental progress
   - Reads git logs and progress files for context
   - Selects highest-priority incomplete features
   - Runs e2e tests before implementing new features
   - Leaves structured artifacts for next session

### CLAUDE.md File-Based Memory

The primary long-term memory mechanism is Markdown files:
- `CLAUDE.md` at project root -- project conventions, architecture
- `.claude/CLAUDE.md` -- more detailed project instructions
- `~/.claude/CLAUDE.md` -- user-level preferences and patterns

### Subagent Persistent Memory

Subagents can maintain their own memory directories across sessions:

```yaml
---
name: code-reviewer
description: Reviews code for quality and best practices
memory: user  # Options: user, project, local
---
```

Memory scopes:
- `user`: `~/.claude/agent-memory/<agent-name>/` -- across all projects
- `project`: `.claude/agent-memory/<agent-name>/` -- project-specific, version-controlled
- `local`: `.claude/agent-memory-local/<agent-name>/` -- project-specific, gitignored

When enabled, the subagent gets MEMORY.md injected into its system prompt (first 200 lines)
and Read/Write/Edit tools for managing memory files.

### Anthropic Memory Tool (Beta, Sep 2025)

Anthropic's memory tool uses a file-based approach (not vector DB):
- Memory stored in Markdown files named CLAUDE.md
- Organized in hierarchical structure
- Simple, transparent, and human-readable

---

## 5. MULTI-AGENT PATTERNS

### Subagent Architecture

Subagents are specialized agents running in isolated context windows:

```python
options = ClaudeAgentOptions(
    allowed_tools=["Read", "Glob", "Grep", "Task"],
    agents={
        "code-reviewer": AgentDefinition(
            description="Expert code reviewer for quality and security reviews.",
            prompt="Analyze code quality and suggest improvements.",
            tools=["Read", "Glob", "Grep"],
            model="sonnet"
        )
    }
)
```

Key properties:
- Each subagent gets its own context window
- Custom system prompt (NOT the full Claude Code prompt)
- Specific tool access (allowlist)
- Independent permissions
- Model selection (sonnet, opus, haiku, inherit)
- Subagents CANNOT spawn other subagents (no nesting)

### Built-in Subagents

| Agent      | Model   | Purpose                                      |
|------------|---------|----------------------------------------------|
| Explore    | Haiku   | Fast, read-only codebase exploration          |
| Plan       | Inherit | Research for plan mode                        |
| General    | Inherit | Complex multi-step tasks                      |
| Bash       | Inherit | Terminal commands in separate context          |

### Subagent Definition Format (Markdown with YAML Frontmatter)

```markdown
---
name: code-reviewer
description: Expert code review specialist
tools: Read, Grep, Glob, Bash
model: sonnet
permissionMode: dontAsk
maxTurns: 20
memory: user
skills:
  - api-conventions
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: command
          command: "./scripts/validate-command.sh"
---

You are a senior code reviewer. Focus on quality, security, and best practices.
```

### Foreground vs Background Subagents

- **Foreground**: Blocks main conversation, permission prompts pass through
- **Background**: Runs concurrently, pre-approved permissions, auto-denies unapproved.
  MCP tools NOT available in background subagents.

### Orchestrator-Worker Pattern (Multi-Agent Research System)

Anthropic's production multi-agent system uses:

1. Lead agent (Opus) coordinates 3-5 subagents (Sonnet) in parallel
2. Two parallelization levels: agent-level AND tool-level
3. Reduced research time by up to 90% for complex queries
4. Lead Opus + Sonnet subagents outperformed single-agent Opus by 90.2%
5. Trade-off: ~15x more tokens than single-agent chats

### Agent Teams (vs Subagents)

For sustained parallelism beyond a single session:
- Agent Teams have separate, independent sessions
- Each team member has its own context window
- Orchestrator coordinates via `delegate` permission mode
- Use when: tasks exceed context window, need true independence

### Pipeline Patterns

```
# Sequential Pipeline (deterministic)
analyst --> architect --> implementer --> tester --> security audit

# Parallel Specialization (when dependencies are low)
[UI agent, API agent, DB agent] --> orchestrator synthesizes
```

### Controlling Subagent Spawning

```yaml
---
name: coordinator
tools: Task(worker, researcher), Read, Bash  # Only these subagent types allowed
---
```

---

## 6. COMPARISON: ANTHROPIC vs OPENAI AGENT SDK

### Architecture Philosophy

| Dimension              | Anthropic Agent SDK                    | OpenAI Agents SDK                    |
|------------------------|----------------------------------------|--------------------------------------|
| **Philosophy**         | SDK-first, developer-controlled        | Platform-first, product-focused      |
| **Runtime**            | Full autonomous runtime with computer  | Lightweight orchestration framework  |
| **Tool execution**     | Built-in (Read, Write, Bash, etc.)     | You implement tool execution         |
| **Agent loop**         | Fully managed internally               | Managed but simpler                  |
| **Core primitives**    | Tools, Hooks, Subagents, MCP, Sessions | Agents, Handoffs, Guardrails         |
| **Model lock-in**      | Claude only                            | Provider-agnostic (100+ LLMs)        |
| **Infrastructure**     | Your servers, full data control        | OpenAI platform or self-hosted       |
| **Complexity**         | Higher (more power)                    | Lower (faster to ship)              |
| **Open source**        | Yes (MIT)                              | Yes                                  |

### OpenAI Agents SDK Core Architecture

Three primitives:
1. **Agents**: LLMs with instructions and tools
2. **Handoffs**: Agent-to-agent delegation (agent B becomes a "tool" for agent A)
3. **Guardrails**: Input/output validation running in parallel with execution

```python
# OpenAI Agents SDK
from agents import Agent, Runner

agent = Agent(name="Assistant", instructions="You are a helpful assistant")
result = Runner.run_sync(agent, "Write a haiku")
print(result.final_output)
```

### Key Differences

1. **Anthropic gives you a computer; OpenAI gives you a framework.**
   The Agent SDK includes bash, file I/O, and web access out of the box.
   OpenAI requires you to implement every tool.

2. **Anthropic uses MCP; OpenAI uses function calling + MCP.**
   Both now support MCP, but Anthropic created and leads the protocol.

3. **Multi-agent**: Anthropic uses subagents with isolated contexts and
   explicit spawning. OpenAI uses handoffs where one agent delegates
   to another as a tool call.

4. **Memory**: Anthropic has built-in context compaction, session resumption,
   and file-based persistent memory. OpenAI has no durable memory out of the box.

5. **Safety**: Anthropic has hooks, permission modes, sandboxing.
   OpenAI has guardrails that run in parallel with agent execution.

6. **Tracing**: OpenAI has built-in tracing visualization integrated with
   evals and fine-tuning. Anthropic relies on hooks and transcript files.

### When to Choose Which

- **Anthropic SDK**: You need a full autonomous agent with computer access,
  have a dedicated engineering team, need data sovereignty, building
  coding/devops agents.

- **OpenAI SDK**: You want fast prototyping, multi-model flexibility,
  simple delegation patterns, customer-facing agents, and are comfortable
  implementing your own tools.

---

## 7. COMPARISON: GOOGLE ADK

### Architecture Overview

Google's Agent Development Kit (ADK) is an open-source, code-first framework
released at Google Cloud NEXT 2025. Available in Python, TypeScript, Go, and Java.

**Repo**: https://github.com/google/adk-python
**Docs**: https://google.github.io/adk-docs/

### Core Design Principles

1. **Multi-agent by design**: Hierarchical agent composition is a first-class concept
2. **Event-driven runtime**: Orchestrates agents, tools, and persistent state
3. **Model-agnostic**: Optimized for Gemini but supports Anthropic, Meta, Mistral via LiteLLM
4. **Deployment-agnostic**: Local, Cloud Run, or Vertex AI Agent Engine

### Workflow Agents (Unique to ADK)

ADK provides explicit workflow control primitives:

- **SequentialAgent**: Execute agents in order
- **ParallelAgent**: Execute agents simultaneously
- **LoopAgent**: Repeat until condition met

This is a key differentiator -- neither Anthropic nor OpenAI provide declarative
workflow agents. You can compose them:

```python
# ADK Workflow Composition (conceptual)
pipeline = SequentialAgent(
    agents=[
        ParallelAgent(agents=[researcher_1, researcher_2, researcher_3]),
        synthesizer_agent,
        LoopAgent(agent=refiner_agent, condition=quality_check)
    ]
)
```

### Three-Way Comparison

| Dimension              | Anthropic SDK         | OpenAI Agents SDK      | Google ADK             |
|------------------------|-----------------------|------------------------|------------------------|
| **Multi-agent**        | Subagents + Teams     | Handoffs               | Hierarchical + Workflow|
| **Workflow control**   | Implicit (agent decides) | Implicit (handoffs) | Explicit (Sequential/Parallel/Loop) |
| **Model support**      | Claude only           | 100+ via API           | Gemini + LiteLLM       |
| **Built-in tools**     | Full computer access  | Minimal                | Search, Code Exec      |
| **Tool protocol**      | MCP (creator)         | MCP + function calling | MCP + LangChain tools  |
| **Deployment**         | Your infrastructure   | Flexible               | Vertex AI native       |
| **Streaming**          | SSE                   | SSE                    | Bidirectional audio/video |
| **Testing**            | Via hooks             | Via guardrails         | Built-in test harness  |
| **Languages**          | Python, TypeScript    | Python, TypeScript     | Python, TS, Go, Java   |
| **Maturity**           | Production (powers Claude Code) | Production     | Production             |

---

## 8. LANGGRAPH ARCHITECTURE PATTERNS

### Core Concept: Agents as State Graphs

LangGraph models agents as directed graphs where:
- **Nodes** = Functions or LLM calls (processing steps)
- **Edges** = Transitions (can be conditional)
- **State** = Typed dictionary flowing through the graph

```python
# LangGraph conceptual structure
from langgraph.graph import StateGraph, MessagesState

graph = StateGraph(MessagesState)
graph.add_node("agent", call_llm)
graph.add_node("tools", execute_tools)
graph.add_edge("agent", "tools")
graph.add_conditional_edges("tools", should_continue, {"continue": "agent", "end": END})
```

### State Management (2025 Pattern)

Uses **reducer-driven state schemas** with Python's TypedDict and Annotated types:

```python
from typing import TypedDict, Annotated
from langgraph.graph import add_messages

class AgentState(TypedDict):
    messages: Annotated[list, add_messages]  # Reducer handles message merging
    context: str
    iteration_count: int
```

Reducers prevent data loss in multi-agent systems by defining how concurrent
updates to the same state field are merged.

### Checkpointing and Persistence

LangGraph provides durable execution via checkpointers:

- **InMemorySaver**: Development/testing
- **SqliteSaver**: Local workflows
- **PostgresSaver**: Production

Every super-step is checkpointed automatically, enabling:
- **Fault recovery**: Resume from last checkpoint after failure
- **Time travel**: Roll back and replay with different parameters
- **Human-in-the-loop**: Pause, inspect state, modify, resume
- **Async approval**: Agent pauses, human responds hours later

### Multi-Agent Orchestration Patterns

1. **Scatter-Gather**: Distribute to multiple agents, consolidate downstream
2. **Pipeline Parallelism**: Sequential stages processed concurrently
3. **Subgraphs**: Reusable agent groups (e.g., document processing subgraph)
4. **Supervisor Pattern**: LLM-based router delegates to specialized agents

### LangGraph vs Agent SDK Comparison

| Dimension              | LangGraph                        | Anthropic Agent SDK              |
|------------------------|----------------------------------|----------------------------------|
| **Abstraction**        | Graph of states and transitions  | Autonomous agent with computer   |
| **Control**            | Explicit (you define the graph)  | Implicit (agent decides)         |
| **State management**   | Typed state with reducers        | Automatic compaction             |
| **Persistence**        | Checkpointing (Postgres etc.)   | Session resumption + files       |
| **Human-in-loop**      | First-class (graph interrupts)   | Via AskUserQuestion tool         |
| **Multi-agent**        | Nodes in graph                   | Subagents with isolated contexts |
| **Debugging**          | Graph visualization              | Hooks + transcript files         |
| **Learning curve**     | Steeper (graph concepts)         | Lower (just prompt + tools)      |

LangGraph is best when you need **explicit control over agent workflow topology** and
**durable execution with checkpointing**. The Agent SDK is best when you want the agent
to **autonomously figure out its own workflow**.

---

## 9. AGENT PROTOCOLS & STANDARDS (A2A, MCP)

### Model Context Protocol (MCP)

**Status**: De facto industry standard for AI-to-tool connectivity.
**Governance**: Donated to Agentic AI Foundation (AAIF) under Linux Foundation in Dec 2025.
Co-founded by Anthropic, Block, and OpenAI.

**Adoption**: OpenAI, Google DeepMind, Microsoft, and thousands of developers.

#### Core Architecture

MCP defines a client-server protocol for connecting AI systems to external tools and data:

- **Servers** expose tools, resources, and prompts
- **Clients** (AI models/agents) discover and invoke them
- **Transports**: stdio (local), SSE (remote), Streamable HTTP (scalable)

#### Current Features (Nov 2025 spec)

- Tool definitions with JSON Schema
- Resource access (files, databases, APIs)
- Prompt templates
- Sampling (server requests LLM completion)
- Tool annotations (metadata about tool behavior)

#### 2026 Roadmap

| Feature                    | Status                              |
|----------------------------|-------------------------------------|
| Async Operations           | In progress (SEP-1686)              |
| Statelessness/Scalability  | In progress (SEP-1442)              |
| Server Identity (.well-known URLs) | In progress                 |
| Official Extensions        | Planned                             |
| SDK Support Standardization| Planned (tiering system)            |
| MCP Registry GA            | Preview launched Sep 2025           |
| Multimodal (images, video) | Planned for 2026                    |
| Servers-as-agents          | Planned for 2026                    |
| W3C MCP-Identity           | Formal discussions scheduled Apr 2026 |

### Agent-to-Agent Protocol (A2A)

**Status**: Open standard under Linux Foundation (donated by Google, Jun 2025).
**Version**: 0.3 (latest, with gRPC support).
**Supporters**: 50+ partners including Atlassian, Salesforce, SAP, LangChain, MongoDB.

#### Core Concepts

1. **Agent Cards**: JSON metadata documents describing agent identity, capabilities,
   skills, endpoint, and auth requirements. The "business card" for agents.

2. **Tasks**: Fundamental unit of work with lifecycle states:
   - submitted -> working -> completed/failed/canceled/rejected
   - input_required (awaiting client input)
   - auth_required (awaiting authentication)

3. **Messages**: Role-based (user/agent) with Parts (text, file, structured data)

4. **JSON-RPC Methods**: SendMessage, GetTask, ListTasks, CancelTask, SubscribeToTask

5. **Streaming**: SSE-based with TaskStatusUpdateEvent and TaskArtifactUpdateEvent

#### A2A vs MCP

| Dimension     | MCP                              | A2A                              |
|---------------|----------------------------------|----------------------------------|
| **Purpose**   | AI <-> Tools (vertical)          | Agent <-> Agent (horizontal)     |
| **Analogy**   | USB-C for AI                     | HTTP for agents                  |
| **Protocol**  | JSON-RPC over stdio/HTTP/SSE     | JSON-RPC over HTTP/SSE/gRPC      |
| **Discovery** | MCP Registry                     | Agent Cards                      |
| **Auth**      | Server-defined                   | OAuth, API keys, mTLS, OIDC      |
| **Creator**   | Anthropic                        | Google                           |
| **Governance**| AAIF (Linux Foundation)          | Linux Foundation                 |

**Key insight**: MCP and A2A are complementary, not competing. MCP connects agents
to tools/data; A2A connects agents to each other. A mature agent system uses both.

---

## 10. ACTIONABLE TAKEAWAYS

### For Building Agents Today

1. **Use the Anthropic Agent SDK if** you want autonomous agents with computer access
   and are building on Claude. It is the most "batteries-included" option.

2. **Use OpenAI Agents SDK if** you want fast prototyping with multi-model flexibility
   and simple delegation patterns.

3. **Use Google ADK if** you need explicit workflow control (Sequential/Parallel/Loop)
   and are building enterprise multi-agent systems, especially on GCP.

4. **Use LangGraph if** you need fine-grained control over agent workflow topology
   with durable execution, checkpointing, and human-in-the-loop patterns.

### Architecture Patterns Worth Adopting

1. **Orchestrator-Worker with Model Tiering**: Use Opus for lead agent, Sonnet for
   workers, Haiku for exploration. This is Anthropic's production pattern.

2. **File-Based Memory**: CLAUDE.md and progress files are simple, transparent, and
   effective. No vector DB needed for most use cases.

3. **Context Compaction**: Summarize older turns rather than truncating. Preserve
   intent, decisions, and open threads.

4. **Hooks for Safety**: PreToolUse hooks are the safest way to enforce constraints
   without modifying agent prompts.

5. **MCP for Tool Integration**: The protocol is now universal -- all major providers
   support it. Build MCP servers, not custom integrations.

### Emerging Standards to Watch

1. **A2A Protocol**: If building multi-vendor agent systems, implement Agent Cards
   now. The protocol is stabilizing quickly.

2. **MCP Registry**: Register your MCP servers for discoverability.

3. **MCP Async Operations**: Will unlock long-running tool calls without blocking.

4. **Agent Cards + .well-known URLs**: Server identity and discovery standard.

---

## SOURCES

### Anthropic Agent SDK
- [Building Agents with the Claude Agent SDK (Blog)](https://claude.com/blog/building-agents-with-the-claude-agent-sdk)
- [Agent SDK Overview (Docs)](https://platform.claude.com/docs/en/agent-sdk/overview)
- [Agent SDK Python Reference](https://platform.claude.com/docs/en/agent-sdk/python)
- [claude-agent-sdk-python (GitHub)](https://github.com/anthropics/claude-agent-sdk-python)
- [claude-agent-sdk-typescript (GitHub)](https://github.com/anthropics/claude-agent-sdk-typescript)
- [claude-agent-sdk-demos (GitHub)](https://github.com/anthropics/claude-agent-sdk-demos)
- [Create Custom Subagents (Docs)](https://code.claude.com/docs/en/sub-agents)
- [Multi-Agent Research System (Engineering)](https://www.anthropic.com/engineering/multi-agent-research-system)
- [Effective Harnesses for Long-Running Agents](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)
- [Advanced Tool Use](https://www.anthropic.com/engineering/advanced-tool-use)
- [Claude Agent SDK Best Practices](https://skywork.ai/blog/claude-agent-sdk-best-practices-ai-agents-2025/)
- [Claude Skills Deep Dive](https://leehanchung.github.io/blogs/2025/10/26/claude-skills-deep-dive/)
- [Context Engineering from Claude](https://01.me/en/2025/12/context-engineering-from-claude/)

### OpenAI Agents SDK
- [OpenAI Agents SDK Docs](https://openai.github.io/openai-agents-python/)
- [New Tools for Building Agents (OpenAI Blog)](https://openai.com/index/new-tools-for-building-agents/)
- [OpenAI Agents SDK Review](https://mem0.ai/blog/openai-agents-sdk-review)

### Google ADK
- [Google ADK Overview (Cloud Docs)](https://docs.google.com/agent-builder/agent-development-kit/overview)
- [ADK Docs](https://google.github.io/adk-docs/)
- [adk-python (GitHub)](https://github.com/google/adk-python)
- [ADK Architectural Tour (The New Stack)](https://thenewstack.io/what-is-googles-agent-development-kit-an-architectural-tour/)
- [ADK for TypeScript (Google Blog)](https://developers.googleblog.com/introducing-agent-development-kit-for-typescript-build-ai-agents-with-the-power-of-a-code-first-approach/)

### LangGraph
- [LangGraph Framework](https://www.langchain.com/langgraph)
- [LangGraph GitHub](https://github.com/langchain-ai/langgraph)
- [LangGraph Architecture Guide 2025](https://latenode.com/blog/ai-frameworks-technical-infrastructure/langgraph-multi-agent-orchestration/langgraph-ai-framework-2025-complete-architecture-guide-multi-agent-orchestration-analysis)
- [LangGraph State Management 2025](https://sparkco.ai/blog/mastering-langgraph-state-management-in-2025)
- [LangGraph Checkpointing Best Practices](https://sparkco.ai/blog/mastering-langgraph-checkpointing-best-practices-for-2025)

### Protocols & Standards
- [MCP Roadmap](https://modelcontextprotocol.io/development/roadmap)
- [A Year of MCP (Pento)](https://www.pento.ai/blog/a-year-of-mcp-2025-review)
- [MCP Wikipedia](https://en.wikipedia.org/wiki/Model_Context_Protocol)
- [A2A Protocol Specification](https://a2a-protocol.org/latest/specification/)
- [A2A Protocol Getting an Upgrade (Google)](https://cloud.google.com/blog/products/ai-machine-learning/agent2agent-protocol-is-getting-an-upgrade)
- [Announcing A2A (Google Blog)](https://developers.googleblog.com/en/a2a-a-new-era-of-agent-interoperability/)
- [A2A GitHub](https://github.com/a2aproject/A2A)
- [Linux Foundation A2A Launch](https://www.linuxfoundation.org/press/linux-foundation-launches-the-agent2agent-protocol-project-to-enable-secure-intelligent-communication-between-ai-agents)

### Comparisons
- [State of AI Agent Frameworks (Medium)](https://medium.com/@roberto.g.infante/the-state-of-ai-agent-frameworks-comparing-langgraph-openai-agent-sdk-google-adk-and-aws-d3e52a497720)
- [14 AI Agent Frameworks Compared (Softcery)](https://softcery.com/lab/top-14-ai-agent-frameworks-of-2025-a-founders-guide-to-building-smarter-systems)
- [Claude vs OpenAI Agents Deep Dive](https://sparkco.ai/blog/claude-vs-openai-agents-a-deep-dive-analysis)
- [How to Think About Agent Frameworks (LangChain)](https://blog.langchain.com/how-to-think-about-agent-frameworks/)
- [Google vs OpenAI vs Anthropic Arms Race (MarkTechPost)](https://www.marktechpost.com/2025/10/25/google-vs-openai-vs-anthropic-the-agentic-ai-arms-race-breakdown/)
