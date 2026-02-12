#!/usr/bin/env python3
"""Legacy Claude hook. Prefer `--model-hook builtin:claude` for production."""
import json
import os
import sys
import urllib.request
import urllib.error
import time

def _env(name, default=None):
    value = os.environ.get(name)
    if value is None or value == "":
        return default
    return value

def _read_input():
    raw = sys.stdin.read()
    if not raw.strip():
        return {}
    return json.loads(raw)

def _merge_system(messages):
    parts = []
    for msg in messages:
        if msg.get("role") == "system":
            content = msg.get("content")
            if content:
                parts.append(content)
    return "\n\n".join(parts).strip()

def _to_anthropic_messages(messages):
    out = []
    for msg in messages:
        role = msg.get("role")
        if role == "system":
            continue
        if role == "user":
            content = msg.get("content") or ""
            out.append({
                "role": "user",
                "content": [{"type": "text", "text": content}],
            })
            continue
        if role == "assistant":
            blocks = []
            content = msg.get("content")
            if content:
                blocks.append({"type": "text", "text": content})
            for call in msg.get("tool_calls") or []:
                blocks.append({
                    "type": "tool_use",
                    "id": call.get("id") or "",
                    "name": call.get("name") or "",
                    "input": call.get("args") or {},
                })
            if not blocks:
                blocks.append({"type": "text", "text": ""})
            out.append({"role": "assistant", "content": blocks})
            continue
        if role == "tool":
            tool_id = msg.get("tool_call_id") or msg.get("id")
            if not tool_id:
                continue
            content = msg.get("content") or ""
            block = {
                "type": "tool_result",
                "tool_use_id": tool_id,
                "content": content,
            }
            if msg.get("is_error") is True:
                block["is_error"] = True
            out.append({
                "role": "user",
                "content": [block],
            })
            continue
    return out

def _to_anthropic_tools(tools):
    out = []
    for tool in tools or []:
        if not isinstance(tool, dict):
            continue
        entry = {
            "name": tool.get("name"),
        }
        if tool.get("description"):
            entry["description"] = tool.get("description")
        schema = tool.get("inputSchema") or tool.get("input_schema")
        if schema:
            entry["input_schema"] = schema
        out.append(entry)
    return out

def _parse_response(payload):
    content_blocks = payload.get("content") or []
    text_parts = []
    tool_calls = []
    for block in content_blocks:
        btype = block.get("type")
        if btype == "text":
            text_parts.append(block.get("text") or "")
        elif btype == "tool_use":
            tool_calls.append({
                "id": block.get("id") or "",
                "name": block.get("name") or "",
                "args": block.get("input") or {},
            })
    text = "\n".join([t for t in text_parts if t is not None]).strip()
    message = {
        "role": "assistant",
        "content": text if text else None,
        "tool_calls": tool_calls,
    }
    return {"message": message}

def _call_api(url, data, headers, timeout, max_retries, retry_base, retry_max):
    """Make an API call with retries. Returns (body_str, None) on success or (None, error_str) on failure."""
    request = urllib.request.Request(url, data=data, headers=headers, method="POST")
    for attempt in range(max_retries + 1):
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return response.read().decode("utf-8"), None
        except urllib.error.HTTPError as err:
            status = err.code
            retryable = status in (429, 500, 502, 503, 504, 529)
            err_body = err.read().decode("utf-8") if err.fp else str(err)
            if attempt < max_retries and retryable:
                wait = min(retry_max, retry_base * (2 ** attempt))
                retry_after = err.headers.get("retry-after") if err.headers else None
                if retry_after:
                    try:
                        wait = max(wait, float(retry_after))
                    except ValueError:
                        pass
                time.sleep(wait)
                continue
            return None, f"API error: {status} {err_body}"
        except (urllib.error.URLError, OSError) as err:
            if attempt < max_retries:
                wait = min(retry_max, retry_base * (2 ** attempt))
                time.sleep(wait)
                continue
            return None, f"API request failed: {err}"
    return None, "API request failed without a response"


def main():
    req = _read_input()
    messages = req.get("messages") or []
    tools = req.get("tools") or []

    api_key = _env("ANTHROPIC_API_KEY")
    if not api_key:
        sys.stderr.write("Missing ANTHROPIC_API_KEY\n")
        return 1

    base_url = _env("ANTHROPIC_BASE_URL", "https://api.anthropic.com/v1/messages")
    model = _env("ANTHROPIC_MODEL")
    if not model:
        sys.stderr.write("Missing ANTHROPIC_MODEL\n")
        return 1
    max_tokens = int(_env("ANTHROPIC_MAX_TOKENS", "1024"))
    temperature = _env("ANTHROPIC_TEMPERATURE")
    top_p = _env("ANTHROPIC_TOP_P")
    timeout = float(_env("ANTHROPIC_TIMEOUT", "60"))
    max_retries = int(_env("ANTHROPIC_MAX_RETRIES", "2"))
    retry_base = float(_env("ANTHROPIC_RETRY_BASE", "0.5"))
    retry_max = float(_env("ANTHROPIC_RETRY_MAX", "4"))

    # Vertex proxy as automatic fallback when Anthropic direct fails.
    vertex_fallback_url = _env("VERTEX_FALLBACK_URL", "http://localhost:11436/v1/messages")
    vertex_fallback_enabled = _env("VERTEX_FALLBACK", "1") == "1"

    system = _merge_system(messages)
    payload = {
        "model": model,
        "max_tokens": max_tokens,
        "messages": _to_anthropic_messages(messages),
    }
    if system:
        payload["system"] = system

    anth_tools = _to_anthropic_tools(tools)
    if anth_tools:
        payload["tools"] = anth_tools

    if temperature is not None:
        payload["temperature"] = float(temperature)
    if top_p is not None:
        payload["top_p"] = float(top_p)

    headers = {
        "content-type": "application/json",
        "x-api-key": api_key,
        "anthropic-version": _env("ANTHROPIC_VERSION", "2023-06-01"),
    }
    beta = _env("ANTHROPIC_BETA")
    if beta:
        headers["anthropic-beta"] = beta

    data = json.dumps(payload).encode("utf-8")

    # Primary: Anthropic direct API.
    body, err = _call_api(base_url, data, headers, timeout, max_retries, retry_base, retry_max)

    # Fallback: Vertex proxy if primary failed.
    if body is None and vertex_fallback_enabled:
        sys.stderr.write(f"Anthropic primary failed ({err}), falling back to Vertex proxy\n")
        vertex_headers = dict(headers)
        vertex_headers["x-api-key"] = _env("VERTEX_API_KEY", api_key)
        body, vertex_err = _call_api(
            vertex_fallback_url, data, vertex_headers, timeout, max_retries, retry_base, retry_max
        )
        if body is None:
            sys.stderr.write(f"Vertex fallback also failed: {vertex_err}\n")
            sys.stderr.write(f"Primary error was: {err}\n")
            return 1

    if body is None:
        sys.stderr.write(f"Anthropic API failed: {err}\n")
        return 1

    try:
        payload = json.loads(body)
    except json.JSONDecodeError:
        sys.stderr.write("Invalid JSON from API\n")
        return 1

    out = _parse_response(payload)
    sys.stdout.write(json.dumps(out))
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
