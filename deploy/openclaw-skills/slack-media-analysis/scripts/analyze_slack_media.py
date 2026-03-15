#!/usr/bin/env python3
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import mimetypes
import os
import shutil
import subprocess
import sys
import tempfile
import textwrap
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

try:
    from openai import OpenAI
except Exception:  # pragma: no cover - optional dependency at runtime
    OpenAI = None  # type: ignore[assignment]

try:
    from google.auth.transport.requests import Request as GoogleAuthRequest
    from google.oauth2 import service_account
except Exception:  # pragma: no cover - optional dependency at runtime
    GoogleAuthRequest = None  # type: ignore[assignment]
    service_account = None  # type: ignore[assignment]


INLINE_GEMINI_MAX_BYTES = 18 * 1024 * 1024
DEFAULT_MAX_KEYFRAMES = 12
DEFAULT_FRAME_EVERY_SECONDS = 3
VERTEX_SCOPE = "https://www.googleapis.com/auth/cloud-platform"
DEFAULT_GEMINI_MODEL = "gemini-3.1-pro-preview"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Analyze Slack media with preprocessing + Gemini-first reasoning")
    parser.add_argument("--input", required=True, help="Local media path or Slack/private URL")
    parser.add_argument("--output-dir", help="Directory to write artifacts into")
    parser.add_argument("--mode", choices=["generic", "bug"], default="generic")
    parser.add_argument("--question", default="", help="Optional focused question to answer")
    parser.add_argument("--frame-every-seconds", type=int, default=DEFAULT_FRAME_EVERY_SECONDS)
    parser.add_argument("--max-keyframes", type=int, default=DEFAULT_MAX_KEYFRAMES)
    return parser.parse_args()


def now_stamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")


def ensure_dir(path: Path) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    return path


def slurp_secret_file(path: Path) -> str:
    if not path.exists():
        return ""
    return path.read_text().strip()


def load_env_file_value(path: Path, key: str) -> str:
    if not path.exists():
        return ""
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        if k.strip() == key:
            return v.strip().strip('"').strip("'")
    return ""


def find_env_value(key: str) -> str:
    candidates = [
        os.environ.get(key, "").strip(),
        load_env_file_value(Path("/root/.secrets/master.env"), key),
        load_env_file_value(Path.home() / ".secrets" / "master.env", key),
        load_env_file_value(Path.cwd() / ".secrets" / "master.env", key),
    ]
    for candidate in candidates:
        if candidate:
            return candidate
    return ""


def find_slack_token() -> str:
    candidates = [
        os.environ.get("SLACK_BOT_TOKEN", "").strip(),
        slurp_secret_file(Path("/root/.openclaw/secrets/slack_bot_token")),
        slurp_secret_file(Path.home() / ".openclaw" / "secrets" / "slack_bot_token"),
        load_env_file_value(Path.home() / ".secrets" / "slack.env", "SLACK_BOT_TOKEN"),
        load_env_file_value(Path.cwd() / ".secrets" / "slack.env", "SLACK_BOT_TOKEN"),
    ]
    for candidate in candidates:
        if candidate:
            return candidate
    return ""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def is_url(value: str) -> bool:
    return value.startswith("http://") or value.startswith("https://")


def download_input(input_ref: str, artifact_dir: Path) -> Path:
    if not is_url(input_ref):
        path = Path(input_ref).expanduser().resolve()
        if not path.exists():
            raise FileNotFoundError(f"input not found: {path}")
        return path

    parsed = urllib.parse.urlparse(input_ref)
    target = artifact_dir / Path(parsed.path).name
    headers = {}
    if "slack.com" in parsed.netloc:
        token = find_slack_token()
        if not token:
            raise RuntimeError("Slack URL provided but SLACK_BOT_TOKEN was not found")
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(input_ref, headers=headers)
    with urllib.request.urlopen(request, timeout=120) as response, target.open("wb") as output:
        shutil.copyfileobj(response, output)
    return target


def run_json(cmd: list[str], output_path: Path) -> dict:
    result = subprocess.run(cmd, check=True, capture_output=True, text=True)
    output_path.write_text(result.stdout)
    return json.loads(result.stdout)


def run_text(cmd: list[str], output_path: Path) -> str:
    result = subprocess.run(cmd, check=True, capture_output=True, text=True)
    output_path.write_text(result.stdout)
    return result.stdout


