# AetherVault Bleeding-Edge Evolution Strategy

## Research Synthesis from 6 Parallel Deep Research Agents
**Date**: February 9, 2026

---

## EXECUTIVE SUMMARY

AetherVault should evolve into a **multi-model, multi-tool, event-driven personal AI system** by integrating:
1. **Grok API** for real-time Twitter/X data access (the killer differentiator no other LLM has)
2. **Sandboxed code execution** via Monty (Pydantic's Rust interpreter) + E2B fallback
3. **MCP protocol** for universal tool connectivity
4. **Event-driven architecture** for proactive behavior (not just reactive)
5. **Personal knowledge graph** for omniscient context awareness

---

## 1. GROK API INTEGRATION (HIGHEST PRIORITY)

### Why Grok is Critical
Grok is the **only LLM with native, real-time X/Twitter data access** via the `x_search` tool. No other model (Claude, GPT, Gemini) has this. This gives AetherVault the ability to monitor Twitter in real-time, analyze sentiment, track specific accounts, and surface breaking news.

### API Details
- **Endpoint**: `https://api.x.ai/v1/responses` (Responses API)
- **Compatibility**: 100% OpenAI SDK compatible — just change `base_url`
- **Best model for tools**: `grok-4-1-fast` — $0.20/1M input, $0.50/1M output, **2M context window**
- **Free credits**: $25 on signup + $150/month with data sharing = **$175 first month free**
- **Tool invocations**: $5 per 1,000 calls

### Built-in Server-Side Tools
| Tool | Description |
|------|-------------|
| `x_search` | Search X posts, users, threads — filter by handles, date ranges |
| `web_search` | Search the internet and browse pages |
| `code_execution` | Execute Python code in xAI's sandbox |
| `document_search` | Search uploaded document collections |

### Integration Pattern for AetherVault
```python
# Add to AetherVault as a new tool or subagent model_hook
import requests, os

def grok_x_search(query, allowed_handles=None, from_date=None, to_date=None):
    """Query Twitter/X via Grok API."""
    headers = {
        "Content-Type": "application/json",
        "Authorization": f"Bearer {os.environ['XAI_API_KEY']}"
    }
    tool = {"type": "x_search"}
    if allowed_handles:
        tool["allowed_x_handles"] = allowed_handles
    if from_date:
        tool["from_date"] = from_date
    if to_date:
        tool["to_date"] = to_date

    payload = {
        "model": "grok-4-1-fast",
        "input": [{"role": "user", "content": query}],
        "tools": [tool],
        "inline_citations": True
    }
    return requests.post("https://api.x.ai/v1/responses", headers=headers, json=payload).json()
```

### Monitoring Use Case
At Grok 4.1 Fast pricing, monitoring 10 topics every 15 minutes costs ~$5-10/day — **dramatically cheaper than Twitter's own API ($200/month)**.

### Python SDK
```bash
pip install xai-sdk
```
```python
from xai_sdk import Client
from xai_sdk.chat import user
from xai_sdk.tools import x_search, web_search

client = Client(api_key=os.getenv("XAI_API_KEY"))
chat = client.chat.create(model="grok-4-1-fast", tools=[x_search(), web_search()])
chat.append(user("What are the latest tweets about AI agents?"))
for response, chunk in chat.stream():
    print(chunk.content, end="")
```

### Grok Voice Agent API
- **<700ms response latency** (5x faster than competitors)
- 100+ languages with native-quality accents
- Compatible with OpenAI Realtime API spec
- Available via LiveKit plugin
- $0.05/minute

---

## 2. SANDBOXED CODE EXECUTION

### Monty (pydantic/monty) — Primary Choice
- **What**: Rust-based Python interpreter designed for AI agent code execution
- **Startup**: Single-digit **microseconds** (vs hundreds of ms for containers)
- **Security**: Process-level isolation, no filesystem/network access
- **Limitations**: No standard library (math/string only), no classes yet
- **Status**: Active development, successor to mcp-run-python (archived)
- **Integration**: Ships as a Rust library — can be compiled directly into AetherVault

### E2B — Fallback/Heavy Lifting
- **What**: Cloud sandbox platform with Firecracker microVMs
- **Startup**: ~150ms cold start
- **Languages**: Any (full Linux runtime)
- **Free tier**: $100 in credits on signup
- **API**: Python/JS SDKs + REST API
- **Integration**: `pip install e2b-code-interpreter`
```python
from e2b_code_interpreter import Sandbox
sbx = Sandbox.create()
execution = sbx.run_code("print(2 + 2)")
print(execution.logs)  # "4"
```

### Recommended Architecture
```
User asks: "Write and run a Python script to analyze my data"
    │
    ├── Simple math/logic → Monty (microsecond, in-process)
    ├── Full Python with imports → E2B (150ms, cloud sandbox)
    └── System commands → AetherVault exec tool (local, auto-approved)
```

### Other Notable Sandbox Options
| Platform | Cold Start | Isolation | Pricing |
|----------|-----------|-----------|---------|
| Daytona | ~90ms | Docker/Kata | $200 free credit |
| Modal | Sub-second | gVisor | $0.047/vCPU-hour |
| microsandbox | Fast | libkrun | Self-hosted, Apache-2.0 |
| Cloudflare Workers | Instant | V8 isolates | JS only |

---

## 3. MCP PROTOCOL ECOSYSTEM

### Key MCP Developments (Nov 2025 Spec)
- **Tasks primitive**: Async long-running operations (call-now, fetch-later)
- **MCP Apps**: Interactive UI components rendered in chat (iframes)
- **OAuth 2.1 support**: Added June 2025
- **MCP Gateways**: Single endpoint managing multiple MCP servers

### Critical MCP Servers for AetherVault
| Server | Purpose | Link |
|--------|---------|------|
| **grok-search-mcp** | Web + X/Twitter search via Grok | github.com/stat-guy/grok-search-mcp |
| **X (Twitter) MCP** | Direct Twitter access | pulsemcp.com/servers/enescinr-twitter-mcp |
| **Gmail MCP** | Email access | Available on mcpmarket.com |
| **Calendar MCP** | Calendar access | Available on mcpmarket.com |
| **Slack MCP** | Slack messaging | Available on mcpmarket.com |
| **GitHub MCP** | Repo management | Available on mcpmarket.com |

### MCP + A2A
- **MCP** = agent-to-tool communication (vertical)
- **A2A** (Google) = agent-to-agent communication (horizontal)
- MCP has won — it's the de facto standard. A2A is fading.

---

## 4. MULTI-AGENT FRAMEWORK LANDSCAPE

### Top Frameworks (Feb 2026)
| Framework | Best For | Status |
|-----------|----------|--------|
| **OpenAI Agents SDK** | Production agents with handoffs | GA, replacing Assistants API |
| **Claude Agent SDK** | Computer use, code, files | GA, renamed from Claude Code SDK |
| **LangGraph** | Complex enterprise workflows | Industry standard |
| **CrewAI** | Role-based multi-agent | Rapid prototyping |
| **AutoGen v0.4** | Distributed agent networks | Complete redesign, async |
| **Manus** | General-purpose autonomy | Acquired by Meta ($2-3B) |

### Key Breakthroughs
- **Anthropic Tool Search**: Agents discover tools on-demand without loading all into context
- **Programmatic Tool Calling (PTC)**: Claude writes code that calls multiple tools in one execution
- **Devin 2.0**: AI software engineer, now $20/month (was $500), 83% more efficient
- **Claude Cowork**: Desktop agent for non-technical users (files, spreadsheets, documents)

---

## 5. OMNISCIENT ASSISTANT DESIGN PATTERNS

### Proactive vs Reactive
Modern assistants don't wait for commands. They use event-driven architecture:
- **Temporal Schedules** for periodic tasks (replaces cron)
- **Event streams** (Kafka/Confluent) for real-time triggers
- **Personal knowledge graph** for context awareness

### Privacy and Trust
- **OAuth 2.0 On-Behalf-Of** extension for AI agents (IETF draft)
- Scope-based consent: users see and control exactly what agents can do
- Continuous consent: re-check permissions during activity

### Memory Architecture (State of the Art)
**Letta (formerly MemGPT)** leads with:
- **Core memory** (in-context, like RAM) — what the agent knows right now
- **Archival memory** (external, like disk) — long-term fact storage
- **Recall memory** — conversation history search

### The "Jarvis" Problem — Remaining Challenges
1. Cross-service orchestration (Gmail + Calendar + Slack in one action)
2. Ambient awareness (knowing user's context without asking)
3. Trust calibration (when to act vs when to ask)
4. Multi-device coordination (phone + desktop + cloud)

---

## 6. IMPLEMENTATION ROADMAP FOR AETHERVAULT

### Phase 1: Grok Integration (1-2 days)
1. Add `XAI_API_KEY` to `.env`
2. Create Grok model hook (like codex-hook, but for xAI)
3. Add "grok" subagent with x_search and web_search capabilities
4. Create `twitter_monitor` exec tool that polls Grok x_search periodically
5. Test: "What are the latest tweets about AI agents?"

### Phase 2: Sandboxed Code Execution (2-3 days)
1. **Option A (Quick)**: Add E2B as a tool via Python wrapper
   - `pip install e2b-code-interpreter` on droplet
   - Create exec tool `sandbox_run` that spins up E2B sandbox
2. **Option B (Native)**: Compile Monty into AetherVault binary
   - Add `monty` as Cargo dependency
   - Create `safe_python` tool that runs code through Monty
3. Both options can coexist: Monty for simple code, E2B for complex

### Phase 3: MCP Gateway (3-5 days)
1. Install `grok-search-mcp` server on droplet
2. Add MCP client support to AetherVault (or use `aethervault` exec to call MCP CLI)
3. Connect Gmail, Calendar, GitHub MCP servers
4. Create unified tool discovery endpoint

### Phase 4: Event-Driven Proactive Mode (5-7 days)
1. Add background polling loop for Twitter monitoring
2. Implement Temporal-style schedule for periodic tasks
3. Create "morning briefing" that checks email + calendar + Twitter
4. Add "alert on mention" for specific Twitter keywords/handles

### Phase 5: Knowledge Graph (7-10 days)
1. Integrate Graphiti (github.com/getzep/graphiti) for personal knowledge graph
2. Build user context model from conversation history
3. Enable "ambient awareness" — agent knows what you're working on
4. Add preference learning from interaction patterns

---

## KEY METRICS TO TRACK

| Metric | Current | Target |
|--------|---------|--------|
| Response time | ~5-15s | <3s for simple, <30s for research |
| Memory usage | 12.5 MB RSS | <50 MB with all features |
| Twitter search latency | N/A | <2s via Grok |
| Code execution latency | N/A | <100ms (Monty), <500ms (E2B) |
| Subagents active | 3 | 5+ (add grok, sandbox) |
| Tools available | ~10 | 30+ (via MCP) |
| Proactive actions/day | 0 | 10+ (morning brief, alerts, reminders) |

---

## COST ESTIMATE

| Service | Monthly Cost |
|---------|-------------|
| Grok API (monitoring) | $150-300 (covered by free credits first month) |
| Claude API (main agent) | Existing budget |
| E2B sandbox | $0-50 (free tier covers testing) |
| Codex CLI | $0 (ChatGPT Pro subscription) |
| DigitalOcean droplet | Existing ($12/mo) |
| **Total incremental** | **~$200-350/month** |

---

## SOURCES

### Grok API
- [xAI Models and Pricing](https://docs.x.ai/developers/models)
- [xAI Python SDK](https://github.com/xai-org/xai-sdk-python)
- [Grok Voice Agent API](https://x.ai/news/grok-voice-agent-api)
- [Real-Time Sentiment Analysis Cookbook](https://docs.x.ai/cookbook/examples/sentiment_analysis_on_x)
- [Grok Search MCP Server](https://github.com/stat-guy/grok-search-mcp)

### Agent Frameworks
- [OpenAI Agents SDK](https://openai.github.io/openai-agents-python/)
- [Claude Agent SDK](https://github.com/anthropics/claude-agent-sdk-python)
- [Letta (MemGPT)](https://github.com/letta-ai/letta)
- [LangGraph](https://langchain-ai.github.io/langgraph/)

### MCP Protocol
- [MCP Nov 2025 Spec](https://modelcontextprotocol.io/specification/2025-11-25)
- [MCP Apps](http://blog.modelcontextprotocol.io/posts/2026-01-26-mcp-apps/)
- [MCP Tasks Primitive](https://dev.to/gregory_dickson_6dd6e2b55/mcp-gets-tasks-a-game-changer-for-long-running-ai-operations-2kel)

### Sandboxed Execution
- [Pydantic Monty](https://github.com/pydantic/monty)
- [E2B](https://e2b.dev/)
- [Awesome Sandbox](https://github.com/restyler/awesome-sandbox)
- [Sandbox Comparison 2026](https://northflank.com/blog/best-code-execution-sandbox-for-ai-agents)

### Personal AI Design
- [Andrew Ng Agentic AI Course](https://learn.deeplearning.ai/courses/agentic-ai/)
- [Graphiti Knowledge Graphs](https://github.com/getzep/graphiti)
- [OAuth for AI Agents (IETF)](https://datatracker.ietf.org/doc/html/draft-oauth-ai-agents-on-behalf-of-user-00)
- [Event-Driven AI Agents (Confluent)](https://www.confluent.io/blog/the-future-of-ai-agents-is-event-driven/)
