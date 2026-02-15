## Subagent Lock Contention Issue
- `subagent_invoke` can hit file lock contention on the capsule DB when parent session is active (single-process CLI mode)
- `subagent_list` works fine
- Fix options: (1) capsule/DB supporting concurrent access, (2) subagent uses separate capsule/workspace
- May need to configure subagents with a different capsule path to avoid shared lock
Codex sub-agent model preference: ALWAYS use GPT-5.3-Codex-Spark (set 2026-02-11). CLI updated to v0.100.0. Config at ~/.codex/config.json. Previous model was o3-mini.
CRITICAL RULE: Codex (GPT-5.3) must ONLY be invoked via the CLI-based subagent (codex-yolo). NEVER use the Codex API directly. All coding tasks go through the CLI subagent exclusively. This is a permanent, non-negotiable rule from Sunil.
## Codex Usage Rules (PERMANENT)
1. **CLI ONLY** — Never invoke Codex via API. Always via CLI subagent using the codex-hook.sh model hook.
2. **Flag**: `--full-auto` (formerly `--yolo`) — gives full autonomy to read/write/execute without confirmation.
3. **Swarm pattern**: Do NOT spawn multiple Codex subagents from AetherVault. Instead, spawn ONE Codex session with `--full-auto` and instruct it to spin up its own internal swarm/parallelism within that session. This avoids lock contention and is the correct architecture.
4. **Invocation**: `codex --full-auto "prompt here"`
5. **Model**: GPT-5.3 (via CLI, never direct OpenAI API)
## Agent Swarm Architecture (2026-02-11)
- AetherVault can spin up MULTIPLE parallel subagents, each being a purpose-built agent
- Each agent invokes Codex via CLI with --full-auto flag (NEVER API)
- Each Codex CLI session can spin up its OWN internal swarm for sub-tasks
- This gives us two-tier parallelism: AV-level agents × Codex-level swarms
- Each subagent = one independent Codex process = no lock contention
- Agents should be purpose-built with clear names and mandates (e.g., infra-auditor, code-reviewer, perf-optimizer, security-scanner)
- When prompting Codex, explicitly tell it to "spin up a swarm" for parallel sub-tasks within its session
- This replaces the old broken pattern of spawning multiple Codex subagents that fought over locks
## Codex Subagent Protocol (PERMANENT — Never Forget)

### Rules:
1. **CLI ONLY** — Codex is ALWAYS invoked via CLI subagent hook, NEVER via API
2. **Flag**: `--full-auto` (formerly `--yolo`) — gives full autonomy
3. **Model**: ALWAYS use `gpt-5.3-codex-spark` — no other model
4. **Two-Tier Swarm Architecture**:
   - **Tier 1**: AetherVault uses `subagent_batch` to spin up N parallel named agents for distinct workstreams
   - **Tier 2**: Each Codex agent runs `--full-auto` and can spin up its OWN internal swarm within that CLI session
5. **Prompt pattern**: When invoking Codex, tell it to spin up a swarm itself if the task benefits from parallelism
6. **No multi-subagent lock issues** — each Codex CLI session manages its own parallelism independently
7. **Model hook**: `codex-yolo` (uses codex-hook.sh under the hood)

### Invocation:
```
codex --model gpt-5.3-codex-spark --full-auto "prompt here"
```