def probe_media(media_path: Path, artifact_dir: Path) -> dict:
    return run_json(
        [
            "ffprobe",
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            str(media_path),
        ],
        artifact_dir / "probe.json",
    )


def extract_audio(media_path: Path, artifact_dir: Path) -> Path | None:
    audio_path = artifact_dir / "audio.wav"
    cmd = [
        "ffmpeg",
        "-y",
        "-i",
        str(media_path),
        "-vn",
        "-ac",
        "1",
        "-ar",
        "16000",
        str(audio_path),
    ]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0 or not audio_path.exists():
        return None
    return audio_path


def extract_keyframes(media_path: Path, artifact_dir: Path, every_seconds: int, max_frames: int) -> list[str]:
    keyframe_dir = ensure_dir(artifact_dir / "keyframes")
    pattern = str(keyframe_dir / "frame-%03d.jpg")
    cmd = [
        "ffmpeg",
        "-y",
        "-i",
        str(media_path),
        "-vf",
        f"fps=1/{max(1, every_seconds)},scale=1280:-1",
        "-frames:v",
        str(max_frames),
        pattern,
    ]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        return []
    return [str(path) for path in sorted(keyframe_dir.glob("frame-*.jpg"))]


def openai_transcribe(audio_path: Path) -> str:
    if not os.environ.get("OPENAI_API_KEY") or OpenAI is None:
        return ""
    client = OpenAI()
    with audio_path.open("rb") as handle:
        transcript = client.audio.transcriptions.create(
            model="gpt-4o-transcribe",
            file=handle,
        )
    text = getattr(transcript, "text", "") or ""
    return text.strip()


