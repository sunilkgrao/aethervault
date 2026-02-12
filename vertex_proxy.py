#!/usr/bin/env python3
"""
Native Anthropic proxy for Vertex AI Claude with proper token tracking.

This proxy ensures SSE events from Vertex AI are in the exact format
that aethervault's pi-ai library expects for token tracking.

Key features:
1. Parses SSE events from Vertex
2. Ensures input_tokens and output_tokens are present and non-zero
3. Logs token information for debugging
4. Re-emits events in standard Anthropic SSE format
"""
import json
import os
import subprocess
import time
import urllib.error
import urllib.request
import sys
import re
from http.server import HTTPServer, BaseHTTPRequestHandler

PROJECT_ID = os.environ.get("GCP_PROJECT_ID", "your-gcp-project")
REGION = os.environ.get("GCP_REGION", "us-east5")
VERTEX_PORT = int(os.environ.get("VERTEX_PROXY_PORT", "11436"))
VERTEX_BIND = os.environ.get("VERTEX_BIND_ADDR", "127.0.0.1")
VERTEX_DEFAULT_MODEL = os.environ.get("VERTEX_DEFAULT_MODEL", "claude-opus-4-5")
VERTEX_MAX_TOKENS = int(os.environ.get("VERTEX_MAX_TOKENS", "4096"))
AETHERVAULT_HOME = os.environ.get("AETHERVAULT_HOME", os.path.expanduser("~/.aethervault"))
VERTEX_CREDENTIALS = os.environ.get("VERTEX_CREDENTIALS", os.path.join(AETHERVAULT_HOME, "vertex-credentials.json"))
_token = {"t": None, "exp": 0}


def log(msg):
    ts = time.strftime("%H:%M:%S")
    print(f"{ts} {msg}", flush=True)


def get_token():
    if _token["t"] and time.time() < _token["exp"] - 300:
        return _token["t"]
    r = subprocess.run(
        ["gcloud", "auth", "print-access-token"],
        capture_output=True,
        text=True,
        timeout=30,
    )
    _token["t"], _token["exp"] = r.stdout.strip(), time.time() + 3600
    return _token["t"]


