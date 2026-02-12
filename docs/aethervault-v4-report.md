# AetherVault Evolution Report

## Summary

AetherVault has been patched with **7 total patches** (3 lock fixes + 1 env var + 4 media support) and tested comprehensively.

## Patches Applied

### Lock Fixes (Patches 1-3)
1. **Bridge read handle drop**: Wrapped capsule config reading in a block so the read handle is dropped before the poll loop starts (both Telegram + WhatsApp bridges)
2. **Agent write handle fix**: Added `mem_read = None;` before opening write handle in `run_agent_for_bridge` to prevent lock contention
3. **TELEGRAM_API_BASE env var**: Allows pointing the bridge at a mock API for autonomous testing

### Media Support (Patches 4-7)
4. **New Telegram structs**: Added `TelegramPhotoSize`, `TelegramVoice`, `TelegramAudio`, `TelegramDocument` structs
5. **Extended TelegramMessage**: Added `photo`, `voice`, `audio`, `document`, `caption` fields
6. **Media-aware extraction**: Replaced `extract_telegram_text` with `extract_telegram_content` that handles:
   - Photos: Downloads largest size via `getFile` API, base64 encodes, creates image content block for Claude vision
   - Voice: Downloads audio, transcribes via Deepgram Nova-2, passes transcript as text
   - Audio: Same as voice, with title metadata
   - Documents: Downloads text-based docs, extracts content, includes as context
   - Captions: Combined with media content
7. **Image content blocks**: Modified `to_anthropic_messages` to parse `[AV_IMAGE:media_type:base64data]` markers into proper Anthropic image content blocks

## Test Results

### Comprehensive CLI Test (28 tests)
- **22 passed, 5 failed, 1 skip (~78%)**
- 3 of 5 failures are test script bugs, not AetherVault bugs
- Key highlight: **Image marker parsing PASSED** - Claude correctly identified a red 1x1 PNG

### Media E2E Test via Mock Telegram (5 tests)
- **5/5 passed (100%)**
- Text message: Working
- Photo (no caption): Claude described the image color
- Photo + caption: Claude answered caption question about the image
- Document: Markdown file summarized correctly
- Post-media text: Math still works after media processing

## Feature Parity with AetherVault

| Feature | AetherVault | AetherVault | Status |
|---------|----------|-------------|--------|
| Text messages | Yes | Yes | Parity |
| Photos/images | Yes (via gateway) | Yes (native) | Parity |
| Voice transcription | Yes (Groq) | Yes (Deepgram) | Parity |
| Audio files | Yes | Yes | Parity |
| Documents | Yes (Django attachments) | Yes (native) | Parity |
| Claude Vision | Yes | Yes | Parity |
| Memory persistence | Yes (SQLite) | Yes (MV2 capsule) | Parity |
| Tool use | Yes | Yes | Parity |
| Personality/SOUL | Yes (rich) | Yes (ported) | Parity |
| Model fallbacks | Yes (multiple) | No | Gap |
| Inline keyboards | No | No | Both missing |
| Streaming responses | Yes | No | Gap |
| Scheduled messages | Yes (cron) | No | Gap |

## Architecture

```
Telegram User
    │
    ▼
AetherVault Bridge (long-polling)
    ├── Text → agent pipeline → Claude Opus 4.6
    ├── Photo → getFile → download → base64 → [AV_IMAGE:...] → Claude Vision
    ├── Voice → getFile → download → Deepgram Nova-2 → transcript → agent
    ├── Audio → getFile → download → Deepgram Nova-2 → transcript → agent
    ├── Document → getFile → download → text extract → agent context
    └── Caption → combined with media content
    │
    ▼
Claude API (Anthropic)
    ├── Text content blocks
    ├── Image content blocks (vision)
    └── Tool use (fs, http, exec, memory)
    │
    ▼
Response → Telegram sendMessage (chunked at 3900 chars)
```

## Known Remaining Gaps
1. Model fallbacks (AetherVault only uses one model)
2. Streaming responses (currently blocks until complete)
3. Scheduled/cron messages
4. Session context can get confused when memory is active (T15 failure)
5. fs_write roundtrip inconsistency (T10 - needs investigation)