def deepgram_transcribe(audio_path: Path) -> str:
    api_key = find_env_value("DEEPGRAM_API_KEY")
    if not api_key:
        return ""
    request = urllib.request.Request(
        "https://api.deepgram.com/v1/listen?model=nova-2&smart_format=true&punctuate=true&detect_language=true",
        data=audio_path.read_bytes(),
        headers={
            "Authorization": f"Token {api_key}",
            "Content-Type": "audio/wav",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=300) as response:
            payload = json.load(response)
        return (
            payload["results"]["channels"][0]["alternatives"][0]["transcript"].strip()
        )
    except Exception:
        return ""


def guess_mime_type(path: Path) -> str:
    mime, _ = mimetypes.guess_type(path.name)
    return mime or "application/octet-stream"


def is_pdf(path: Path) -> bool:
    return guess_mime_type(path) == "application/pdf"


def candidate_vertex_credential_paths() -> list[Path]:
    home = Path.home()
    candidates = []
    env_path = os.environ.get("GOOGLE_APPLICATION_CREDENTIALS", "").strip()
    if env_path:
        candidates.append(Path(env_path).expanduser())
    candidates.extend(
        [
            Path("/root/.openclaw/secrets/google_application_credentials.json"),
            Path("/root/.openclaw/secrets/gemini-vertex-adc.json"),
            Path("/root/.secrets/google_application_credentials.json"),
            home / ".config" / "gcloud" / "legacy_credentials" / "bot-vertex@tribble-ai.iam.gserviceaccount.com" / "adc.json",
            home / ".config" / "gcloud" / "legacy_credentials" / "gemini-long-form-rfp@tribble-ai.iam.gserviceaccount.com" / "adc.json",
            home / ".config" / "gcloud" / "legacy_credentials" / "clawdbot-vertex@tribble-ai.iam.gserviceaccount.com" / "adc.json",
        ]
    )
    return candidates


def get_vertex_token() -> tuple[str, str]:
    if service_account is None or GoogleAuthRequest is None:
        return "", "google-auth service-account support is not available"
    last_error = "no usable Vertex credential file found"
    for path in candidate_vertex_credential_paths():
        if not path.exists():
            continue
        try:
            creds = service_account.Credentials.from_service_account_file(
                str(path),
                scopes=[VERTEX_SCOPE],
            )
            creds.refresh(GoogleAuthRequest())
            token = getattr(creds, "token", "")
            if token:
                return token, ""
        except Exception as exc:  # pragma: no cover - depends on local auth state
            last_error = f"{path}: {type(exc).__name__}: {exc}"
    return "", last_error


def build_prompt(mode: str, question: str, transcript_text: str) -> str:
    base = """
You are analyzing an inbound artifact for Linus. The artifact may be video, audio, or a raw PDF.

Return strict JSON with these keys:
- summary
- timeline
- visible_ui_states
- errors_observed
- reproduction_steps
- suspected_root_causes
- action_items
- confidence

Rules:
- Be grounded in the media only.
- If something is uncertain, say so explicitly.
- Include timestamps when possible.
- If the media is a screen recording, call out visible routes, buttons, dialogs, loaders, and error text.
- If the media includes speech, incorporate it into the summary.
- If the artifact is a PDF, summarize the document directly and cite the exact sections or visible facts you can ground in the file.
"""
    if mode == "bug":
        base += """
- Treat this as a product/debugging clip.
- Focus on what the user is trying to do, where it fails, and what likely subsystem is implicated.
"""
    if transcript_text:
        base += f"\nSupplemental transcript:\n{transcript_text[:12000]}\n"
    if question:
        base += f"\nSpecific question to answer:\n{question}\n"
    return textwrap.dedent(base).strip()


def inline_media_part(media_path: Path, *, camel_case: bool) -> dict:
    file_size = media_path.stat().st_size
    if file_size > INLINE_GEMINI_MAX_BYTES:
        raise ValueError(
            f"media file is {file_size} bytes; inline Gemini analysis limit is {INLINE_GEMINI_MAX_BYTES} bytes"
        )
    mime_type = guess_mime_type(media_path)
    media_b64 = base64.b64encode(media_path.read_bytes()).decode("ascii")
    key = "inlineData" if camel_case else "inline_data"
    mime_key = "mimeType" if camel_case else "mime_type"
    return {
        key: {
            mime_key: mime_type,
            "data": media_b64,
        }
    }


def parse_model_payload(payload: dict) -> tuple[dict | None, str]:
    text = ""
    try:
        parts = payload["candidates"][0]["content"]["parts"]
        text = "".join(part.get("text", "") for part in parts)
    except Exception:
        return None, f"unexpected model response shape: {json.dumps(payload)[:1000]}"
    text = text.strip()
    if not text:
        return None, "model returned empty content"
    try:
        return json.loads(text), ""
    except json.JSONDecodeError:
        return {"raw_text": text}, ""


def vertex_generate(media_path: Path, prompt: str) -> tuple[dict | None, str]:
    token, token_error = get_vertex_token()
    if not token:
        return None, token_error
    try:
        media_part = inline_media_part(media_path, camel_case=True)
    except ValueError as exc:
        return None, str(exc)
    project = os.environ.get("VERTEX_PROJECT_ID", "tribble-ai").strip() or "tribble-ai"
    location = os.environ.get("VERTEX_LOCATION", "us-central1").strip() or "us-central1"
    model = (
        os.environ.get("GEMINI_VERTEX_MODEL", "").strip()
        or os.environ.get("GEMINI_MODEL", "").strip()
        or DEFAULT_GEMINI_MODEL
    )
    body = {
        "contents": [
            {
                "role": "user",
                "parts": [
                    {"text": prompt},
                    media_part,
                ]
            }
        ],
        "generationConfig": {
            "responseMimeType": "application/json",
        },
    }
    base_url = (
        "https://aiplatform.googleapis.com"
        if location == "global"
        else f"https://{location}-aiplatform.googleapis.com"
    )
    request = urllib.request.Request(
        f"{base_url}/v1/projects/{project}/locations/{location}/publishers/google/models/{model}:generateContent",
        data=json.dumps(body).encode("utf-8"),
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {token}",
        },
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=300) as response:
        payload = json.load(response)
    return parse_model_payload(payload)


def gemini_generate(media_path: Path, prompt: str) -> tuple[dict | None, str]:
    api_key = find_env_value("GEMINI_API_KEY") or find_env_value("GOOGLE_API_KEY")
    if not api_key:
        return None, "GEMINI_API_KEY not set"
    try:
        media_part = inline_media_part(media_path, camel_case=False)
    except ValueError as exc:
        return None, str(exc)
    model = os.environ.get("GEMINI_MODEL", "").strip() or DEFAULT_GEMINI_MODEL
    body = {
        "contents": [
            {
                "parts": [
                    {"text": prompt},
                    media_part,
                ]
            }
        ],
        "generationConfig": {
            "responseMimeType": "application/json",
        },
    }
    request = urllib.request.Request(
        f"https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}",
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=300) as response:
        payload = json.load(response)
    return parse_model_payload(payload)


