# Multimodal Media Understanding SOTA

Snapshot date: March 15, 2026

## Goal

Linus should be able to ingest Slack media such as:

- MP4 screen recordings
- voice notes and audio clips
- video clips with speech
- short product demos
- bug-report recordings

and turn them into:

- a reliable synopsis
- a timestamped timeline
- extracted text/transcript
- visible UI states and errors
- likely reproduction steps
- actionable troubleshooting notes

## Bottom line

Do not build this as "sample a frame every 3 seconds and hope for the best."

That is a useful fallback, but not the primary architecture.

The best March 2026 production approach is:

1. native multimodal video understanding for first-pass comprehension
2. transcript + keyframes + timestamps as persistent evidence
3. targeted higher-fps / clipped re-analysis for tricky UI bug videos
4. structured outputs stored in Linus memory/search

## What looks strongest right now

### 1. Best general-purpose default: Gemini video understanding

Gemini is the strongest general API choice if you want one backend that can directly ingest video, reason over both visual and audio content, return timestamps, and let you control clipping and frame sampling.

Why it stands out:

- native video input in the Gemini API
- direct support for audio + visual reasoning from one video asset
- timestamp-aware prompting
- custom clipping intervals
- configurable frame sampling (`fps`)
- long-context handling for long videos

Operationally important details from Google:

- default video sampling is `1 FPS`
- you can override sampling with custom `fps`
- you can clip videos by time range before analysis
- videos can be uploaded through the Files API and reused
- Google documents support for videos up to about `1 hour` at default media resolution or `3 hours` at low media resolution on `1M` context models

Why this matters for Linus:

- short bug videos can be analyzed directly
- UI troubleshooting videos can be rerun at higher `fps`
- long internal recordings can be clipped and summarized in pieces

### 2. Best specialized video platform: TwelveLabs

If you want a dedicated "video memory/search" layer, TwelveLabs is the strongest specialized option.

Why it stands out:

- purpose-built video indexing and retrieval
- multimodal indexing across visuals, speech, sounds, and OCR
- separate models for:
  - `Marengo` for search and embeddings
  - `Pegasus` for analysis and text generation
- structured search/retrieval over large video corpora

Why this matters for Linus:

- if Slack/video volume becomes large, TwelveLabs is better than re-analyzing every old clip from scratch
- it is well suited for:
  - "find the screen recording where Ray showed the editable long answer bug"
  - "find clips where someone mentions Jira sync failures"
  - "show all videos where this workflow crashes after upload"

### 3. Viable but not my first choice for this use case: Amazon Nova

Amazon Nova can do video understanding, but AWS explicitly documents a major limitation:

- Nova video understanding does **not** process audio tracks in video

That makes it weaker for Slack bug videos and voice-heavy walkthroughs unless you add a separate audio/transcript pipeline.

Nova is more attractive if:

- you are already deep in Bedrock
- the videos are mostly visual
- you are fine splitting video understanding from audio understanding

### 4. Useful secondary tools, but not the primary video backend

#### OpenAI

OpenAI is strong for:

- audio transcription
- diarization-style speech workflows
- image/frame reasoning
- follow-up reasoning over extracted transcripts and screenshots

But the current official audio model pages explicitly say:

- `Video: Not supported`

So OpenAI is useful in this architecture, but not as the primary native video-ingest layer.

#### Anthropic

Anthropic is strong for:

- reasoning over extracted evidence
- image understanding
- PDF analysis

But the current official docs expose image and PDF multimodality, not first-class native video input. So Claude is better as a second-pass reasoner over transcript + keyframes + screenshots than as the main video parser.

## Recommendation for Linus

### Recommendation in one sentence

Use Gemini as the primary video parser, optionally add TwelveLabs as the scalable video index, and use transcript + keyframes as the durable evidence layer.

## Recommended pipeline

### Layer 1: Raw evidence capture

For every Slack media asset, store:

- raw Slack file metadata
- original file
- thread context
- sender
- channel
- timestamp

Never treat model output as the only record.

### Layer 2: Media preprocessing

Before deeper reasoning:

1. extract container metadata with `ffprobe`
2. extract audio track with `ffmpeg`
3. generate transcript
4. detect shot boundaries / scene changes
5. extract keyframes
6. keep frame timestamps

For audio transcription, OpenAI audio models or another strong speech stack are fine.

### Layer 3: Primary analysis

#### For short videos and bug recordings

Use Gemini directly on the video.

