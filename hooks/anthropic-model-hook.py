#!/usr/bin/env python3
"""
Anthropic Claude model hook for AetherVault agent subcommand.
Returns: {"message": {"role": "assistant", "content": "...", "tool_calls": [...]}}

Process group isolation, stdin timeout, and output capping are handled by the Rust binary.
"""
import json
import os
import sys
import httpx

ANTHROPIC_API_KEY = os.environ.get("ANTHROPIC_API_KEY", "")
ANTHROPIC_MODEL = os.environ.get("ANTHROPIC_MODEL", "claude-sonnet-4-5-20250929")
ANTHROPIC_MAX_TOKENS = int(os.environ.get("ANTHROPIC_MAX_TOKENS", "16384"))


def main():
    try:
        request = json.loads(sys.stdin.read())
    except (json.JSONDecodeError, TypeError) as e:
        print(json.dumps({"message": {"role": "assistant", "content": f"(Invalid JSON input: {e})", "tool_calls": []}}))
        return

    messages = request.get("messages", [])
    system_prompt = request.get("system", "")
    tools = request.get("tools", [])

    api_messages = []
    for msg in messages:
        role = msg.get("role", "user")
        content = msg.get("content", "")
        if role == "system":
            if not system_prompt:
                system_prompt = content
            continue
        if not content:
            content = " "
        api_messages.append({"role": role, "content": content})

    if not api_messages:
        api_messages = [{"role": "user", "content": "Hello"}]

    body = {
        "model": ANTHROPIC_MODEL,
        "max_tokens": ANTHROPIC_MAX_TOKENS,
        "messages": api_messages,
    }
    if system_prompt:
        body["system"] = system_prompt
    if tools:
        anthropic_tools = []
        for tool in tools:
            t = {
                "name": tool.get("name", ""),
                "description": tool.get("description", ""),
                "input_schema": tool.get("input_schema") or tool.get("parameters", {"type": "object", "properties": {}}),
            }
            anthropic_tools.append(t)
        body["tools"] = anthropic_tools

    headers = {
        "x-api-key": ANTHROPIC_API_KEY,
        "anthropic-version": "2023-06-01",
        "content-type": "application/json",
    }

    try:
        resp = httpx.post(
            "https://api.anthropic.com/v1/messages",
            json=body,
            headers=headers,
            timeout=120.0,
        )
    except Exception as e:
        print(json.dumps({"message": {"role": "assistant", "content": f"(HTTP error: {e})", "tool_calls": []}}))
        return

    if resp.status_code != 200:
        print(json.dumps({"message": {"role": "assistant", "content": f"(API Error {resp.status_code}: {resp.text[:500]})", "tool_calls": []}}))
        return

    try:
        data = resp.json()
    except Exception as e:
        print(json.dumps({"message": {"role": "assistant", "content": f"(Invalid API response: {e})", "tool_calls": []}}))
        return

    text_parts = []
    tool_calls = []

    for block in data.get("content", []):
        if block["type"] == "text":
            text_parts.append(block["text"])
        elif block["type"] == "tool_use":
            tool_calls.append({
                "id": block["id"],
                "name": block["name"],
                "arguments": json.dumps(block["input"]),
            })

    response = {
        "message": {
            "role": "assistant",
            "content": "\n".join(text_parts),
            "tool_calls": tool_calls,
        }
    }
    print(json.dumps(response))

if __name__ == "__main__":
    main()