def model_generate(media_path: Path, prompt: str) -> tuple[dict | None, str]:
    analysis, error = vertex_generate(media_path, prompt)
    if analysis is not None:
        return analysis, ""
    gemini_analysis, gemini_error = gemini_generate(media_path, prompt)
    if gemini_analysis is not None:
        return gemini_analysis, ""
    return None, f"Vertex unavailable ({error}); Gemini API key path unavailable ({gemini_error})"


def write_report(
    artifact_dir: Path,
    input_ref: str,
    media_path: Path,
    probe: dict,
    transcript_text: str,
    keyframes: list[str],
    analysis: dict | None,
    analysis_error: str,
) -> None:
    report_path = artifact_dir / "report.md"
    lines = [
        "# Inbound Artifact Analysis",
        "",
        f"- Source input: `{input_ref}`",
        f"- Local media path: `{media_path}`",
        f"- SHA256: `{sha256_file(media_path)}`",
        f"- MIME type: `{guess_mime_type(media_path)}`",
        f"- Size bytes: `{media_path.stat().st_size}`",
        "",
        "## Artifact Probe",
        "",
        "```json",
        json.dumps(probe, indent=2),
        "```",
        "",
    ]
    if transcript_text:
        lines.extend(
            [
                "## Transcript",
                "",
                transcript_text.strip(),
                "",
            ]
        )
    if keyframes:
        lines.extend(
            [
                "## Keyframes",
                "",
                *(f"- `{Path(frame).name}`" for frame in keyframes),
                "",
            ]
        )
    if analysis is not None:
        lines.extend(
            [
                "## Model Analysis",
                "",
                "```json",
                json.dumps(analysis, indent=2),
                "```",
                "",
            ]
        )
    if analysis_error:
        lines.extend(
            [
                "## Analysis Warning",
                "",
                analysis_error,
                "",
            ]
        )
    report_path.write_text("\n".join(lines))


def main() -> int:
    args = parse_args()
    artifact_dir = ensure_dir(
        Path(args.output_dir).expanduser().resolve()
        if args.output_dir
        else Path(tempfile.gettempdir()) / f"slack-media-analysis-{now_stamp()}"
    )

    media_path = download_input(args.input, artifact_dir)
    if is_pdf(media_path):
        probe = {
            "format": {
                "filename": media_path.name,
                "format_name": "pdf",
                "size": media_path.stat().st_size,
                "mime_type": guess_mime_type(media_path),
            },
            "streams": [],
        }
        (artifact_dir / "probe.json").write_text(json.dumps(probe, indent=2))
    else:
        probe = probe_media(media_path, artifact_dir)

    audio_path = None if is_pdf(media_path) else extract_audio(media_path, artifact_dir)
    keyframes = []
    if any(stream.get("codec_type") == "video" for stream in probe.get("streams", [])):
        keyframes = extract_keyframes(media_path, artifact_dir, args.frame_every_seconds, args.max_keyframes)

    transcript_text = ""
    if audio_path is not None:
        transcript_text = deepgram_transcribe(audio_path) or openai_transcribe(audio_path)
        if transcript_text:
            (artifact_dir / "transcript.txt").write_text(transcript_text)

    prompt = build_prompt(args.mode, args.question, transcript_text)
    analysis, analysis_error = model_generate(media_path, prompt)
    if analysis is not None:
        (artifact_dir / "analysis.json").write_text(json.dumps(analysis, indent=2))
    else:
        (artifact_dir / "analysis.json").write_text(json.dumps({"error": analysis_error}, indent=2))

    manifest = {
        "created_at": datetime.now(timezone.utc).isoformat(),
        "input_ref": args.input,
        "media_path": str(media_path),
        "probe_path": str(artifact_dir / "probe.json"),
        "audio_path": str(audio_path) if audio_path else "",
        "transcript_path": str(artifact_dir / "transcript.txt") if transcript_text else "",
        "keyframes": keyframes,
        "analysis_path": str(artifact_dir / "analysis.json"),
        "question": args.question,
        "mode": args.mode,
        "analysis_error": analysis_error,
    }
    (artifact_dir / "manifest.json").write_text(json.dumps(manifest, indent=2))
    write_report(artifact_dir, args.input, media_path, probe, transcript_text, keyframes, analysis, analysis_error)

    print(json.dumps({"artifact_dir": str(artifact_dir), "analysis_error": analysis_error}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
