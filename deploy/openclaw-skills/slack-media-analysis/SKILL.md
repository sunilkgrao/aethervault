---
name: slack-media-analysis
description: Analyze screenshots, MP4/video/audio attachments, raw PDFs, screen recordings, and voice notes from Slack, Telegram, and other inbound channels with preprocessing, transcript extraction, keyframes, thread context, and Gemini-first multimodal reasoning. Use when Sunil asks Linus what is happening in a recording, screenshot, PDF, voice note, or other media evidence.
allowed-tools: Bash, Read, Write
---

# Slack Media Analysis

Use this skill when a Slack thread, Telegram chat, or other inbound channel contains media evidence:

- screenshots
- MP4 screen recordings
- short product demos
- voice notes
- audio clips
- videos with narration
- raw PDFs

Goals:

- produce a grounded synopsis
- extract timestamps and visible states
- preserve transcript/keyframes as evidence
- identify likely errors, reproduction steps, and next actions

## Sender gate

Only respond by default when the triggering sender is Sunil Rao:

- email: `sunil@tribble.ai`
- Slack user: `U0528KFHAE8`

If the sender is anyone else or identity is ambiguous, stay silent unless Sunil explicitly directs Linus to engage.

## Backend policy

Default architecture:

1. preserve the raw Slack file and message context
2. preprocess with `ffprobe` / `ffmpeg`
3. transcribe audio when possible
4. use Gemini on Vertex as the primary multimodal analyzer when Vertex credentials are available
5. fall back to direct Gemini API key auth only if Vertex auth is unavailable
6. save structured JSON and a Markdown report

Do not rely on "sample every 3 seconds and guess" as the only path.

Keyframes are evidence and fallback, not the primary understanding engine.

## Thread packet first

When the ask starts from a Slack thread, build the packet first:

```bash
python3 /root/.openclaw/workspace/skills/slack-media-analysis/scripts/build_slack_media_packet.py \
  --channel <CHANNEL_ID> \
  --thread-ts <THREAD_TS> \
  --out /tmp/slack-media-<thread-ts>
```

That packet preserves:

- thread context
- raw media metadata
- raw document metadata
- `ffprobe` output
- transcript when available
- keyframes for video

For a local smoke test:

```bash
python3 /root/.openclaw/workspace/skills/slack-media-analysis/scripts/build_slack_media_packet.py \
  --file /path/to/local/video.mp4 \
  --title "local-smoke-test" \
  --out /tmp/local-media-smoke
```

## Inputs

The main script accepts:

- a local media path
- a Slack private download URL
- an optional focused question
- an optional `bug` mode for UI / troubleshooting clips

## Script

Primary helper:

```bash
python3 /root/.openclaw/workspace/skills/slack-media-analysis/scripts/analyze_slack_media.py \
  --input "<local-path-or-slack-url>" \
  --mode bug \
  --question "What is failing in this recording, and what are the likely reproduction steps?"
```

Local repo path:

```bash
python3 deploy/openclaw-skills/slack-media-analysis/scripts/analyze_slack_media.py \
  --input "<local-path-or-slack-url>" \
  --mode bug \
  --question "What is failing in this recording, and what are the likely reproduction steps?"
```

## Output

The script writes an artifact directory with:

- `analysis.json`
- `report.md`
- `probe.json`
- `transcript.txt` when available
- `keyframes/` when available

For PDFs, the raw file is sent directly to Gemini. There is no `ffprobe`/audio/keyframe stage, but the same analysis/report artifacts are still written.

When the packet builder is used first, also expect:

- `manifest.json`
- `messages.json`
- `summary.txt`
- `files/<id-or-name>/metadata.json`

The report should capture:

- summary
- timeline
- visible UI states
- errors observed
- likely reproduction steps
- likely root causes
- confidence

## Environment

Preferred:

- Vertex auth through a service-account credential file or `GOOGLE_APPLICATION_CREDENTIALS`
- default local project: `tribble-ai`
- default Vertex location: `global`
- default frontier model: `gemini-3.1-pro-preview` unless explicitly overridden

Fallback:

- `GEMINI_API_KEY` or `GOOGLE_API_KEY` for direct Gemini API access

Optional:

- `OPENAI_API_KEY` for audio transcription fallback
- `DEEPGRAM_API_KEY` for audio transcription fallback
- `SLACK_BOT_TOKEN` when the input is a private Slack file URL

The script also checks common local secret file locations for the Slack bot token and known local Vertex credential paths.

## DS9 / product bug workflow

If the media is a DS9 / Tribble bug recording:

1. run this skill first
2. preserve the evidence and extract the likely failing UI step
3. then route code/debug work through `ds9-triage`
4. if a PR exists and Sunil asks whether it works, route real local validation through `ds9-pr-testing`

## Shared-channel behavior

In shared Slack threads:

- do not post internal file paths, private Slack URLs, or raw infra details
- summarize the evidence in plain language
- attach screenshots only after the evidence is actually grounded
- do not call it fixed just because the model produced a plausible summary
- if Gemini is unavailable, say the analysis was packet-based rather than pretending it was full native-video analysis

Use labels like:

- `media analyzed`
- `timeline extracted`
- `backend validated locally`
- `fully locally tested`

## Example questions

- `What is actually happening in this bug video?`
- `What is happening in this screenshot thread?`
- `Where does the workflow fail?`
- `What error text is visible, and at what timestamp?`
- `What does the voice note actually say?`
- `What are the likely reproduction steps from this recording?`