Prompt for:

- what is happening
- timestamps of meaningful moments
- visible UI states
- errors/toasts/dialogs
- likely user intention
- likely system failure point
- exact reproduction sequence

For UI bug videos:

- do not stay on default `1 FPS` if tiny UI changes matter
- rerun the critical segment at higher `fps`
- clip around the suspected failure moment instead of reprocessing the whole video

#### For large-scale video archives

Index videos with TwelveLabs.

Use:

- `Marengo` to search across many videos
- `Pegasus` to generate summaries or answer targeted questions

This is better than brute-force re-analysis if Linus needs long-term searchable video memory.

### Layer 4: Structured outputs

Store outputs as structured JSON, not only prose.

Suggested schema:

- `summary`
- `transcript`
- `timeline`
- `visible_entities`
- `ui_states`
- `errors_observed`
- `action_items`
- `suspected_root_causes`
- `keyframes`
- `confidence`
- `source_file_id`
- `source_channel`
- `source_message_ts`

### Layer 5: Second-pass reasoning

After primary parsing, optionally run a second model pass over:

- transcript
- keyframes
- Gemini/TwelveLabs synopsis
- surrounding Slack thread

This second pass is where Claude or another strong reasoner can help produce:

- a cleaner diagnosis
- a concise explanation for Slack
- a PR-quality bug summary
- Jira-ready reproduction steps

## How I would answer your frame-sampling question

Your instinct is right that multimodal models should be central.

My recommendation is:

- **do not** make "sample every 3 seconds" the main architecture
- **do** use native multimodal video models first
- **do** keep frame extraction as a controllable fallback and evidence layer

Frame sampling alone is too weak because:

- it loses audio
- it loses motion cues
- it can miss short UI transitions
- it cannot reliably recover temporal relationships

But frame extraction is still important for:

- screenshots in bug reports and PR comments
- OCR-heavy UI debugging
- high-detail rechecks when the main model misses a subtle state change
- caching evidence so you do not repeatedly pay to reprocess the full video

## Practical design for Linus

### MVP

Build this first:

1. Slack file ingestion
2. `ffprobe` + `ffmpeg` preprocessing
3. transcript generation
4. Gemini video analysis
5. structured JSON result
6. keyframe extraction
7. Slack-ready summary + timestamped findings

### V2

Add:

1. TwelveLabs index for long-term searchable video memory
2. second-pass reasoning over transcript + keyframes + thread context
3. automatic bug-report artifact pack:
   - summary
   - reproduction steps
   - screenshots
   - timestamps
   - likely owner/system

### V3

Add:

1. automatic classification:
   - bug
   - workflow confusion
   - UI regression
   - integration failure
   - infra incident
2. entity extraction:
   - product area
   - account/customer
   - feature flag
   - UI route
3. retrieval over all historical media

## Final recommendation

If I were implementing this now, I would do:

- Gemini for primary video understanding
- OpenAI audio/transcription where needed
- keyframe + transcript + metadata persistence in the evidence layer
- optional TwelveLabs once media volume/search requirements justify it

That gives Linus:

- strong comprehension now
- good debugging for bug videos
- durable evidence
- a clear path to searchable media memory later

## Primary sources

- Google Gemini video understanding: https://ai.google.dev/gemini-api/docs/video-understanding
- Google Gemini changelog: https://ai.google.dev/gemini-api/docs/changelog
- Google Gemini Files API: https://ai.google.dev/api/files
- TwelveLabs models: https://docs.twelvelabs.io/docs/concepts/models
- TwelveLabs analyze videos: https://docs.twelvelabs.io/docs/guides/analyze-videos
- TwelveLabs release notes: https://docs.twelvelabs.io/docs/get-started/release-notes
- Amazon Nova multimodal understanding: https://docs.aws.amazon.com/nova/latest/nova2-userguide/using-multimodal-models.html
- Amazon Nova video limitations: https://docs.aws.amazon.com/nova/latest/userguide/prompting-vision-limitations.html
- Amazon Nova audio understanding: https://docs.aws.amazon.com/nova/latest/userguide/modalities-audio.html
- OpenAI models: https://developers.openai.com/api/docs/models
- OpenAI audio model page (`Video: Not supported`): https://platform.openai.com/docs/models/gpt-audio-mini
- Anthropic vision docs: https://platform.claude.com/docs/en/build-with-claude/vision
- Anthropic files docs: https://docs.anthropic.com/en/docs/build-with-claude/files