def estimate_tokens(text):
    """Rough token estimate: ~4 chars per token for English."""
    if not text:
        return 0
    return max(1, len(text) // 4)


def parse_sse_line(line_bytes):
    """Parse an SSE line and return (event_type, data_str) or (None, None)."""
    line = line_bytes.decode("utf-8", errors="replace").rstrip()
    if line.startswith("event:"):
        return ("event_type", line[6:].strip())
    elif line.startswith("data:"):
        return ("data", line[5:].strip())
    elif line == "":
        return ("blank", "")
    return (None, line)


class Proxy(BaseHTTPRequestHandler):
    def do_POST(self):
        if "/v1/messages" not in self.path:
            self.send_error(404)
            return

        raw_body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        try:
            body = json.loads(raw_body.decode())
        except (json.JSONDecodeError, UnicodeDecodeError) as e:
            log(f"Bad request body: {e}")
            self.send_response(400)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps({"type": "error", "error": {"type": "invalid_request_error", "message": str(e)}}).encode())
            return
        model_id = body.get("model", VERTEX_DEFAULT_MODEL)
        # Model map: configurable via VERTEX_MODEL_MAP env var (JSON), with sensible defaults
        default_map = {
            "claude-opus-4-5": "claude-opus-4-5@20251101",
            "claude-sonnet-4-5": "claude-sonnet-4-5@20250929",
        }
        model_map_env = os.environ.get("VERTEX_MODEL_MAP")
        model_map = json.loads(model_map_env) if model_map_env else default_map
        model = model_map.get(model_id, model_id)
        stream = body.get("stream", False)

        # Estimate input tokens from request
        estimated_input = estimate_tokens(raw_body.decode())
        log(f"POST /v1/messages model={model_id} stream={stream} est_input={estimated_input}")

        payload = {
            "anthropic_version": "vertex-2023-10-16",
            "messages": body.get("messages", []),
            "max_tokens": body.get("max_tokens", VERTEX_MAX_TOKENS),
        }
        if "system" in body:
            payload["system"] = body["system"]
        if "tools" in body:
            payload["tools"] = body["tools"]
        if "tool_choice" in body:
            payload["tool_choice"] = body["tool_choice"]

        if stream:
            payload["stream"] = True
            url = f"https://{REGION}-aiplatform.googleapis.com/v1/projects/{PROJECT_ID}/locations/{REGION}/publishers/anthropic/models/{model}:streamRawPredict"
        else:
            url = f"https://{REGION}-aiplatform.googleapis.com/v1/projects/{PROJECT_ID}/locations/{REGION}/publishers/anthropic/models/{model}:rawPredict"

        req = urllib.request.Request(
            url,
            json.dumps(payload).encode(),
            {"Authorization": f"Bearer {get_token()}", "Content-Type": "application/json"},
        )

        try:
            if stream:
                self._handle_streaming(req, estimated_input)
            else:
                self._handle_non_streaming(req, estimated_input)
        except urllib.error.HTTPError as e:
            err_body = e.read().decode()
            log(f"HTTP Error {e.code}: {err_body[:200]}")

            if stream:
                # For streaming requests, return SSE-formatted error so SDK can parse it
                self._send_sse_error(e.code, err_body, estimated_input)
            else:
                self.send_response(e.code)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(err_body.encode())
        except Exception as e:
            log(f"Error: {e}")
            if stream:
                self._send_sse_error(500, str(e), estimated_input)
            else:
                self.send_response(500)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(
                    json.dumps({"type": "error", "error": {"type": "api_error", "message": str(e)}}).encode()
                )

    def _handle_streaming(self, req, estimated_input):
        """Handle streaming response with proper token tracking."""
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()

        current_event_type = None
        input_tokens_seen = 0
        output_tokens_seen = 0
        message_start_processed = False

        try:
            with urllib.request.urlopen(req, timeout=300) as resp:
                for line_bytes in resp:
                    line_type, content = parse_sse_line(line_bytes)

                    if line_type == "event_type":
                        current_event_type = content
                        # Pass through event type line
                        self.wfile.write(line_bytes)
                        self.wfile.flush()

                    elif line_type == "data":
                        # Parse JSON and potentially fix token tracking
                        try:
                            data = json.loads(content) if content else None
                        except json.JSONDecodeError:
                            data = None

                        if data and data.get("type") == "message_start" and not message_start_processed:
                            # Ensure input_tokens is present and reasonable
                            message = data.get("message", {})
                            usage = message.get("usage", {})

                            actual_input = usage.get("input_tokens", 0)
                            # Vertex often returns 0 or very low token counts
                            # Use estimate if actual is suspiciously low (< 10% of estimate or < 10)
                            use_estimate = (
                                actual_input == 0
                                or (estimated_input > 100 and actual_input < estimated_input * 0.1)
                                or (estimated_input > 1000 and actual_input < 10)
                            )

                            if use_estimate:
                                # Inject estimated tokens
                                if "usage" not in message:
                                    message["usage"] = {}
                                message["usage"]["input_tokens"] = estimated_input
                                data["message"] = message
                                input_tokens_seen = estimated_input
                                log(f"  message_start: actual={actual_input} -> injected={estimated_input}")
                            else:
                                input_tokens_seen = actual_input
                                log(f"  message_start: actual input_tokens={actual_input}")

                            message_start_processed = True
                            # Re-serialize with proper tokens
                            fixed_line = f"data: {json.dumps(data)}\n"
                            self.wfile.write(fixed_line.encode())
                            self.wfile.flush()

                        elif data and data.get("type") == "message_delta":
                            # Capture output tokens from message_delta
                            usage = data.get("usage", {})
                            output_tokens = usage.get("output_tokens", 0)
                            if output_tokens > 0:
                                output_tokens_seen = output_tokens
                            # Pass through as-is
                            self.wfile.write(line_bytes)
                            self.wfile.flush()

                        else:
                            # Pass through other data lines as-is
                            self.wfile.write(line_bytes)
                            self.wfile.flush()

                    elif line_type == "blank":
                        # Blank line signals end of event
                        self.wfile.write(b"\n")
                        self.wfile.flush()

                    else:
                        # Unknown line type, pass through
                        self.wfile.write(line_bytes)
                        self.wfile.flush()

        except urllib.error.HTTPError as e:
            # Headers already sent — emit error as SSE event, don't re-send headers
            err_body = e.read().decode("utf-8", errors="replace")
            log(f"Streaming HTTP Error {e.code}: {err_body[:200]}")
            self._emit_sse_error_events(e.code, err_body, estimated_input)
            return
        except Exception as e:
            # Headers already sent — emit error as SSE event
            log(f"Streaming error: {e}")
            self._emit_sse_error_events(500, str(e), estimated_input)
            return

        log(f"Stream complete input={input_tokens_seen} output={output_tokens_seen}")

    def _handle_non_streaming(self, req, estimated_input):
        """Handle non-streaming response with token tracking."""
        with urllib.request.urlopen(req, timeout=300) as resp:
            data = json.loads(resp.read())

        # Ensure input_tokens is present
        usage = data.get("usage", {})
        if usage.get("input_tokens", 0) == 0:
            if "usage" not in data:
                data["usage"] = {}
            data["usage"]["input_tokens"] = estimated_input
            log(f"Non-streaming: injected input_tokens={estimated_input}")
        else:
            log(f"Non-streaming: actual input_tokens={usage.get('input_tokens')}")

        response_bytes = json.dumps(data).encode()
        log(f"Response OK: {len(response_bytes)} bytes")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(response_bytes)

    def _emit_sse_error_events(self, status_code, error_body, estimated_input):
        """Emit SSE error events on an already-open stream (headers already sent)."""
        try:
            err_data = json.loads(error_body)
            err_message = err_data.get("error", {}).get("message", error_body)
            err_type = err_data.get("error", {}).get("type", "api_error")
        except (json.JSONDecodeError, KeyError):
            err_message = error_body
            err_type = "api_error"

        message_start = {
            "type": "message_start",
            "message": {
                "id": f"msg_error_{int(time.time())}",
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": VERTEX_DEFAULT_MODEL,
                "stop_reason": "error",
                "stop_sequence": None,
                "usage": {"input_tokens": estimated_input, "output_tokens": 0}
            }
        }
        self.wfile.write(f"event: message_start\ndata: {json.dumps(message_start)}\n\n".encode())
        self.wfile.flush()

        error_event = {
            "type": "error",
            "error": {
                "type": err_type,
                "message": err_message
            }
        }
        self.wfile.write(f"event: error\ndata: {json.dumps(error_event)}\n\n".encode())
        self.wfile.flush()

        log(f"  SSE error sent: {err_message[:100]}")

    def _send_sse_error(self, status_code, error_body, estimated_input):
        """Send error in SSE format for streaming requests (headers NOT yet sent).

        This is critical for context overflow errors - the Anthropic SDK expects
        SSE format for streaming requests. Without this, the SDK throws a generic
        'request ended without sending any chunks' error instead of propagating
        the actual error message that AetherVault's isContextOverflowError can detect.
        """
        self.send_response(200)  # SSE requires 200 to start streaming
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()

        self._emit_sse_error_events(status_code, error_body, estimated_input)

    def log_message(self, fmt, *args):
        pass


if __name__ == "__main__":
    subprocess.run(
        ["gcloud", "auth", "activate-service-account", f"--key-file={VERTEX_CREDENTIALS}"],
        capture_output=True,
    )
    get_token()
    log(f"Vertex proxy ready on {VERTEX_BIND}:{VERTEX_PORT} (with token tracking)")
    HTTPServer((VERTEX_BIND, VERTEX_PORT), Proxy).serve_forever()
