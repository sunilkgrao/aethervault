#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import mimetypes
import os
import re
import shutil
import subprocess
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


DEFAULT_FRAME_INTERVAL_SECS = 3.0
DEFAULT_MAX_FRAMES = 40


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build a deterministic evidence packet for Slack media or a local media file."
    )
    parser.add_argument("--channel", help="Slack channel ID, e.g. C09DGSTL5B5")
    parser.add_argument("--thread-ts", help="Slack thread timestamp")
    parser.add_argument("--file", help="Local file path to ingest instead of Slack")
    parser.add_argument("--title", default="", help="Optional title for local file mode")
    parser.add_argument("--out", required=True, help="Output directory")
    parser.add_argument("--token-file", default="", help="Optional Slack bot token file")
    parser.add_argument(
        "--frame-interval-secs",
        type=float,
        default=DEFAULT_FRAME_INTERVAL_SECS,
        help="Seconds between extracted frames for video evidence",
    )
    parser.add_argument(
        "--max-frames",
        type=int,
        default=DEFAULT_MAX_FRAMES,
        help="Maximum number of extracted frames per video",
    )
    args = parser.parse_args()
    if not args.file and not (args.channel and args.thread_ts):
        parser.error("Provide either --file or both --channel and --thread-ts")
    return args


def ensure_cmd(name: str) -> str:
    path = shutil.which(name)
    if not path:
        raise RuntimeError(f"required command not found: {name}")
    return path


def safe_slug(value: str) -> str:
    value = value.strip() or "untitled"
    value = re.sub(r"[^A-Za-z0-9._-]+", "-", value)
    return value[:96].strip("-") or "untitled"


def read_slack_token(token_file: str) -> str:
    if os.environ.get("SLACK_BOT_TOKEN"):
        return os.environ["SLACK_BOT_TOKEN"].strip()
    candidates = [
        Path(token_file) if token_file else None,
        Path("/root/.openclaw/secrets/slack_bot_token"),
        Path("/root/.secrets/slack_bot_token"),
    ]
    for candidate in candidates:
        if candidate and candidate.exists():
            return candidate.read_text().strip()
    raise RuntimeError("Slack bot token not found; set SLACK_BOT_TOKEN or provide --token-file")


def slack_api(method: str, token: str, params: dict[str, str]) -> dict[str, Any]:
    query = urllib.parse.urlencode(params)
    url = f"https://slack.com/api/{method}?{query}"
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    with urllib.request.urlopen(req, timeout=60) as response:
        data = json.load(response)
    if not data.get("ok"):
        raise RuntimeError(f"Slack API {method} failed: {data}")
    return data


def download_slack_file(url: str, token: str, dest: Path) -> None:
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    with urllib.request.urlopen(req, timeout=300) as response:
        dest.write_bytes(response.read())


def ffprobe_json(ffprobe_path: str, media_path: Path) -> dict[str, Any]:
    cmd = [
        ffprobe_path,
        "-v",
        "quiet",
        "-print_format",
        "json",
        "-show_streams",
        "-show_format",
        str(media_path),
    ]
    result = subprocess.run(cmd, check=True, capture_output=True, text=True)
    return json.loads(result.stdout)


def extract_audio(ffmpeg_path: str, media_path: Path, out_path: Path) -> bool:
    cmd = [
        ffmpeg_path,
        "-y",
        "-i",
        str(media_path),
        "-vn",
        "-ac",
        "1",
        "-ar",
        "16000",
        str(out_path),
    ]
    result = subprocess.run(cmd, capture_output=True, text=True)
    return result.returncode == 0 and out_path.exists()


