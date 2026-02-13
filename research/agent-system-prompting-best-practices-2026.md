# AI Agent System Prompting: State of the Art (Early 2026)

## Research Report -- Compiled February 13, 2026

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Claude Code System Prompt Analysis](#2-claude-code-system-prompt-analysis)
3. [Anthropic's Official Guidance](#3-anthropics-official-guidance)
4. [OpenAI Agent Prompting Patterns](#4-openai-agent-prompting-patterns)
5. [Real-World Production System Prompts](#5-real-world-production-system-prompts)
6. [Anti-Patterns: What NOT To Do](#6-anti-patterns-what-not-to-do)
7. [Dynamic Prompt Composition](#7-dynamic-prompt-composition)
8. [The "Acknowledge Before Acting" Pattern](#8-the-acknowledge-before-acting-pattern)
9. [The Six-Component Blueprint](#9-the-six-component-blueprint)
10. [Concrete Recommendations for a Personal AI Assistant](#10-concrete-recommendations-for-a-personal-ai-assistant)

---

## 1. Executive Summary

The field has undergone a major conceptual shift in 2025-2026. The dominant framing is no
longer "prompt engineering" but **context engineering** -- the discipline of designing dynamic
systems that provide the right information and tools, in the right format, at the right time,
to give an LLM everything it needs to accomplish a task.

Key takeaways:

- **Most agent failures are context failures, not model failures.** LangChain's 2025 State of
  Agent Engineering report found 57% of organizations have agents in production, but 32% cite
  quality as the top barrier -- and most failures trace to poor context management, not LLM
  capabilities.

- **System prompts should read like lightweight contracts**, not conversational prose. They need
  explicit structure: role, goals, constraints, tools, output format, and a final reminder of
  the most critical rules.

- **Dynamic composition is table stakes.** Production agents do not use static system prompts.
  They compose prompts from modular sections based on context, user preferences, available
  tools, and conversation state. Claude Code uses 110+ conditional prompt strings.

- **The "right altitude" principle** (from Anthropic): Avoid both overly rigid hardcoded logic
  and vague high-level guidance. Give agents the heuristics and principles they need to make
  good decisions independently.

- **Newer models need less aggressive prompting.** Claude Opus 4.6 and GPT-5.2 follow
  instructions more precisely. Previously necessary aggressive language ("CRITICAL: You MUST")
  now causes overtriggering. Use normal, clear language instead.

---

## 2. Claude Code System Prompt Analysis

### Architecture

Claude Code does NOT have a single monolithic system prompt. It uses a **modular, conditional
architecture** with 110+ distinct prompt strings, constantly updated across versions (currently
v2.1.41 as of Feb 12, 2026). Large portions activate based on:

- Environment variables and user configuration
- Operating mode (learning mode, plan mode, delegate mode)
- Available tools and MCP servers
- Git repository state
- CLAUDE.md file contents

### Core Identity Section

The main system prompt establishes:

```
You are an agent for Claude Code, Anthropic's official CLI for Claude. Given the
user's prompt, you should use the tools available to you to answer the user's question.
```

### Communication Style Rules

Claude Code enforces extreme conciseness for CLI output:

- Responses must be fewer than 4 lines (excluding tool use or code generation)
- No preambles like "The answer is" or "Here is what I will do next"
- Answer questions directly without elaboration
- Let code speak for itself -- do not add comments unless asked
- Do not use emojis

### Tool Definitions (18+ Builtin Tools)

Each tool gets a dedicated, self-contained description. The Bash tool alone is 1,067 tokens
covering sandboxing behavior, git/PR workflows, and security restrictions. Tools include:

- **Write, Edit, Read** -- file operations with safety constraints
- **Bash** -- command execution with sandboxing and security review
- **Task** -- sub-agent delegation for focused work
- **Explore, Plan** -- specialized sub-agents for research and planning
- **TodoWrite** -- task tracking (mandated for frequent use)
- **WebFetch** -- URL fetching with security restrictions
- **Grep, Glob** -- fast codebase searching

### Sub-Agent Architecture

Claude Code delegates to specialized sub-agents:

- **Explore agent**: Limited tool access (View, GlobTool, GrepTool, LS, ReadNotebook, WebFetchTool)
  for search/research when confidence is low
- **Plan agent**: For breaking down complex tasks before execution
- **Task agent**: For focused sub-tasks with clean context windows

Each sub-agent returns condensed 1,000-2,000 token summaries back to the main agent.

### Security Constraints

- Every Bash command triggers two LLM evaluations: command prefix extraction and file path
  impact analysis
- Command injection patterns (backtick substitutions, `$(...)`) trigger user confirmation
- "Assist with defensive security tasks only. Refuse to create, modify, or improve code that
  may be used maliciously."
- WebFetch restricted to URLs mentioned by user or in CLAUDE.md

### Behavioral Rules

- Never commit without explicit user request
- Run lint/typecheck commands before completion
- Be proactive only when requested
- Understand existing code patterns before making changes
- Never assume library availability
- Follow security best practices; never expose secrets

### CLAUDE.md Integration

CLAUDE.md files serve as user-customizable context injection:

- Read automatically at session start
- Become part of the system prompt
- Ideal for: common commands, code style, testing instructions, repo etiquette
- Warning: every line competes for context attention
- Must not contain secrets or credentials

### Key Structural Pattern: System Reminders

Claude Code uses ~40 "system reminders" -- short directive messages injected at strategic
points in the conversation to reinforce critical behaviors. This is distinct from the initial
system prompt and acts as a guardrail mechanism against drift in long conversations.

---

## 3. Anthropic's Official Guidance

### The "Right Altitude" Principle

From Anthropic's context engineering blog post (one of the most influential pieces in the
space):

**Anti-pattern (too rigid):** Hardcoding complex, brittle logic that creates fragility.
Example: elaborate if/then decision trees in the prompt.

**Anti-pattern (too vague):** High-level guidance lacking concrete signals. Example: "Be
helpful and thorough."

**Optimal approach:** Balance specificity with flexibility. Give agents heuristics and
principles, not scripts.

### System Prompt Structure Recommendations

- Organize into distinct sections using XML tags or Markdown headers
  - Examples: `<background_information>`, `<instructions>`, `## Tool guidance`
- Strive for minimal information that fully outlines expected behavior
- Test minimal prompts first, then add instructions based on observed failure modes
- The format of the prompt influences the format of the output

### Tool Design Principles

- Tools must be self-contained, robust, and extremely clear about intended use
- Minimize functionality overlap (ambiguity about which tool to use creates agent failures)
- Input parameters must be descriptive and unambiguous
- Return token-efficient information
- Curate a minimal viable toolset (bloated tool sets degrade decision-making)

### Context Retrieval: Just-in-Time Pattern

Rather than loading everything upfront:

1. Maintain lightweight identifiers (file paths, URLs, stored queries)
2. Load data dynamically at runtime using tools
3. Progressive disclosure: agents incrementally discover relevant context
4. Hybrid approach: retrieve some data upfront for speed, allow autonomous exploration at
   agent discretion

Claude Code exemplifies this: CLAUDE.md provides upfront context, while glob/grep provide
just-in-time retrieval.

### Long-Horizon Task Techniques

1. **Compaction**: Summarize conversation as context window fills. Preserve architectural
   decisions and unresolved bugs. Discard redundant tool outputs.

2. **Structured Note-Taking**: Agent writes persistent notes outside the context window.
   Enables tracking progress across complex tasks.

3. **Sub-Agent Architectures**: Specialized sub-agents with clean context windows. Main agent
   coordinates with high-level plan.

### Claude 4 Best Practices (from official docs)

**For agents with context compaction**, add to the system prompt:

```
Your context window will be automatically compacted as it approaches its limit, allowing
you to continue working indefinitely from where you left off. Therefore, do not stop tasks
early due to token budget concerns. As you approach your token budget limit, save your
current progress and state to memory before the context window refreshes. Always be as
persistent and autonomous as possible and complete tasks fully, even if the end of your
budget is approaching. Never artificially stop any task early regardless of the context
remaining.
```

**For proactive tool use:**

```xml
<default_to_action>
By default, implement changes rather than only suggesting them. If the user's intent is
unclear, infer the most useful likely action and proceed, using tools to discover any
missing details instead of guessing. Try to infer the user's intent about whether a tool
call (e.g., file edit or read) is intended or not, and act accordingly.
</default_to_action>
```

**For conservative/cautious behavior:**

```xml
<do_not_act_before_instructions>
Do not jump into implementation or change files unless clearly instructed to make changes.
When the user's intent is ambiguous, default to providing information, doing research, and
providing recommendations rather than taking action. Only proceed with edits, modifications,
or implementations when the user explicitly requests them.
</do_not_act_before_instructions>
```

**For safe autonomy (critical for personal assistants):**

```
Consider the reversibility and potential impact of your actions. You are encouraged to take
local, reversible actions like editing files or running tests, but for actions that are hard
to reverse, affect shared systems, or could be destructive, ask the user before proceeding.

Examples of actions that warrant confirmation:
- Destructive operations: deleting files or branches, dropping database tables, rm -rf
- Hard to reverse operations: git push --force, git reset --hard
- Operations visible to others: pushing code, commenting on PRs/issues, sending messages
```

**For reducing hallucination:**

```xml
<investigate_before_answering>
Never speculate about code you have not opened. If the user references a specific file, you
MUST read the file before answering. Make sure to investigate and read relevant files BEFORE
answering questions about the codebase. Never make any claims about code before
investigating unless you are certain of the correct answer - give grounded and
hallucination-free answers.
</investigate_before_answering>
```

**For parallel tool calling:**

```xml
<use_parallel_tool_calls>
If you intend to call multiple tools and there are no dependencies between the tool calls,
make all of the independent tool calls in parallel. Prioritize calling tools simultaneously
whenever the actions can be done in parallel rather than sequentially. However, if some tool
calls depend on previous calls to inform dependent values, do NOT call these tools in
parallel and instead call them sequentially. Never use placeholders or guess missing
parameters in tool calls.
</use_parallel_tool_calls>
```

**For reducing over-engineering (Claude Opus 4.6 tendency):**

```
Avoid over-engineering. Only make changes that are directly requested or clearly necessary.
Keep solutions simple and focused:

- Scope: Don't add features, refactor code, or make "improvements" beyond what was asked.
- Documentation: Don't add docstrings, comments, or type annotations to code you didn't
  change.
- Defensive coding: Don't add error handling, fallbacks, or validation for scenarios that
  can't happen.
- Abstractions: Don't create helpers, utilities, or abstractions for one-time operations.
  Don't design for hypothetical future requirements.
```

**Key migration note for Claude Opus 4.6:** Previously necessary aggressive language like
"CRITICAL: You MUST use this tool when..." now causes overtriggering. Use normal prompting
like "Use this tool when..." instead.

---

## 4. OpenAI Agent Prompting Patterns

### Responses API and Agent SDK

The OpenAI Agents SDK (released March 2025, available for Python and TypeScript) provides a
structured approach to agent definition:

```python
from agents import Agent, function_tool

@function_tool
def get_weather(city: str) -> str:
    """Returns weather info for the specified city."""
    return f"The weather in {city} is sunny"

agent = Agent(
    name="Haiku agent",
    instructions="Always respond in haiku form",
    model="gpt-5-nano",
    tools=[get_weather],
)
```

### Dynamic Instructions

The SDK explicitly supports dynamic instruction generation. Instructions can be either a
static string or a callable function that receives the current context:

```python
def dynamic_instructions(context: RunContextWrapper[UserInfo], agent: Agent) -> str:
    return f"""You are a personal assistant for {context.context.name}.
    Today's date is {datetime.now().strftime('%Y-%m-%d')}.
    The user's timezone is {context.context.timezone}.
    Always respond in their preferred language: {context.context.language}."""

agent = Agent(
    name="Personal Assistant",
    instructions=dynamic_instructions,
    tools=[...],
)
```

### Context Management Pattern

The SDK distinguishes between:

1. **Local context** (RunContextWrapper) -- Python objects available to your code, tools, and
   lifecycle hooks. Not visible to the LLM.
2. **Agent/LLM context** -- What the model actually sees. Must be explicitly placed in
   conversation history, instructions, or tool results.

Ways to expose context to the LLM:
- Static or dynamic instructions (system prompt)
- Input messages
- Function tools (on-demand context; LLM decides when to fetch)
- Retrieval/web search

### Multi-Agent Composition Patterns

**Manager pattern** (agents as tools):

```python
customer_facing_agent = Agent(
    name="Customer-facing agent",
    instructions="Handle all direct user communication.",
    tools=[
        booking_agent.as_tool(tool_name="booking_expert"),
        refund_agent.as_tool(tool_name="refund_expert"),
    ],
)
```

**Handoff pattern** (peer agents):

```python
triage_agent = Agent(
    name="Triage agent",
    instructions="Help users, handing off to specialists as needed.",
    handoffs=[booking_agent, refund_agent],
)
```

### GPT-5 / GPT-5.2 Specific Guidance

- For agentic tool-calling flows: upgrade to Responses API where reasoning persists between
  tool calls
- Request thorough, descriptive tool-calling preambles that update the user on progress
- GPT-5.2 excels at production agents prioritizing reliability and evaluability
- Clear length constraints: 3-6 sentences for typical answers, 2 or fewer for yes/no, short
  overview + 5 or fewer bullets for complex tasks

---

## 5. Real-World Production System Prompts

### Common Structural Elements Across 30+ Agents

Analysis of system prompts from Augment Code, Claude Code, Cursor, Devin, Cline, Kiro,
Perplexity, VSCode Agent, Gemini, Codex, and others reveals consistent patterns:

1. **Identity and Role** -- Clear persona statement. Always first.
2. **Capability Declaration** -- Explicit tool lists and operational boundaries.
3. **Output Specifications** -- Formatting rules for code, diffs, or structured responses.
4. **Security Constraints** -- Bash safety, injection prevention, content restrictions.
5. **Interaction Protocols** -- How agents communicate and interpret requests.
6. **Memory/Context Management** -- Systems for tracking history and preferences.

### Cline (VS Code Coding Agent)

- Extensive system prompt (~11,000 characters)
- Documents environment details comprehensively
- Implements confirmation workflows before major operations
- Separates tool definitions by function with usage guidelines
- Uses step-by-step execution with confirmation after each tool use

### Bolt (Browser-Based App Builder)

- Defines operational constraints upfront
- Enforces consistent code formatting standards
- Uses modular action sequencing with dependency ordering
- Uses "ULTRA IMPORTANT" markers for critical guidelines (though this approach is becoming
  less recommended with newer models)

### oh-my-opencode

Demonstrates a layered prompt composition:
- Base prompt with core instructions
- Optional skills content
- Environment context (temporal/locale information)
- User customization through prompt_append fields

---

## 6. Anti-Patterns: What NOT To Do

### The Prompting Fallacy

You cannot prompt your way out of a system-level failure. If agents consistently
underperform, the issue is likely the architecture of the collaboration, not the wording of
instructions. A 2,000-word prompt to make a fast generator act like a thinker is a bad
hire -- you need a different architecture.

### Over-Engineering Complexity

Do NOT jump to complex multi-agent frameworks for problems that may not require it.
Start with the simplest viable solution: a single-prompt, single-model system. Only add
complexity when the pain of the simple approach becomes clear and measurable.

### Specific Anti-Patterns

1. **Over-prompting with aggressive language**: "CRITICAL: You MUST..." causes overtriggering
   on modern models. Use normal language.

2. **Negative instructions only**: Telling models what NOT to do is less effective than telling
   them what TO do. "Do not use markdown" works worse than "Write in flowing prose paragraphs."

3. **Vague tool descriptions**: Leaving ambiguity about when/how tools should be used is a
   top failure mode. Tool descriptions need to be as carefully crafted as the system prompt
   itself.

4. **Overfitting to specific examples**: Examples guide models effectively but risk degradation
   on novel scenarios. Use diverse, canonical examples rather than exhaustive edge cases.

5. **Cascading tool execution without feedback**: Allowing multiple tools simultaneously
   without verification creates cascading errors.

6. **Ignoring environment constraints**: Failing to communicate system limitations (available
   tools, directory structure, permissions) explicitly.

7. **Bloated tool sets**: Too many overlapping tools degrade agent decision-making. Curate a
   minimal viable toolset.

8. **Static prompts in production**: Using the same prompt regardless of context, user
   preferences, or conversation state.

9. **Insufficient security boundaries**: System prompts reveal attack surface (role
   definitions, tool descriptions, policy boundaries). Treat every interaction as part of an
   expanded attack surface.

10. **Prompt length over clarity**: "Prompt engineering didn't become 'writing longer prompts'
    in 2026 -- it became writing clearer specs."

### The Sycophancy Trap

Anthropic explicitly addresses this in the Claude 4 system prompt: "Claude never starts its
response by saying a question was good, great, fascinating..." LLMs naturally default to
excessive flattery, which degrades trust and utility.

---

## 7. Dynamic Prompt Composition

### Why Static Prompts Fail

Different users, contexts, and conversation stages need different instructions. Static prompts
cannot adapt to:

- Changing tool availability (MCP servers connecting/disconnecting)
- User preference evolution over sessions
- Task-specific context requirements
- Security context changes
- Conversation state (early exploration vs. deep execution)

### Claude Code's Approach

Claude Code composes its system prompt from 110+ conditional strings:

1. **Environment metadata** injected at runtime: working directory, git status, platform,
   date, model version
2. **Directory structure snapshot**: file tree (excluding .gitignore patterns)
3. **Git status snapshot**: current branch, recent commits, staged changes
4. **CLAUDE.md contents**: user/project-specific instructions
5. **Topic-detection preprocessing**: analyzes whether input is a new thread
6. **Mode-specific sections**: learning mode, plan mode, delegate mode activate different
   prompt segments
7. **System reminders**: ~40 short directive messages injected at strategic conversation
   points

### OpenAI SDK's Approach

Dynamic instructions via callable functions:

```python
def generate_instructions(ctx: RunContextWrapper[AppContext], agent: Agent) -> str:
    sections = [CORE_IDENTITY]

    if ctx.context.user_preferences:
        sections.append(format_preferences(ctx.context.user_preferences))

    if ctx.context.available_tools:
        sections.append(format_tool_guidance(ctx.context.available_tools))

    if ctx.context.conversation_depth > 10:
        sections.append(COMPACTION_GUIDANCE)

    return "\n\n".join(sections)
```

### Microsoft Research: Dynamic Prompt Middleware (2025)

A research paper from Microsoft introduced the concept of a "prompt generation module" that
aggregates contextual signals:

- Session context
- Conversation history
- Relevant knowledge snippets
- Ranked skill lists

These signals are composed into a meta-prompt at inference time.

### Practical Composition Strategy

Based on the research, a recommended layered approach:

```
Layer 1: Core Identity (static)
    Who the agent is, fundamental personality, non-negotiable rules

Layer 2: Capability Context (semi-static, changes when tools change)
    Available tools, MCP servers, permissions

Layer 3: User Context (session-level)
    User preferences, name, timezone, language, interaction history

Layer 4: Task Context (per-request)
    Current objective, relevant files/data, success criteria

Layer 5: Conversation State (dynamic)
    Depth of conversation, whether in planning or execution mode,
    remaining context budget

Layer 6: Safety Reminders (periodic injection)
    Reinforcement of critical constraints at strategic points
```

### Prompt Caching Considerations

From Augment Code's research: structure prompts for appending during sessions to preserve
cache validity. Place changing state (timestamps, user messages) in user messages rather than
system prompts to avoid invalidating the cache on every request.

---

## 8. The "Acknowledge Before Acting" Pattern

### The Core Question

Should an agent acknowledge the user's request before executing a long operation, or should
it dive straight into tool calls? This is driven by **system prompt instructions**, not
hardcoded UX logic, though the UX framework determines what the user sees.

### How Claude Code Handles It

Claude Code's system prompt explicitly mandates conciseness and discourages preambles:
"Do not start responses with 'Here is what I will do next.'" It jumps directly to tool
calls. However, the Anthropic docs note this creates a tradeoff:

> "Claude's latest models tend toward efficiency and may skip verbal summaries after tool
> calls, jumping directly to the next action. While this creates a streamlined workflow,
> you may prefer more visibility into its reasoning process."

If you want acknowledgment:

```
After completing a task that involves tool use, provide a quick summary of the work
you've done.
```

### The Autonomy Dial Pattern

From Smashing Magazine's agentic AI UX patterns (Feb 2026), there are four levels of agent
autonomy, each implying different acknowledgment behavior:

1. **Observe and Suggest**: Agent only recommends actions. Maximum acknowledgment.
2. **Plan and Propose**: Agent creates a plan, shows it, waits for approval.
3. **Act with Confirmation**: Agent prepares actions, shows a summary, executes on approval.
4. **Act Autonomously**: Agent executes without acknowledgment. Minimum friction.

### Implementation via System Prompt

The acknowledgment pattern is best driven by the system prompt, not hardcoded logic, because
it needs to be context-sensitive:

```
## Action Protocol

For ROUTINE actions (reading files, running tests, searching):
- Execute immediately without asking for confirmation.
- Provide a brief summary after completion.

For SIGNIFICANT actions (editing files, creating resources):
- State what you plan to do in one sentence.
- Then execute unless the action is irreversible.

For IRREVERSIBLE actions (deleting, pushing, deploying, sending messages):
- Describe the action and its consequences.
- Wait for explicit user confirmation before proceeding.
- Never proceed with destructive actions without confirmation.
```

### The Vercel AI SDK Approach

The Vercel AI SDK v6 implements a "Tool Execution Approval" pattern where tool calls can be
paused for user approval. This is a UX-level mechanism, but the decision about which tools
require approval is driven by the system prompt and tool definitions.

### Best Practices for Acknowledgment

1. **Use a tiered system** based on action reversibility and impact
2. **Confirmation dialogs must be specific**: "Allow AI to delete draft_v1.txt?" not "Allow
   AI to use the file system?"
3. **Include a pre-flight summary** for high-impact actions: who/what/when/where/value,
   rollback strategy, cost/time estimate
4. **Do not over-confirm**: Confirmation fatigue leads users to rubber-stamp everything,
   defeating the purpose

---

## 9. The Six-Component Blueprint

Based on convergent recommendations from multiple sources (Anthropic, OpenAI, ilert,
PromptHub, Augment Code), here is the recommended structure for a production agent system
prompt:

### Component 1: Role and Identity

```
You are [name], a personal AI assistant for [user]. You have expertise in [domains].
Your communication style is [tone descriptors].
```

Keep it to 1-3 sentences. This anchors all subsequent behavior.

### Component 2: Goal and Success Criteria

```
Your goal is to [primary objective].
Success looks like:
- [Criterion 1]
- [Criterion 2]
- [Criterion 3]
```

Define what "good" looks like so the agent can self-evaluate.

### Component 3: Rules, Guardrails, and Constraints

```
## Rules
- [Required behavior 1]
- [Required behavior 2]

## Constraints
- [Forbidden behavior 1]
- [Forbidden behavior 2]

## Safety
- Never guess or fabricate information. Say "I don't know" when uncertain.
- For irreversible actions, always confirm with the user first.
```

Use positive framing ("Write in prose") over negative framing ("Don't use markdown") where
possible.

### Component 4: Tool Guidance

```
## Available Tools

You have access to the following tools:

### [tool_name]
- Purpose: [when to use this tool]
- Parameters: [what inputs it needs]
- Output: [what it returns]
- Guidance: [any special instructions for usage]
```

Tool descriptions should be self-contained and unambiguous. Minimize overlap between tools.

### Component 5: Output Format

```
## Response Format
- For simple questions: respond in 1-3 sentences of prose.
- For complex analysis: use a short overview paragraph followed by structured sections.
- For code: provide the code directly with minimal explanation unless asked.
- Always match the formality level of the user's message.
```

### Component 6: Key Reminder

```
## Critical Reminders
[Repeat the 2-3 most important constraints here. Research shows that restating critical
rules at the end of the prompt significantly improves adherence.]
```

---

## 10. Concrete Recommendations for a Personal AI Assistant

Based on all the research above, here are specific recommendations for driving agent behavior
through prompting rather than hardcoded logic.

### Recommendation 1: Use Dynamic Prompt Composition

Do not use a single static system prompt. Compose the prompt from modular layers:

```python
def build_system_prompt(user_context, tools, conversation_state):
    sections = []

    # Layer 1: Core identity (always present)
    sections.append(CORE_IDENTITY)

    # Layer 2: User preferences (session-level)
    if user_context.preferences:
        sections.append(format_user_preferences(user_context))

    # Layer 3: Available tools (changes with MCP connections)
    sections.append(format_tool_guidance(tools))

    # Layer 4: Action protocol (based on user's autonomy preference)
    sections.append(get_action_protocol(user_context.autonomy_level))

    # Layer 5: Task-specific context (per-request)
    if conversation_state.active_task:
        sections.append(format_task_context(conversation_state))

    # Layer 6: Critical reminders (always last)
    sections.append(CRITICAL_REMINDERS)

    return "\n\n".join(sections)
```

### Recommendation 2: Structure the Prompt with XML Tags

Anthropic's models respond well to XML-tagged sections. This also makes prompts more
maintainable:

```xml
<identity>
You are Clawdbot, a personal AI assistant. You are direct, concise, and action-oriented.
You prefer doing things over talking about doing things.
</identity>

<available_tools>
{dynamically_injected_tool_descriptions}
</available_tools>

<action_protocol>
For routine actions (reading, searching, analyzing): execute immediately.
For significant actions (writing, editing, creating): state your plan in one sentence, then
execute.
For irreversible actions (deleting, sending, deploying): describe the action and wait for
explicit confirmation.
</action_protocol>

<user_preferences>
{dynamically_injected_user_preferences}
</user_preferences>

<critical_reminders>
- Never fabricate information. If uncertain, say so.
- Investigate before answering. Read files before making claims about them.
- Match the user's energy and formality level.
</critical_reminders>
```

### Recommendation 3: Use the "Right Altitude" Principle

Instead of scripting exact behavior for every scenario, give the agent decision-making
principles:

**Too rigid:**
```
If the user asks about weather, use the weather_tool. If they ask about calendar,
use the calendar_tool. If they ask about email, use the email_tool.
```

**Too vague:**
```
Be helpful and use available tools as needed.
```

**Right altitude:**
```
When the user's request requires external data or actions, use the most appropriate tool.
Prefer tools that provide authoritative, real-time data over your own knowledge when the
information might be outdated. If multiple tools could work, prefer the more specific one.
If no tool fits, say what you would need to accomplish the task.
```

### Recommendation 4: Explain the "Why" Behind Constraints

Modern models generalize better from explanations than from bare rules:

**Less effective:**
```
NEVER use ellipses.
```

**More effective:**
```
Your response will be read aloud by a text-to-speech engine, so never use ellipses since
the text-to-speech engine will not know how to pronounce them.
```

### Recommendation 5: Handle Context Window Limits Explicitly

If your agent runs in a context-compacting harness:

```
Your context window will be automatically compacted as it approaches its limit. This means:
- Do not stop tasks early due to context budget concerns.
- Before compaction occurs, save progress and state to persistent storage.
- After compaction, re-read relevant state files to reorient yourself.
- Be as persistent and autonomous as possible; complete tasks fully.
```

### Recommendation 6: Inject System Reminders for Long Conversations

For conversations that span many turns, inject periodic reminders of critical constraints.
Claude Code does this with ~40 system reminders. For a personal assistant:

```python
# Every N turns, inject a reminder
if turn_count % 10 == 0:
    inject_system_reminder("""
    Reminder: You are {agent_name}. Stay focused on the user's current objective.
    Do not fabricate information. Confirm before irreversible actions.
    If you are uncertain about something, investigate using available tools.
    """)
```

### Recommendation 7: Separate Planning from Execution

Drive this through the system prompt, not hardcoded logic:

```
For complex multi-step tasks:
1. First, create a brief plan listing the steps you will take.
2. Execute each step, checking results before moving to the next.
3. If a step fails, reassess the plan before continuing.
4. After completion, provide a brief summary of what was accomplished.

For simple single-step tasks:
- Execute directly without planning overhead.
```

### Recommendation 8: Use Prompt Caching Strategically

Structure your prompt so that the static core (identity, tools, rules) comes first and the
dynamic parts (user context, conversation state) come later. This maximizes cache hits:

```
[STATIC: Core identity -- cacheable]
[STATIC: Tool definitions -- cacheable]
[STATIC: Rules and constraints -- cacheable]
[SEMI-STATIC: User preferences -- changes per session]
[DYNAMIC: Task context -- changes per request]
[STATIC: Critical reminders -- cacheable]
```

Note: placing dynamic content at the very end is ideal because prompt caching works on
prefix matching.

### Recommendation 9: Test Minimal Prompts First

From Anthropic's guidance: start with the simplest possible prompt. Modern models (Claude
Opus 4.6, GPT-5.2) are highly capable out of the box. Add instructions only when you observe
specific failure modes. Every instruction you add competes for attention.

### Recommendation 10: Version and Track Your Prompts

Treat prompts as code:
- Version control them
- Test them against evaluation sets
- Track performance metrics
- Deploy changes through a controlled process
- Monitor in production

This is the discipline of "PromptOps" -- systematic experimentation, rigorous versioning,
comprehensive testing, controlled deployment, and continuous monitoring.

---

## Key Sources

### Anthropic
- Effective Context Engineering for AI Agents (anthropic.com/engineering)
- Claude 4 Best Practices (platform.claude.com/docs)
- Claude Code: Best Practices for Agentic Coding (anthropic.com/engineering)
- CLAUDE.md documentation (claude.com/blog)

### OpenAI
- Agents SDK documentation (openai.github.io/openai-agents-python)
- GPT-5.2 Prompting Guide (cookbook.openai.com)
- Prompt Engineering guide (platform.openai.com/docs)

### System Prompt Collections
- Piebald-AI/claude-code-system-prompts (GitHub) -- 110+ Claude Code prompt strings
- EliFuzz/awesome-system-prompts (GitHub) -- 30+ agent prompts
- x1xhlol/system-prompts-and-models-of-ai-tools (GitHub) -- comprehensive collection

### Analysis and Commentary
- Simon Willison: Claude 4 system prompt analysis, prompt injection design patterns
- Augment Code: 11 prompting techniques for better AI agents
- Smashing Magazine: Designing for Agentic AI (Feb 2026)
- ilert: Engineering Reliable AI Agents prompt structure guide
- PromptHub: Prompt Engineering for AI Agents

### Research
- Microsoft Research: Dynamic Prompt Middleware (2025)
- LangChain: 2025 State of Agent Engineering / Context Engineering docs
- Lilian Weng: LLM Powered Autonomous Agents (foundational reference)