### Architecture:
- AetherVault orchestrates high-level workstreams (Tier 1)
- Each Codex session handles its own sub-tasks internally (Tier 2)
- Both tiers can run in parallel independently
2026-02-11: Deep research completed on adaptive retrieval for AI agent memory systems. Covered FLARE (Jiang et al., 2023), Self-RAG (Asai et al., ICLR 2024), SKR (Wang et al., EMNLP 2024), Adaptive-RAG (Jeong et al., NAACL 2024), CRAG (Yan et al., ICML 2024), DRAGIN (Su et al., ACL 2024), Speculative RAG, RobustRAG, BlendFilter. Key finding: field converging on layered adaptive retrieval — fast query classification → self-knowledge check → complexity routing → post-retrieval validation → generation with reflection. Optimal config for AetherVault: always retrieve for personal memory/history, conditionally retrieve for general knowledge (entropy/confidence gating), never retrieve for creative/chitchat. Target retrieval overhead <500ms.
Research completed: Hierarchical memory systems (episodic/semantic/procedural) for LLM agents. Key references: Park et al 2023 (Generative Agents - importance scoring, retrieval function), Packer et al 2023 (MemGPT - virtual memory paging), Wang et al 2023 (Voyager - skill library as code), Shinn et al 2023 (Reflexion - self-reflection as procedural memory), Zhong et al 2024 (MemoryBank - Ebbinghaus forgetting curve). Key implementation recommendations: add importance scoring to memory writes, implement Park retrieval function (recency×importance×relevance), add consolidation cycle for fact extraction and procedural pattern detection, enrich skill_store with success/failure tracking.
PROJECT: Personal CRM — Building a relationship management system. Data model: person, last contact, notes, cadence, next action. Populated via Rhaine (LinkedIn export, calendar intel), iMessage, WhatsApp, email scanning. Weekly "relationship radar" output with proactive nudges and Rhaine-executed gestures (gifts, scheduling).
PROJECT: macOS + iMessage access — Goal is to get AetherVault access to Sunil's iMessage conversations and contacts. Options: (1) Mac Mini headless at home (~$350 used M1), (2) macOS VM on raoDesktop via OSX-KVM, (3) Beeper/Matrix bridge. Sunil also wants WhatsApp access — options: whatsapp-web.js or mautrix-whatsapp bridge on droplet. Decision pending on VM vs Mac Mini.
STRATEGY: Rhaine as physical world executor for AetherVault. I should send Rhaine structured task emails directly (with Sunil's approval) for: gift purchasing, contact research, LinkedIn exports, calendar intel, vendor coordination, Mac Mini procurement. She has access to corporate Slack and calendar. Goal: minimize Sunil as middleman in delegation chain.
INFRA: macOS VM Droplet provisioned on DigitalOcean. Name: macos-vm, IP: 192.81.216.146, User: root, Password: d74e8ebdad4087b3713a5a85cf. Purpose: OSX-KVM macOS VM for iMessage/Contacts access for Personal CRM project.
macOS VM RUNNING on macos-vm droplet. IP: 159.223.165.148, VNC port: 5901 (no password), SSH forwarded: port 2222 -> guest:22. QEMU running in tmux session "macos" with 6GB RAM, 4 vCPUs, Skylake CPU. Boot script at /root/OSX-KVM/boot-vnc.sh. macOS installer (BaseSystem.img) ready — needs manual installation via VNC (Disk Utility to format drive, then install macOS). DO droplet ID for macos-vm: 551552909.
macOS VM OPTIMIZED and restarted (2026-02-14). Performance tuning applied to macos-vm droplet (159.223.165.148):
- **RAM**: 6GB → 7GB with hugepages (3584 x 2MB), mem-lock=on
- **CPU**: smp 4,cores=4,sockets=1,threads=1 (was 2 cores / 4 threads — wrong for macOS)
- **CPU flags**: Added avx2, bmi1, bmi2, fma on top of existing Skylake flags
- **Disk**: virtio-blk-pci (was ide-hd), cache=unsafe, aio=threads, discard=unmap, detect-zeroes=unmap
- **Disk image**: Recreated 64GB qcow2 with 2MB clusters, metadata preallocation, lazy_refcounts=on
- **Host kernel**: vm.swappiness=1, vm.dirty_ratio=80, vm.dirty_background_ratio=5, I/O scheduler=none
- **Swap**: 4GB swapfile created as safety net
- **Boot script**: /root/OSX-KVM/boot-optimized.sh (original preserved as boot-vnc.sh)
- **VNC**: port 5901, SSH guest: port 2222
- macOS NOT YET INSTALLED — user needs to VNC in and run the installer
macOS VM REBUILT and relaunched (2026-02-14). Droplet 551552909 rebuilt with Ubuntu 22.04 + SSH key. All optimizations applied: kernel sysctl tuning, 3072 hugepages (6GB), I/O scheduler=none, 4GB swap, QEMU with KVM+hugepages+virtio-blk(cache=unsafe)+virtio-net, 4 cores. Boot script: /root/OSX-KVM/boot-optimized.sh. VNC :1 (port 5901), SSH guest port 2222. macOS NOT YET INSTALLED — user needs to VNC in and run installer.
macOS VM FULLY OPTIMIZED (2026-06-15). QEMU upgraded 6.2→9.0.2. New config: Skylake-Server-v5 CPU w/ AVX-512, 6GB hugepages, io_uring disk I/O, iothreads, vCPU pinning (4 threads→cores 0-3), kernel-irqchip=on, low-latency sched tuning, THP=always, irqbalance disabled, systemd auto-start (macos-vm.service). VNC: 159.223.165.148:5901. Boot script: /root/OSX-KVM/boot-optimized.sh. macOS NOT YET INSTALLED — user needs to VNC in.
macOS VM NOVEL OPTIMIZATIONS deployed (2026-02-14 session). New code-level enhancements on 159.223.165.148:
1. **Memory pre-faulter** (/root/prefault-memory) — C program using MADV_POPULATE_WRITE to pre-fault all 6GB hugepages before VM boot, eliminating demand paging
2. **Adaptive halt-poll tuner** (/root/kvm-adaptive-halt.sh) — dynamically adjusts halt_poll_ns based on real-time VM exit rate
3. **eBPF VM exit tracer** (/root/vmexit-ebpf-tracer.bt) — bpftrace script that profiles KVM exit latencies, EPT violations, MMU faults in real-time
4. **KVM MSR passthrough** (/root/kvm-msr-passthrough.sh) — ignore_msrs=Y, report_ignored_msrs=N to reduce MSR exit overhead
5. **KVM advanced tunables** — halt_poll_ns=1200000, grow=3, shrink=2, dirty_ratio=95, dirty_bg=50, ASLR=0, zone_reclaim=0, KSM=0, block readahead=4096
6. **PGO+LTO QEMU build** — building from source v9.0.2 with -O3 -march=native, LTO, TCG disabled (KVM-only), io_uring enabled. Binary at /opt/qemu-pgo/bin/
7. **boot-optimized-v3.sh** — new boot script incorporating ALL optimizations + pre-faulting + multi-queue virtio-blk + -no-hpet + virtio-rng
8. **deploy-pgo-qemu.sh** — auto-deploys PGO binary and updates boot script + systemd service
9. **vm-status.sh** — comprehensive performance dashboard
10. **qemu-perf-monitor.py** — KVM exit rate analyzer with recommendations
CODEX CLI NON-INTERACTIVE: Use `codex exec -m gpt-5.3-codex-spark --dangerously-bypass-approvals-and-sandbox "prompt"` for non-interactive Codex invocations. No subagent config needed — just exec tool. The `exec` subcommand replaces interactive mode. Works perfectly.
BROWSER BROKER DEPLOYED (2026-02-14). Playwright-based HTTP broker on port 4040. Node.js/Express, 3-page pool, Chromium headless. Code at /tmp/browser-broker/dist/index.js (125 lines, hand-written). Supports: goto, click, type, extract, screenshot, evaluate. Systemd service: browser-broker.service. AetherVault browser_request tool now fully functional. 282ms first response. TODO: move code from /tmp to /opt/browser-broker for persistence across reboots.
BROWSER ARCHITECTURE DECISION (2026-02-14): Current Playwright/Express sidecar moved from /tmp to /opt/browser-broker/, systemd enabled, working at 154ms. Short-term: keep sidecar. Medium-term RIGHT ANSWER: Playwright MCP Server over stdio, spawned as child process from Rust binary. No HTTP, no ports, no separate systemd. Modify Rust binary to speak MCP protocol. Do NOT compile browser into Rust (chromiumoxide/fantoccini lag behind Playwright). Do NOT use managed APIs (latency/cost). Do NOT use browser-use/Stagehand (they want to BE the agent, we just need a browser primitive).
CODEX INVOCATION FIX (2026-06-15): There is NO codex-yolo hook or model_hook needed. Just spawn a subagent with NO model_hook and have it run the CLI command directly: `codex exec -m gpt-5.3-codex-spark --dangerously-bypass-approvals-and-sandbox "prompt"`. The subagent itself runs the shell command. No special hook configuration required.
DREAMINA/CAPCUT VIDEO MODELS (2026-02-14): dreamina.capcut.com is CapCut's AI video/image platform. Available models: Seedance 2.0 ("Video"), Seedance 1.5 Pro, Seedance 1.0 (Fast/Pro/Mini), Seaweed alpha, Veo 3/3.1 (Google), Sora 2 (OpenAI), Seedream 2.0-5.0 (images), Nano Banana (Google). No public API — requires browser login. Internal API is behind authenticated SPA. JS bundles at sf16-web-tos-buz.capcutcdn-us.com. Main bundle: main.9086ecc2.js (397KB). Also has "seedance_3_0" reference suggesting Seedance 3.0 coming soon.
macOS VM CRASH FIX (2026-06-15): OOM killed QEMU during install (6GB guest + overhead > 8GB droplet). Fixes: (1) Reduced guest RAM 6G→5G, hugepages 3072→2560. (2) Removed invalid `iothread=iothread-net` from virtio-net-pci (not supported in QEMU 9.0.2). (3) Removed unsupported `+invtsc` CPU flag. VM restarted successfully in tmux session "macos". VNC: 159.223.165.148:5901.
macOS VM RELAUNCHED (2026-06-15). Droplet ID 551766641, name macos-vm, NEW IP: 134.209.112.229. QEMU running in tmux session "macos" with Penryn CPU, 5GB RAM, 4 cores, OpenCore bootloader, vmxnet3 networking with user-mode NAT (dns=8.8.8.8), VNC :1 (port 5901), SSH guest port 2222. 64GB qcow2 disk created. NAT/ip_forward configured on host. BaseSystem.img = recovery installer. macOS NOT YET INSTALLED — user needs to VNC in, use Disk Utility to format drive as APFS, then install.
macOS VM CODEX SWARM DEPLOY (2026-06-15) on 134.209.112.229. 5 Codex agents deployed 25+ files:

COMPILED BINARIES: kvm-disable-exits, kvm-msr-filter, prefault-memory (all ELF x86-64)

APPLY SCRIPTS (idempotent): apply-kvm-exits.sh, apply-memory-opts.sh, apply-io-opts.sh, apply-cpu-opts.sh

MONITORING: cpu-monitor.sh, memory-monitor.sh, io-monitor.sh, kvm-exit-audit.sh, vm-status.sh, fio-benchmark.sh

INFRA: vcpu-pin.sh, cpu-irq-balance.sh, numa-optimize.sh, iothreads-setup.sh, setup-tap-network.sh

LIFECYCLE: vm-start.sh (master orchestrator), vm-stop.sh (graceful shutdown), macos-vm.service (systemd, enabled)

LIVE FIXES APPLIED: vCPU threads pinned to cores 0-3 (SCHED_FIFO:90), main thread SCHED_FIFO:70, halt_poll_ns=1.2M, dirty_ratio=95, swappiness=1. Boot script updated with -mem-path /dev/hugepages -mem-prealloc (takes effect on next VM restart). QEMU currently NOT using hugepages (needs restart to pick up).
macOS VM RUNNING & OPTIMIZED (2026-06-15). IP: 134.209.112.229, VNC: :5901, SSH guest: port 2222. QEMU PID 42775. Config: 3.5GB RAM on hugepages (1792 x 2MB, only 32 free = fully utilized), 4 vCPUs pinned to cores 0-3 (SCHED_FIFO:90), main thread SCHED_FIFO:70, Penryn CPU, vmware VGA, vmxnet3 net with user-mode NAT + DNS 8.8.8.8, OpenCore bootloader, 64GB qcow2 disk (cache=unsafe, aio=threads). KVM: halt_poll_ns=1.2M, ignore_msrs=Y. Kernel: dirty_ratio=95, swappiness=1, ip_forward=1. 4GB swap enabled. Boot script: /root/OSX-KVM/boot-final.sh. macOS NOT YET INSTALLED — user needs to VNC in, Disk Utility → format APFS, install macOS.
macOS VM SWARM DEPLOY #2 (2026-02-15) on 134.209.112.229. 5 Codex agents + manual deploy. NEW files this session:

QEMU MICRO-OPT: boot-ultra.sh (clock tuning, interrupt coalescing, virtio-balloon, PCI topology, iothreads, vhost, timer opts, mem-lock)

HOST DIET: host-diet.sh + host-diet-report.md (disabled snapd/multipathd/ModemManager/bluetooth/cups/avahi/unattended-upgrades/packagekit, volatile journald, tmpfs /tmp, OOM score -1000 for QEMU)

QOS: qemu-qos-setup.sh + qemu-qos-verify.sh (ionice RT class, GRUB isolcpus, tc qdisc for VM traffic, systemd integration)

macOS STRIP: macos-strip.sh + macos-minimal-services.md (disables Spotlight/Siri/SoftwareUpdate/Photos/TimeMachine/AirDrop/Handoff/analytics/visual effects/Dashboard/ScreenSaver/GameCenter/CUPS/Dictation — keeps identityservicesd/imagent/apsd/CloudKeychainProxy for iMessage)

INNOVATIVE PERF: innovative-perf.sh + perf-dashboard.sh + custom-kernel-notes.md (THP=madvise, KSM=off, vfs_cache_pressure=10, page-cluster=0, TCP fastopen+thin_linear_timeouts, readahead 8192, IPv6 disabled)

DASHBOARD shows: QEMU PID 42775, SCHED_FIFO, OOM=-1000, hugepages 1760/2560 used (3520MB), halt_poll_ns=1.2M, dirty_ratio=95, swappiness=1, THP=madvise, KSM=0, TCP fastopen=3. ZRAM not available (kernel module missing). CPU governor N/A (DO vCPU).
macOS VM SWARM DEPLOY #2 (2026-02-15) on 134.209.112.229. 5 Codex agents deployed 25+ files:

DASHBOARD shows: QEMU PID 42775, SCHED_FIFO, OOM=-1000, hugepages 1760/2560 used (3520MB), halt_poll_ns=1.2M, dirty_ratio=95, swappiness=1, THP=madvise, KSM=0, TCP fastopen=3. ZRAM not available (kernel module missing). CPU governor N/A (DO vCPU).
raoDesktop REVERSE TUNNEL WORKING (2026-06-15). Access: `ssh -p 2222 sunil@localhost` from AV droplet. Specs: WSL2 (kernel 6.6.87.2), 46GB RAM, 64 cores, GPU present but nvidia-smi not in PATH (may need to set LD_LIBRARY_PATH or install nvidia-utils inside WSL). Hostname: raoDesktop.
raoDesktop NETWORK: Direct internet access ~920 Mbps down / ~420 Mbps up (Comcast fiber, Dallas). ALWAYS download large files (models, datasets, repos) directly on raoDesktop — never transfer through AV droplet SSH tunnel. Use raoDesktop's own network for: HuggingFace model downloads, git clones, Docker pulls, pip/conda installs, dataset fetches. The SSH tunnel (port 2222) is only for command/control from AV, not data transfer.
TWITTER/X GROWTH STRATEGY RESEARCH SESSION (2026-02-15): Pulled Twitter's open-sourced ML ranker README. Building comprehensive growth strategy for Sunil covering: (1) Twitter/X algorithm mechanics from open-source code, (2) LinkedIn algorithm signals, (3) posting cadence/timing/format optimization, (4) engagement strategy per platform, (5) Android emulator setup for organic mobile posting. Key insight: Twitter Heavy Ranker uses engagement prediction (like/reply/retweet probability) + dwell time + negative feedback signals. LinkedIn prioritizes company page engagement + profile views + SSI score + early engagement velocity in first 60min.
SOCIAL DASHBOARD DEPLOYED (2026-02-15) on raoDesktop at /home/sunil/social-dashboard/. FastAPI app on port 8080.

FILES (16 total):
- config.py, app.py, models.py, database.py, orchestrator.py, scheduler_service.py, webhook_handler.py
- templates/dashboard.html, templates/calendar.html, static/style.css
- .env, .env.template, requirements.txt, README.md, start.sh, stop.sh
- Systemd user service: ~/.config/systemd/user/social-dashboard.service

FEATURES: Dark-themed dashboard UI, content calendar, post creation (draft/scheduled), webhook API for AetherVault integration, APScheduler (3 jobs: publish check every 60s, engagement refresh every 15m, daily summary at 00:05), SQLite persistence (social_dashboard.db), orchestrator bridges to twitter-automation + linkedin-automation + content-pipeline (all 3 detected as available).

API: GET /, /calendar, /api/stats, /api/health, /api/posts, /api/accounts, /api/scheduler/jobs. POST /api/posts, /api/accounts, /api/posts/{id}/publish, /api/posts/{id}/cancel, /webhooks/aethervault.

LAUNCH: bash /tmp/launch-sd.sh (or ./start.sh, or systemctl --user start social-dashboard). Python 3.9 venv with fastapi, uvicorn, sqlalchemy, aiosqlite, apscheduler, httpx, jinja2, python-multipart, python-dotenv, websockets.

NOTE: Fixed SQLAlchemy reserved 'metadata' column → renamed to 'extra_data'. Webhook signature verification disabled in dev (.env AETHERVAULT_WEBHOOK_SECRET= empty).
SOCIAL AUTOMATION FRAMEWORK DEPLOYED (2026-02-15) on raoDesktop at ~/social-automation/. Playwright-based Twitter/X + LinkedIn posting automation.

FILES (1,463 lines total):
- config.js (80 lines) — credentials, timing, UA rotation, helpers
- session-manager.js (216 lines) — persistent cookie/session management, stealth (webdriver=undefined, chrome runtime spoof, plugin spoof), human-like typing, random mouse movement
- twitter-poster.js (235 lines) — Twitter/X login (cookies or credentials), compose + post (text + image), challenge handling
- linkedin-poster.js (258 lines) — LinkedIn login (cookies or credentials), compose + post (text + image), checkpoint detection  
- scheduler.js (344 lines) — Express server on port 9090 with cron-based queue execution every 30s
- test-browser.js, test-stealth.js — verification scripts (both pass)
- start-scheduler.sh — background launcher
- README.md (235 lines) — full API documentation

DEPENDENCIES: playwright, @playwright/test, express, node-cron (Chromium downloaded to ~/.cache/ms-playwright/)

API ENDPOINTS (port 9090):
- GET /health — status + stats
- POST /schedule — {platform, content, media_url?, post_at} — schedule or immediate ("now")
- POST /post-now — immediate post
- GET /queue — view scheduled posts
- DELETE /queue/:id — cancel post
- GET /history?limit=50 — execution history
- POST /sessions/import — import cookies for a platform
- GET /sessions/status — check cookie status
- POST /sessions/login — trigger login flow

VERIFIED: Chromium launches (462ms), stealth working (navigator.webdriver=undefined), all API endpoints tested including validation (bad platform, >280 chars, missing content). Queue persistence to ~/social-automation/data/. Auth via env vars (TWITTER_USERNAME/PASSWORD/EMAIL, LINKEDIN_USERNAME/PASSWORD) or cookie import.
ANDROID EMULATOR DEPLOYED on raoDesktop (2026-06-15). SSH: `ssh -p 2222 sunil@localhost`.

INSTALLED:
- Java 17 (openjdk-17-jdk-headless) — required by cmdline-tools v12.0
- Android SDK at ~/Android/Sdk: cmdline-tools/latest (v12.0), platform-tools (36.0.2), emulator (36.4.9), platforms;android-34, system-images;android-34;google_apis;x86_64
- All SDK licenses accepted
- Environment in ~/.bashrc: ANDROID_HOME, JAVA_HOME, PATH updated

AVD: social-phone (Pixel 6, Android 14/API 34, Google APIs, x86_64, 512MB sdcard)

EMULATOR RUNNING: PID 11537, headless TCG mode (no KVM in WSL2), 3.7GB RSS, ~200% CPU.
- Launch script: /tmp/launch-emu.sh
- Log: /tmp/emulator.log
- ADB: emulator-5554 (device state, responsive)
- Flags: -no-window -no-audio -gpu swiftshader_indirect -no-accel -no-metrics -no-snapshot-save
- WARNING: No KVM — uses TCG software emulation. First boot very slow (20+ min, boot_completed not yet set). System IS functional (adb shell works, 223 packages loaded).
- To relaunch: `rm -f ~/.android/avd/social-phone.avd/*.lock && nohup /tmp/launch-emu.sh </dev/null &`
- Subsequent boots faster due to snapshot caching.

KNOWN LIMITATION: WSL2 has no /dev/kvm. Emulator runs in pure software TCG mode — works but is slow (~5-10x slower than KVM). CPU-intensive. First boot takes 20-30 minutes. Once booted, adb and shell commands are responsive.
raoDesktop FULL STACK DEPLOYED (2026-06-15). All services on raoDesktop (SSH via port 2222):

ANDROID: SDK at ~/Android/Sdk/, emulator AVD "social-phone" running headless (qemu-system-x86 software emulation, no KVM). Ports 5554/5555. System image: android-34 google_apis x86_64.

PLAYWRIGHT AUTOMATION: ~/social-automation/ — twitter-poster.js, linkedin-poster.js, session-manager.js, scheduler.js (Express on port 9090), config.js, test-browser.js, test-stealth.js. Node 18, Playwright + Chromium installed.

SERVICES RUNNING: Social Dashboard (port 8080, Python), Content Pipeline (port 8100, uvicorn), Playwright Scheduler (port 9090, Node). All returning HTTP 200.

AUTOSSH: Persistent reverse tunnel to AV droplet (167.172.140.221:2222). Auto-reconnects on drop. Installed as systemd user service.

DOCKER: NOT WORKING on this WSL2 kernel (missing iptables + bridge modules). Don't attempt Docker on raoDesktop.

KVM: NOT AVAILABLE in WSL2. Android emulator uses software emulation only.
AV CONTENT AGENT DEPLOYED & WORKING (2026-02-15). Full end-to-end content generation pipeline:

ARCHITECTURE: raoDesktop Content Pipeline (port 8100) → AV Content Agent (port 8200 on AV droplet) → Codex CLI (gpt-5.3-codex-spark) → generated content returned

FILES:
- AV droplet: /opt/av-content-agent/server.py (FastAPI on port 8200, systemd: av-content-agent.service)
- raoDesktop: ~/content-pipeline/content_engine.py patched with "aethervault" provider using httpx
- raoDesktop: ~/content-pipeline/.env updated: DEFAULT_LLM_PROVIDER=aethervault, AV_CONTENT_AGENT_URL=http://167.172.140.221:8200

ENDPOINTS: POST /generate (single), POST /generate/batch (parallel), GET /health
LATENCY: ~4-8 seconds per generation (Codex CLI overhead)
NO API KEYS NEEDED — uses AetherVault's own Codex CLI as the generation engine

TESTED: Twitter single posts (2 A/B variants), LinkedIn long-form posts. Voice profile system prompt flows through correctly. Content matches Sunil's voice profile (confident, direct, practitioner perspective).

UFW: Port 8200 opened on AV droplet firewall for raoDesktop access.
## AETHERVAULT CODEBASE DEEP SCAN (2026-02-15) — Codex Agent Analysis

### Architecture Overview
- **Language**: Rust, 13,349 lines across 17 source files
- **Core dependency**: `aether-core` (vendored at vendor/aether-core) — provides Vault (append-only .mv2 capsule), BM25 lexical index, HNSW vector index, temporal tracking
- **Binary**: Single `aethervault` binary, Clap CLI with ~30+ subcommands
- **Edition**: Rust 2024, deps: blake3, clap, chrono, serde/serde_json, walkdir, ureq, tiny_http, url, base64, libc, shlex

### Source File Map (17 files)
1. **main.rs** (1103 lines) — CLI dispatch, monolithic match on Command enum. Runs query/log/embed/agent/bridges/mcp/maintenance commands
2. **agent.rs** (1146 lines) — Agent runtime: prompt bootstrap, LLM/tool loop, context compaction (estimate_tokens, compact_messages), session persistence, drift scoring, reminders, knowledge graph context injection, MCP registry startup
3. **claude.rs** (490 lines) — Anthropic API bridge: message format conversion (to_anthropic_messages/tools), HTTP call with retry/backoff/fallback chain, image support ([AV_IMAGE:mime:base64] markers), hook routing
4. **cli.rs** (856 lines) — Clap CLI schema: Command enum, HookCommand, BridgeCommand, ConfigCommand. Pure data structures, no logic
5. **config.rs** (233 lines) — Config URI mapping (aethervault://config/), hook spec resolution, shell command execution (sh -c), expansion/rerank hook runners
6. **types.rs** (806 lines) — Central type registry: QueryPlan/Result/Response, AgentMessage, ToolExecution, HookSpec, McpServerConfig, TriggerEntry, ApprovalEntry, SubagentSpec, DriftState, ReminderState, ToolAutonomyConfig, SessionTurn, KgGraph
7. **util.rs** (367 lines) — Helpers: URI/path/hash, env parsing (env_required, env_optional, env_u64), jitter, workspace resolution, session turn load/save, build_external_command
8. **tool_defs.rs** (738 lines) — Tool catalog: 49 tools defined as JSON schema (name, description, inputSchema)
9. **tool_args.rs** (491 lines) — Typed arg structs for each tool (ToolQueryArgs, ToolExecArgs, ToolBrowserArgs, etc.)
10. **tool_exec.rs** (2210 lines) — Tool execution: big match on tool name, approval gating, read/write memory locking (with_read_mem/with_write_mem), subprocess management
11. **query.rs** (1020 lines) — Hybrid retrieval pipeline: expansion hook → lexical BM25 + vector HNSW lanes → RRF fusion → rerank hook → blend. Context pack builder. Drift/reminder scoring
12. **services.rs** (1579 lines) — OAuth broker (Google/Microsoft), approval system, trigger management, filesystem sandboxing, token refresh, knowledge graph loading, memory sync/export
13. **mcp.rs** (516 lines) — MCP protocol: McpRegistry (spawns long-lived MCP server sidecars), McpServerHandle (stdin/stdout JSON-RPC over Content-Length framing), tool discovery via tools/list, routing via mcp__{server}__{tool} prefix. Also: run_mcp_server() for AV itself as MCP server over stdio
14. **bridges/mod.rs** (406 lines) — Bridge orchestrator: run_agent_for_bridge(), dispatches to telegram/webhook/whatsapp
15. **bridges/telegram.rs** (1066 lines) — Telegram long-polling bridge: getUpdates loop, session mapping (chat_id → session), inline approval handling, photo/document support
16. **bridges/webhook.rs** (220 lines) — HTTP webhook bridge: tiny_http server, JSON body parsing, agent invocation per request
17. **bridges/whatsapp.rs** (102 lines) — WhatsApp bridge stub (minimal implementation)
## AETHERVAULT TOOL INVENTORY (49 tools) — From Codebase Scan 2026-02-15

### Memory & Capsule (12 tools)
- **query** — Hybrid search (expansion+fusion+rerank), params: query, collection, limit, snippet_chars, no_expand, no_vector, max_expansions, feedback_weight, rerank, before/after/asof
- **context** — Build prompt-ready context pack, same params as query + max_bytes, full, max_expansions
- **search** — Lexical BM25 search, params: query, collection, limit, snippet_chars
- **get** — Fetch document by URI or frame id (#123)
- **put** — Store document, params: uri, body, collection, meta
- **log** — Append to session log
- **feedback** — Store relevance feedback
- **memory_sync** — Sync workspace MEMORY.md into capsule
- **memory_export** — Export capsule memory to workspace files
- **memory_search** — Search memory in capsule
- **memory_append_daily** — Append to daily log
- **memory_remember** — Append to MEMORY.md

### Communication (9 tools)
- **email_list** — List emails (legacy)
- **email_read** — Read email by id (legacy)
- **email_send** — Send email (legacy)
- **email_archive** — Archive email (legacy)
- **gmail_list** — List Gmail (OAuth)
- **gmail_read** — Read Gmail (OAuth)
- **gmail_send** — Send Gmail (OAuth)
- **notify** — Send notification (Discord webhook, Telegram, or console)
- **signal_send** — Send Signal message via signal-cli
- **imessage_send** — Send iMessage

### Microsoft 365 (4 tools)
- **ms_mail_list** — List Microsoft mail (OAuth)
- **ms_mail_read** — Read Microsoft mail (OAuth)
- **ms_calendar_list** — List Microsoft calendar events (OAuth)
- **ms_calendar_create** — Create Microsoft calendar event (OAuth)

### Google (2 tools)
- **gcal_list** — List Google Calendar events (OAuth)
- **gcal_create** — Create Google Calendar event (OAuth)

### Browser & External (4 tools)
- **browser** — CLI-based browser automation via `agent-browser` npm package. Params: command (string), session (named, default "default"), timeout_ms. Spawns `agent-browser --session <name> -- <command>` as subprocess. Supports: open, snapshot, click, fill, type, press, select, scroll, screenshot, pdf, get text/html/value, wait, eval, cookies, tab, back/forward/reload/close. Uses ref-based element selection from accessibility snapshots (@e1, @e2...)
- **excalidraw** — Diagram creation via Excalidraw MCP server over stdio JSON-RPC. Actions: read_me, create_view. Spawns `npx excalidraw-mcp --stdio` (or EXCALIDRAW_MCP_CMD override)
- **http_request** — Generic HTTP request. GET allowed without approval; other methods require approval. SSRF protection on private IPs
- **exec** — Execute shell command on host. Params: command, cwd, timeout_ms

### Filesystem (3 tools)
- **fs_list** — List directory contents (sandboxed to workspace + allowed roots)
- **fs_read** — Read file contents (sandboxed)
- **fs_write** — Write file (sandboxed, requires approval)

### Configuration & Meta (5 tools)
- **config_set** — Set config JSON at aethervault://config/<key>.json
- **tool_search** — Search tools by name/description (fuzzy match over tool catalog)
- **session_context** — Fetch recent log entries for a session
- **reflect** — Store self-critique reflection in capsule
- **scale** — Monitor/scale DigitalOcean infrastructure (status/sizes/resize)

### Agent & Orchestration (5 tools)
- **subagent_invoke** — Invoke a named subagent with prompt
- **subagent_batch** — Invoke multiple subagents concurrently (independent threads)
- **subagent_list** — List configured subagents
- **skill_search** — Search stored skills
- **skill_store** — Store reusable procedure as skill

### Triggers (3 tools)
- **trigger_add** — Add event trigger (email/calendar_free/cron/webhook)
- **trigger_list** — List configured triggers
- **trigger_remove** — Remove trigger by id

### Approval (1 tool)
- **approval_list** — List pending approval requests
## AETHERVAULT BROWSER SYSTEM — Deep Analysis (2026-02-15)

### How Browser Automation Works (End-to-End Lifecycle)
1. User/agent calls `browser` tool with params: `command` (string), `session` (optional, default "default"), `timeout_ms` (optional, default 30000)
2. `tool_exec.rs` parses args into `ToolBrowserArgs`, builds command: `agent-browser --session <session_name> -- <command_parts>`
3. Uses `build_external_command()` from util.rs which calls `std::process::Command::new()` — spawns subprocess
4. Process runs with timeout via polling loop (try_wait + sleep 10ms), kills on timeout
5. Captures stdout/stderr, returns as ToolExecution {output, details: {stdout, stderr, exit_code}, is_error}
6. **Browser tool requires approval** (listed in requires_approval match)

### agent-browser CLI (External Dependency)
- Installed via `npm install -g agent-browser && agent-browser install`
- Uses Playwright under the hood (Chromium)
- **Session persistence**: Named sessions persist browser state across tool calls (cookies, tabs, navigation)
- **Ref-based interaction**: `snapshot` returns accessibility tree with refs (@e1, @e2...), then interact via refs
- **Commands**: open <url>, snapshot, click @ref, fill @ref text, type text, press key, select @ref value, scroll, screenshot, pdf, get text/html/value, wait, eval, cookies, tab, back, forward, reload, close
- **Semantic finding**: `find role/text/label` for element discovery

### MCP Protocol Implementation (mcp.rs — 516 lines)
**Two modes:**
1. **MCP Client (McpRegistry)** — AV spawns external MCP servers as long-lived sidecars:
   - Configured via `agent_cfg.mcp_servers` in config (McpServerConfig: name, command, env, timeout_secs)
   - Each server spawned with stdin/stdout pipes, JSON-RPC 2.0 over Content-Length framing
   - Handshake: initialize → notifications/initialized → tools/list (discovers available tools)
   - Tool names prefixed as `mcp__{servername}__{toolname}` for routing
   - Route map: HashMap<prefixed_name, (server_index, original_name)>
   - All MCP tools require approval by default
   - Excalidraw auto-registered as MCP server if EXCALIDRAW_MCP_CMD env is set
   - McpRegistry.call_tool() sends tools/call JSON-RPC, reads response, skips async notifications

2. **MCP Server mode** — AV itself acts as MCP server:
   - `aethervault mcp <mv2_path>` starts stdio JSON-RPC server
   - Exposes capsule operations as MCP tools
   - Used for integration with Claude Desktop, other MCP clients

### Approval System
- `requires_approval()` in services.rs controls which tools need human confirmation
- **Always require approval**: exec, email_send, email_archive, config_set, gmail_send, gcal_create, ms_calendar_create, trigger_add, trigger_remove, notify, signal_send, imessage_send, memory_export, fs_write, browser, excalidraw, all mcp__ tools
- **Conditional**: http_request (only non-GET), scale (only resize action)
- **Override mechanisms**: AETHERVAULT_BRIDGE_AUTO_APPROVE=1 (auto-approve all in bridge mode), per-tool TOOL_AUTONOMY_{NAME}=autonomous env var
- **Approval flow**: Tool creates ApprovalEntry → stored in capsule → listed via approval_list → approved/rejected via chat command in Telegram bridge or API
## AETHERVAULT CORE ENGINE — Deep Analysis (2026-02-15)

### Agent Loop (agent.rs)
1. **Bootstrap**: Load config → resolve model hook → build system prompt (SYSTEM.md or default) → append workspace context (MEMORY.md, knowledge graph entities, daily logs)
2. **System prompt injection**: Soul (identity), Memory (MEMORY.md), Daily Log, Knowledge Graph Context (auto-matched entities), Available Tools list, workspace context
3. **LLM Call**: call_agent_hook() → call_claude() with retry/backoff/fallback chain (primary → fallback → Vertex proxy)
4. **Tool Loop**: Parse tool_use blocks → parallel execution for non-MCP tools (thread::scope) → sequential for MCP → feed results back → repeat until no more tool calls
5. **Context Management**: estimate_tokens(), compact_messages() with compaction_budget_tokens — summarizes old turns when context grows too large
6. **Drift Scoring**: Tracks agent behavior drift (DriftState) — monitors for repetitive patterns, excessive tool calls, approval request accumulation
7. **Reminders**: ReminderState tracks approval_required_count, generates behavioral nudges (e.g., "Combine or batch work" when too many approvals)
8. **Session Persistence**: Turns saved to capsule as SessionTurn, loadable across sessions

### Retrieval Pipeline (query.rs — 1020 lines)
1. **Expansion**: Optional expansion hook transforms query into multiple sub-queries
2. **Lexical Lane**: BM25 search over capsule frames
3. **Vector Lane**: HNSW approximate nearest neighbor search over embeddings
4. **Fusion**: Reciprocal Rank Fusion (RRF) merges lexical + vector results
5. **Rerank**: Optional rerank hook (external model) re-scores merged results
6. **Blend**: Final scoring with feedback_weight adjustment
7. **Context Pack**: Builds prompt-ready context from top results with citations
8. **Time-travel**: `asof` parameter enables "what did the agent know at time T?" queries

### Claude/LLM Bridge (claude.rs)
- Converts internal AgentMessage format to Anthropic message schema
- Supports tool_use blocks (input/output) and image blocks
- Retry: status-aware + retry-after header + jitter backoff
- Fallback chain: ANTHROPIC_MODEL → ANTHROPIC_FALLBACK → Vertex proxy
- Hook routing: builtin (call_claude) or external CLI hook (run_claude_hook)

### Configuration System
- Config stored as capsule frames at aethervault://config/<key>.json
- Main config: config/aethervault.json (models, providers, agent settings, MCP servers)
- Auth profiles: config/auth-profiles.json (API key management)
- System prompt: config/system-prompt.md (injected as system message)
- OAuth tokens: stored in capsule as oauth.google, oauth.microsoft
- Hook spec resolution: supports inline commands, env var references, CLI commands with timeout

### Bridges
- **Telegram** (1066 lines): Long-polling via getUpdates, session per chat_id, inline approval (approve/reject commands in chat), photo/document support, completion events
- **Webhook** (220 lines): HTTP server via tiny_http, JSON body → agent invocation → response
- **WhatsApp** (102 lines): Stub implementation

### Infrastructure
- **OAuth Broker**: Built-in HTTP server for Google/Microsoft OAuth flows, token exchange, refresh
- **Filesystem Sandbox**: fs_list/fs_read/fs_write restricted to workspace + allowed_fs_roots
- **Knowledge Graph**: Loaded from file, entities matched against conversation context, injected into system prompt
- **Triggers**: Cron, email (Gmail query), calendar_free (window), webhook (URL polling) — stored in capsule
- **DigitalOcean Scale**: Status monitoring, droplet resize (with approval)
## AETHERVAULT CODEBASE DEEP SCAN (2026-02-15) — Codex Agent Analysis

## 7 Autonomous Triggers Active:
1. daily-memory-consolidation (3am) — dedupe, importance scoring, prune stale entries
2. nightly-knowledge-gap-research (1am) — identify gaps, browser research, store findings
3. hourly-infra-health-check (every hour) — check all services, restart if down
4. weekly-skill-review (Sun 7am) — audit skills, extract new ones from sessions
5. weekly-self-eval-report (Sun 6pm) — metrics, trends, improvement plan
6. daily-knowledge-graph-update (4:15am) — entity extraction, dedup, pruning
7. pre-session-warmup (7:55am weekdays) — calendar, priorities, briefing prep

## 10 Core Skills Stored:
codex-cli-invocation, raodesktop-ssh-access, macos-vm-management, browser-automation, content-generation-pipeline, infrastructure-health-check, codex-swarm-pattern, memory-self-scan, post-session-reflection, subagent-config-registration

## 10-Agent Swarm Design Docs at /tmp/swarm-output/:
01-architecture.md (394 lines), 02-memory.md (329 lines), 03-triggers.md (51 lines), 04-skills.md (137 lines), 05-research.md (326 lines), 06-reflection.md (256 lines), 07-compute.md (138 lines), 08-evaluation.md (265 lines), 09-knowledge-graph.md (301 lines), 10-master-plan.md (243 lines). Total: 2,440 lines / 107KB.

## Key Architecture Decisions:
- Codex swarm via parallel exec calls (NOT subagent_batch — config doesn't hot-reload)
- 5 always-on services: Memory Consolidation, Research Engine, Skill Synthesis, KG Growth, Self-Evaluation
- Event-driven control plane with topic classes: session.lifecycle, consolidation.jobs, research.requests, skill.proposals, kg.updates, eval.results
- Three-phase rollout: Phase 1 (triggers+skills) DONE, Phase 2 (memory consolidation+reflection loops) THIS WEEK, Phase 3 (GPU compute+fine-tuning) THIS MONTH
## COST-OPTIMIZED COMPUTE ARCHITECTURE (2026-02-15)

### Principle: MINIMIZE Anthropic API tokens, MAXIMIZE free Codex CLI + local Ollama

### Compute Tiers (cheapest first):
1. **LOCAL OLLAMA (FREE, fastest)** — raoDesktop RTX 3090 24GB VRAM, 40GB RAM, 32 cores, CUDA 12.6, Ollama v0.15.2
   - Models downloading: qwen3:8b, qwen3:14b, qwen3:32b, nomic-embed-text
   - Use for: Memory consolidation, dedup, importance scoring, entity extraction, KG updates, embeddings, classification, summarization, skill review
   - Access: ssh -p 2222 sunil@localhost → ollama API at 127.0.0.1:11434
   - 24GB VRAM fits qwen3:32b (Q4 ~20GB) or qwen3:14b with huge context

2. **CODEX CLI (FREE, powerful)** — gpt-5.3-codex-spark via `codex exec --dangerously-bypass-approvals-and-sandbox`
   - Use for: Research tasks, code analysis, complex reasoning, multi-step planning, content generation
   - Can spin up internal swarms for parallelism
   - Fire-and-forget pattern: nohup codex exec ... &

3. **ANTHROPIC API ($$, only when necessary)** — Claude via AetherVault agent loop
   - Use ONLY for: Direct user conversations, real-time interactive sessions
   - NEVER use for: Background autonomous tasks, batch processing, consolidation

### Autonomous Task Routing:
- daily-memory-consolidation → Ollama qwen3:14b (local)
- nightly-knowledge-gap-research → Codex CLI swarm (free)
- hourly-infra-health-check → Shell scripts only, no LLM needed
- weekly-skill-review → Codex CLI (free)
- weekly-self-eval-report → Codex CLI (free)
- daily-knowledge-graph-update → Ollama qwen3:8b for entity extraction (local)
- pre-session-warmup → Ollama qwen3:8b for summarization (local)

### raoDesktop GPU Details:
- GPU: NVIDIA GeForce RTX 3090 (24GB VRAM, 22.8GB available)
- CUDA: 12.6, Compute Capability 8.6
- Driver: 560.94 (WSL2 passthrough via /dev/dxg)
- nvidia-smi path: /usr/lib/wsl/lib/nvidia-smi
- Ollama: v0.15.2, serving on 127.0.0.1:11434, GPU detected as CUDA0
COST-OPTIMIZED COMPUTE DEPLOYED (2026-06-15). All autonomous tasks routed to free compute. Tier 1: Ollama on raoDesktop RTX 3090 (24GB VRAM) — pulling qwen3:8b/14b/32b + nomic-embed-text. Tier 2: Codex CLI (free gpt-5.3). Tier 3: Anthropic API (user conversations only). Ollama started on raoDesktop (PID 8328, port 11434, GPU detected). Model downloads running in background.
SELF-EVOLUTION SYSTEM DEPLOYED (2026-02-15). Two-part system:

## Part 1: Lock Contention Fix (CODE CHANGES APPLIED, BUILD IN PROGRESS)
Source: /root/.aethervault/rust-src/

### aether-core lockfile.rs changes:
- DEFAULT_TIMEOUT_MS: 250 → 5000
- AETHERVAULT_LOCK_TIMEOUT_MS env var override
- Exponential backoff with ±30% jitter (5ms → 200ms, doubling) replaces fixed 10ms spin
- Logs warning when lock acquisition takes >1 second

### aether-core lock.rs changes:
- MAX_ATTEMPTS: 200 → 400
- Exponential backoff with jitter (10ms start, 500ms cap, ±30% jitter) replaces fixed 50ms
- AETHERVAULT_LOCK_TIMEOUT_MS env var override (converts timeout to attempt count)
- lock_attempts_from_timeout() helper function

### main.rs changes:
- with_write_mem: Added is_lock_error() detection + retry up to 3 times with exponential backoff (500ms, 1s, 2s)
- subagent_batch: 150ms stagger between thread::spawn calls to reduce thundering herd
- subagent_invoke: Comment documenting no stagger needed (parent releases locks before spawn)

## Part 2: Self-Evolution Pipeline (DEPLOYED)
Location: /opt/av-evolution/ (9 scripts, 689 lines total)

### Scripts:
- evolve.sh (157 lines) — Master orchestrator with flock, run IDs, logging
- review.sh (78 lines) — Codex CLI code review, outputs proposals.json
- propose.sh (87 lines) — Codex CLI code generation, outputs patches/
- classify.sh (58 lines) — Core-binary vs scaffolding classification
- build.sh (86 lines) — cargo build --release with stash/revert on failure
- deploy.sh (110 lines) — Canary test → deploy → health check → auto-rollback
- health-check.sh (47 lines) — Standalone health verification
- rollback.sh (51 lines) — Manual rollback to any .bak binary
- cron-install.sh (15 lines) — Installs 2am daily cron

### Fault Tolerance:
- Canary binary testing before swap
- Post-deploy 15s health check
- Automatic rollback on failure
- Binary backups: /usr/local/bin/aethervault.bak.{timestamp} (keeps last 5)
- Feature branches for code changes, push to GitHub after successful deploy
- Lock file prevents concurrent evolution runs
- Max 1 binary change per night

## Build Infrastructure:
- Rust nightly 1.95.0 installed on AV droplet via rustup
- cargo 1.95.0-nightly available at /root/.cargo/bin/cargo
- First build in progress (all deps + LTO, will be slow)
- GitHub SSH authenticated as sunilkgrao
- Repo: git@github.com:sunilkgrao/aethervault.git
- Source: /root/.aethervault/rust-src/ (main.rs + vendor/aether-core/)

## TODO after build completes:
1. Verify cargo build --release succeeds with lock contention fixes
2. Run canary test on new binary
3. Deploy new binary and restart service
4. Install cron job: /opt/av-evolution/cron-install.sh
5. Run first evolution cycle manually to validate pipeline
## REPO & CODEBASE LAYOUT (2026-06-15) — AUTHORITATIVE

### GitHub Repo
- URL: https://github.com/sunilkgrao/aethervault
- Branch: main (commit 22102d0)
- Auth: SSH key (sunilkgrao) on AV droplet, HTTPS clone on raoDesktop

### Git Root: /root/.aethervault/ (NOT rust-src/)
- The `.git` directory lives at `/root/.aethervault/.git`
- `rust-src/` is a SUBDIRECTORY containing the OLD monolith (main.rs 10,618 lines) — DO NOT USE FOR BUILDS
- The ROOT Cargo.toml at `/root/.aethervault/Cargo.toml` uses default `src/main.rs` convention (no explicit bin path)

### Source Layout (modular, 17 files):
- `/root/.aethervault/src/main.rs` (1,103 lines) — CLI dispatch
- `/root/.aethervault/src/agent.rs` (1,146) — Agent runtime
- `/root/.aethervault/src/tool_exec.rs` (2,210) — Tool execution
- `/root/.aethervault/src/services.rs` (1,579) — OAuth, approvals, triggers
- `/root/.aethervault/src/query.rs` (1,020) — Retrieval pipeline
- `/root/.aethervault/src/bridges/telegram.rs` (1,066) — Telegram bridge
- `/root/.aethervault/src/cli.rs` (856) — Clap CLI schema
- `/root/.aethervault/src/types.rs` (806) — Type definitions
- `/root/.aethervault/src/tool_defs.rs` (738) — 49 tool JSON schemas
- `/root/.aethervault/src/mcp.rs` (516) — MCP protocol
- `/root/.aethervault/src/tool_args.rs` (491) — Tool arg structs
- `/root/.aethervault/src/claude.rs` (490) — Anthropic API bridge
- `/root/.aethervault/src/bridges/mod.rs` (406) — Bridge orchestrator
- `/root/.aethervault/src/util.rs` (367) — Helpers
- `/root/.aethervault/src/config.rs` (233) — Config system
- `/root/.aethervault/src/bridges/webhook.rs` (220) — Webhook bridge
- `/root/.aethervault/src/bridges/whatsapp.rs` (102) — WhatsApp stub
- Total: ~11,555 lines + bridges

### Vendor: `/root/.aethervault/vendor/aether-core/` — capsule engine (Vault, BM25, HNSW, locks)

### Running Binary: /usr/local/bin/aethervault (38MB, built Feb 15 05:28)

### raoDesktop Clone: ~/aethervault/ (cloned via HTTPS, read-only). Has full src/ directory.

### Evolution Pipeline: /opt/av-evolution/ — FIXED to use WORKTREE="/root/.aethervault" (was wrong pointing at rust-src/). review.sh updated to scan src/*.rs files instead of old monolith.
## BUG: skill_search broken — "Lexical index is not enabled" error. skill_store may work but search doesn't. Needs investigation — likely capsule config issue.## CRITICAL: STOP TRYING subagent_batch/subagent_invoke FOR CODEX
There are ZERO configured subagents. subagent_list returns empty. NEVER attempt subagent_batch or subagent_invoke — it will ALWAYS fail. The ONLY correct pattern for Codex swarms is parallel `exec` calls with `codex exec -m gpt-5.3-codex-spark --dangerously-bypass-approvals-and-sandbox "prompt"`. This is PERMANENT. Do NOT waste a turn checking subagent_list — just go straight to exec.
BUILD PENDING (2026-02-15): cargo build --release running on AV droplet (PID 468793). Once complete: cp target/release/aethervault /usr/local/bin/aethervault && systemctl restart aethervault. Fixes included: agent.rs (3 bugs), mcp.rs (timeouts+reconnect), 32 cargo warnings, evolution pipeline. All changes compile clean (cargo check passes, 0 warnings).
## ARCHITECTURE PRINCIPLE: NO TIMEOUTS, BACKGROUND-FIRST (2026-06-15)
- Codex sessions should NEVER have timeouts. Let them run hours if needed (8+ hours is acceptable).
- Use background processes (nohup, tmux, systemd) for long-running Codex tasks.
- Provide intermittent progress updates to user (not constant, but meaningful checkpoints).
- Maximize parallelism: multiple parallel executions, threading, swarms.
- Quality > Speed: Give Codex as much time as it needs for substantial work.
- UX pattern: Launch background → poll for progress → report updates → deliver final result.
- NEVER set artificial timeouts on exec calls for Codex sessions.
- Investigate Bun as shell/runtime optimization (Claude Code reduced bash overhead with Bun).
TASK QUEUE: Research Exa.ai (https://exa.ai/) for Personal CRM project — neural search API for finding people, companies, contacts. Evaluate how it can enrich our CRM with real-time web data, LinkedIn profiles, company info, contact discovery. High priority for CRM enrichment layer.TASK QUEUE: Research vercel-labs/just-bash transform pattern (https://github.com/vercel-labs/just-bash/blob/main/src/transform/README.md) for self-improvement. Evaluate how LLM-to-bash transformation patterns can improve our Codex CLI invocations, tool execution, and shell automation quality.