def extract_frames(
    ffmpeg_path: str,
    media_path: Path,
    frames_dir: Path,
    interval_secs: float,
    max_frames: int,
) -> list[str]:
    frames_dir.mkdir(parents=True, exist_ok=True)
    fps_value = f"1/{interval_secs:.6f}"
    pattern = str(frames_dir / "frame-%04d.jpg")
    cmd = [
        ffmpeg_path,
        "-y",
        "-i",
        str(media_path),
        "-vf",
        f"fps={fps_value}",
        "-frames:v",
        str(max_frames),
        pattern,
    ]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        return []
    return [p.name for p in sorted(frames_dir.glob("*.jpg"))]


def deepgram_key() -> str:
    if os.environ.get("DEEPGRAM_API_KEY"):
        return os.environ["DEEPGRAM_API_KEY"].strip()
    for candidate in [
        Path("/root/.secrets/master.env"),
        Path("/root/.openclaw/secrets/master.env"),
    ]:
        if not candidate.exists():
            continue
        for line in candidate.read_text(errors="ignore").splitlines():
            if line.startswith("DEEPGRAM_API_KEY="):
                return line.split("=", 1)[1].strip()
    return ""


def transcribe_with_deepgram(media_path: Path, mime: str) -> str:
    key = deepgram_key()
    if not key:
        return ""
    url = (
        "https://api.deepgram.com/v1/listen?"
        "model=nova-2&smart_format=true&punctuate=true&detect_language=true"
    )
    data = media_path.read_bytes()
    req = urllib.request.Request(
        url,
        data=data,
        headers={
            "Authorization": f"Token {key}",
            "Content-Type": mime,
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=300) as response:
            payload = json.load(response)
    except Exception:
        return ""
    try:
        return payload["results"]["channels"][0]["alternatives"][0]["transcript"].strip()
    except Exception:
        return ""


def file_kind(mime: str) -> str:
    if mime.startswith("video/"):
        return "video"
    if mime.startswith("audio/"):
        return "audio"
    if mime.startswith("image/"):
        return "image"
    return "other"


def write_json(path: Path, payload: Any) -> None:
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")


def build_local_messages(title: str) -> list[dict[str, Any]]:
    text = title or "Local media smoke test"
    return [{"text": text, "user": "local", "ts": "local", "thread_ts": "local"}]


def summarize_manifest(manifest: dict[str, Any]) -> str:
    lines = [
        f"Source mode: {manifest['source_mode']}",
        f"Messages captured: {len(manifest['messages'])}",
        f"Media files: {len(manifest['files'])}",
    ]
    for item in manifest["files"]:
        lines.append(
            f"- {item['name']} [{item['kind']}] size={item['size_bytes']} transcript={'yes' if item['has_transcript'] else 'no'} frames={item.get('frame_count', 0)}"
        )
    lines.append("")
    lines.append("Thread context:")
    for msg in manifest["messages"][:12]:
        user = msg.get("user") or msg.get("bot_id") or "unknown"
        text = (msg.get("text") or "").strip().replace("\n", " ")
        if text:
            lines.append(f"- {user}: {text[:240]}")
    return "\n".join(lines).strip() + "\n"


def build_packet_for_file(
    ffmpeg_path: str,
    ffprobe_path: str,
    src_path: Path,
    base_dir: Path,
    item_id: str,
    item_name: str,
    mime: str,
    frame_interval_secs: float,
    max_frames: int,
) -> dict[str, Any]:
    item_dir = base_dir / safe_slug(f"{item_id}-{item_name}")
    item_dir.mkdir(parents=True, exist_ok=True)
    dest_path = item_dir / item_name
    if src_path != dest_path:
        shutil.copy2(src_path, dest_path)

    metadata = {
        "id": item_id,
        "name": item_name,
        "mime": mime,
        "kind": file_kind(mime),
        "size_bytes": dest_path.stat().st_size,
    }
    write_json(item_dir / "metadata.json", metadata)

    transcript = ""
    frame_files: list[str] = []

    if metadata["kind"] in {"video", "audio"}:
        probe = ffprobe_json(ffprobe_path, dest_path)
        write_json(item_dir / "ffprobe.json", probe)
        audio_path = item_dir / "audio.wav"
        audio_ready = extract_audio(ffmpeg_path, dest_path, audio_path)
        if audio_ready:
            transcript = transcribe_with_deepgram(audio_path, "audio/wav")
            if transcript:
                (item_dir / "transcript.txt").write_text(transcript + "\n")
        elif metadata["kind"] == "audio":
            transcript = transcribe_with_deepgram(dest_path, mime)
            if transcript:
                (item_dir / "transcript.txt").write_text(transcript + "\n")

    if metadata["kind"] == "video":
        frame_files = extract_frames(
            ffmpeg_path,
            dest_path,
            item_dir / "frames",
            frame_interval_secs,
            max_frames,
        )

    return {
        "id": item_id,
        "name": item_name,
        "path": str(dest_path),
        "kind": metadata["kind"],
        "mime": mime,
        "size_bytes": metadata["size_bytes"],
        "has_transcript": bool(transcript),
        "frame_count": len(frame_files),
    }


def slack_thread_messages(token: str, channel: str, thread_ts: str) -> list[dict[str, Any]]:
    data = slack_api(
        "conversations.replies",
        token,
        {"channel": channel, "ts": thread_ts, "limit": "200"},
    )
    return data.get("messages", [])


def main() -> int:
    args = parse_args()
    ffmpeg_path = ensure_cmd("ffmpeg")
    ffprobe_path = ensure_cmd("ffprobe")

    out_dir = Path(args.out).expanduser().resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    files_dir = out_dir / "files"
    files_dir.mkdir(parents=True, exist_ok=True)

    messages: list[dict[str, Any]]
    manifest_files: list[dict[str, Any]] = []

    if args.file:
        src_path = Path(args.file).expanduser().resolve()
        if not src_path.exists():
            raise FileNotFoundError(src_path)
        mime = mimetypes.guess_type(src_path.name)[0] or "application/octet-stream"
        messages = build_local_messages(args.title)
        manifest_files.append(
            build_packet_for_file(
                ffmpeg_path,
                ffprobe_path,
                src_path,
                files_dir,
                "local",
                src_path.name,
                mime,
                args.frame_interval_secs,
                args.max_frames,
            )
        )
        source_mode = "local-file"
    else:
        token = read_slack_token(args.token_file)
        messages = slack_thread_messages(token, args.channel, args.thread_ts)
        for message in messages:
            for file_obj in message.get("files", []) or []:
                url = file_obj.get("url_private_download") or file_obj.get("url_private")
                if not url:
                    continue
                file_id = file_obj.get("id") or "slack-file"
                name = file_obj.get("name") or f"{file_id}.bin"
                mime = file_obj.get("mimetype") or "application/octet-stream"
                tmp_path = out_dir / f"download-{safe_slug(file_id)}-{safe_slug(name)}"
                download_slack_file(url, token, tmp_path)
                manifest_files.append(
                    build_packet_for_file(
                        ffmpeg_path,
                        ffprobe_path,
                        tmp_path,
                        files_dir,
                        file_id,
                        name,
                        mime,
                        args.frame_interval_secs,
                        args.max_frames,
                    )
                )
        source_mode = "slack-thread"

    filtered_messages = [
        {k: msg.get(k) for k in ["ts", "thread_ts", "user", "bot_id", "text", "subtype"]}
        for msg in messages
    ]
    write_json(out_dir / "messages.json", filtered_messages)

    manifest = {
        "source_mode": source_mode,
        "channel": args.channel or "",
        "thread_ts": args.thread_ts or "",
        "messages": filtered_messages,
        "files": manifest_files,
    }
    write_json(out_dir / "manifest.json", manifest)
    (out_dir / "summary.txt").write_text(summarize_manifest(manifest))

    print(json.dumps({"ok": True, "out": str(out_dir), "files": manifest_files}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
