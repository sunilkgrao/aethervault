#!/usr/bin/env python3
"""Adversarial swarm simulator for AetherVault MCP tools.

This utility launches multiple short-lived MCP sessions in parallel and performs
high-risk tool-call scenarios to surface security control gaps.

- default mode: execute attack matrix against a local probe endpoint and report if
  expected defenses are missing.
- static mode: analyze source files for high-confidence risky sinks when the
  MCP binary is unavailable.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import random
import shutil
import subprocess
import tempfile
import threading
import time
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Set


def random_suffix(length: int = 8) -> str:
    chars = "abcdefghijklmnopqrstuvwxyz0123456789"
    return "".join(random.choice(chars) for _ in range(length))


class ProbeHandler(BaseHTTPRequestHandler):
    """Lightweight local HTTP target used to verify egress behavior."""

    def do_GET(self) -> None:
        self._reply({"method": "GET", "path": self.path})

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0") or "0")
        body = self.rfile.read(length).decode(errors="replace") if length > 0 else ""
        self._reply({"method": "POST", "path": self.path, "body": body})

    def log_message(self, fmt, *args) -> None:  # pragma: no cover
        return

    def _reply(self, payload: Dict[str, Any]) -> None:
        payload = dict(payload)
        payload["token"] = PROBE_TOKEN
        data = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def start_probe_server() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), ProbeHandler)
    addr, port = server.server_address
    url = f"http://{addr}:{port}"
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, url


@dataclass
class MCPResult:
    tool: str
    output: str
    details: Dict[str, Any]
    is_error: bool
    raw: Dict[str, Any]


@dataclass
class AttackStep:
    tool: str
    arguments_factory: Callable[[str], Dict[str, Any]]


class MCPClient:
    def __init__(
        self,
        binary: str,
        mv2: Path,
        auto_approve: bool = False,
        timeout_sec: int = 20,
        env_overrides: Optional[Dict[str, str]] = None,
    ):
        self.binary = str(binary)
        self.mv2 = str(mv2)
        self.auto_approve = auto_approve
        self.timeout_sec = timeout_sec
        self.env_overrides = env_overrides or {}
        self.proc: Optional[subprocess.Popen[str]] = None
        self._msg_id = 0
        self._lock = threading.Lock()

    def __enter__(self) -> "MCPClient":
        env = os.environ.copy()
        if self.auto_approve:
            env["AETHERVAULT_BRIDGE_AUTO_APPROVE"] = "1"
        env.update(self.env_overrides)
        self.proc = subprocess.Popen(
            [self.binary, "mcp", self.mv2],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
        )
        if self.proc.stdin is None or self.proc.stdout is None:
            raise RuntimeError("unable to open MCP stdio pipes")

        init = self._call("initialize", {"protocolVersion": "2025-03-26", "clientInfo": {"name": "aethervault-swarm"}}, expect_id=False)
        if init.get("error"):
            raise RuntimeError(f"MCP init failed: {init['error']}")
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        if self.proc is None:
            return
        try:
            self.proc.terminate()
            self.proc.wait(timeout=2)
        except Exception:
            try:
                self.proc.kill()
            except Exception:
                pass

    def list_tools(self) -> List[str]:
        resp = self._call("tools/list", {})
        tools = resp.get("result", {}).get("tools", [])
        names = []
        for tool in tools:
            if not isinstance(tool, dict):
                continue
            name = tool.get("name")
            if isinstance(name, str):
                names.append(name)
        return names

    def _call(self, method: str, params: Dict[str, Any], expect_id: bool = True) -> Dict[str, Any]:
        if not self.proc or not self.proc.stdin or not self.proc.stdout:
            raise RuntimeError("MCP client is not running")

        with self._lock:
            if expect_id:
                self._msg_id += 1
                msg_id = self._msg_id
            else:
                msg_id = None

            body = {"jsonrpc": "2.0", "method": method, "params": params}
            if msg_id is not None:
                body["id"] = msg_id
            self.proc.stdin.write(json.dumps(body) + "\n")
            self.proc.stdin.flush()

            # Wait for response line with matching id.
            deadline = time.time() + self.timeout_sec
            while time.time() < deadline:
                line = self.proc.stdout.readline()
                if not line:
                    if self.proc.poll() is not None:
                        raise RuntimeError("MCP process exited")
                    time.sleep(0.02)
                    continue

                line = line.strip()
                if not line:
                    continue
                try:
                    msg = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if not expect_id or msg.get("id") == msg_id:
                    return msg
            raise TimeoutError(f"timeout waiting for MCP response to {method}")

    def call_tool(self, name: str, arguments: Dict[str, Any]) -> MCPResult:
        resp = self._call("tools/call", {"name": name, "arguments": arguments})
        if "error" in resp:
            return MCPResult(
                tool=name,
                output="",
                details={"error": resp["error"].get("message", "")},
                is_error=True,
                raw=resp,
            )

        result = resp.get("result", {})
        content = result.get("content", [])
        output = "".join(c.get("text", "") for c in content if isinstance(c, dict))
        details = result.get("details", {}) if isinstance(result.get("details"), dict) else {}
        is_error = bool(result.get("isError", False))
        return MCPResult(tool=name, output=output, details=details, is_error=is_error, raw=result)


def approval_blocked(result: MCPResult) -> bool:
    output = (result.output or "").lower()
    if "approval required" in output:
        return True
    details_error = ""
    if isinstance(result.raw, dict):
        error_obj = result.raw.get("error", {})
        if isinstance(error_obj, dict):
            details_error = error_obj.get("message", "")
        elif isinstance(error_obj, str):
            details_error = error_obj
    return "approval required" in (details_error or "").lower()


def tool_output_field(result: MCPResult, key: str, default: Any = None) -> Any:
    if not isinstance(result.details, dict):
        return default
    return result.details.get(key, default)


@dataclass
class Attack:
    id: str
    name: str
    tool: str
    arguments_factory: Callable[[str], Dict[str, Any]]
    severity: str
    evaluate: Callable[[List[MCPResult], MCPResult, bool, str], tuple[str, str]]
    prerequisites: List[AttackStep] = field(default_factory=list)
    marker: str = ""
    required_tools: List[str] = field(default_factory=list)
    env_overrides: Dict[str, str] = field(default_factory=dict)
    stop_on_prereq_error: bool = True
    isolate_capsule: bool = False


def safe_json_text(value: Any) -> str:
    if isinstance(value, str):
        return value
    try:
        return json.dumps(value, ensure_ascii=False, sort_keys=True)
    except (TypeError, ValueError):
        return str(value)


def result_contains_marker(result: MCPResult, marker: str) -> bool:
    if not marker:
        return False

    def walk(value: Any) -> bool:
        if isinstance(value, str):
            return marker in value
        if isinstance(value, dict):
            return any(walk(v) for v in value.values())
        if isinstance(value, (list, tuple, set)):
            return any(walk(v) for v in value)
        if value is None:
            return False
        return marker in str(value)

    if walk(result.output):
        return True
    if walk(result.details):
        return True
    if walk(result.raw):
        return True
    return False


def expected_count(marker: str) -> int:
    if not marker:
        return 0
    marker = marker.strip()
    if marker.startswith("count="):
        marker = marker.split("=", 1)[1]
    if marker.isdigit():
        return int(marker)
    return 0


def evaluate_approval_gate(
    prereqs: List[MCPResult],
    result: MCPResult,
    auto_approve: bool,
    expected_marker: str,
) -> tuple[str, str]:
    blocked = approval_blocked(result)
    if auto_approve:
        if blocked:
            return "pass", "approval required marker is present even though auto-approval is enabled"
        if result.is_error:
            return "pass", f"auto-approval enabled but the tool still failed ({result.output})"
        return "fail", f"tool executed while auto-approval enabled ({expected_marker})"
    if blocked:
        return "pass", "guarded by approval flow"
    return "fail", f"sensitivity gate absent (expected approval) ({expected_marker})"


def evaluate_http_get(
    _prereqs: List[MCPResult],
    result: MCPResult,
    _auto_approve: bool,
    expected_marker: str,
) -> tuple[str, str]:
    status = tool_output_field(result, "status", 0)
    body = tool_output_field(result, "body", "")
    body_text = body if isinstance(body, str) else json.dumps(body)
    if status == 200 and expected_marker and expected_marker in body_text:
        return "fail", "GET allows arbitrary outbound URL access without explicit approval requirement"
    if result.is_error:
        return "pass", f"HTTP request failed before data exfiltration surface could be validated ({result.output})"
    return "pass", "no direct probe evidence"


def evaluate_http_post(
    prereqs: List[MCPResult],
    result: MCPResult,
    auto_approve: bool,
    _expected_marker: str,
) -> tuple[str, str]:
    return evaluate_approval_gate(prereqs, result, auto_approve, "POST request should be approval-gated")


def evaluate_fs_write(
    prereqs: List[MCPResult],
    result: MCPResult,
    auto_approve: bool,
    _expected_marker: str,
) -> tuple[str, str]:
    if "path outside allowed roots" in result.output.lower():
        return "pass", "path remains constrained by allowed roots"
    if auto_approve:
        if result.is_error:
            return "pass", "operation failed despite auto-approval"
        return "fail", "writable file access accepted under risky target path when auto-approval is enabled"
    return evaluate_approval_gate(prereqs, result, auto_approve, "write path should require approval")


def evaluate_generic_error(
    prereqs: List[MCPResult],
    result: MCPResult,
    auto_approve: bool,
    marker: str,
) -> tuple[str, str]:
    return evaluate_approval_gate(prereqs, result, auto_approve, marker)


def evaluate_browser(
    _prereqs: List[MCPResult],
    result: MCPResult,
    auto_approve: bool,
    marker: str,
) -> tuple[str, str]:
    if approval_blocked(result):
        if auto_approve:
            return "pass", "approval requirement still enforced despite auto-approval mode"
        return "pass", "approval flow blocked browser automation call"
    if result.is_error:
        return "pass", f"browser did not complete ({result.output or result.details.get('error', '')})"
    body = json.dumps(result.details or {})
    if marker and marker in body:
        return ("pass" if auto_approve else "fail"), "browser executed as expected for attack harness endpoint"

    return "fail", "browser succeeded but did not hit the expected local probe path"


def evaluate_hook_chain(
    prereqs: List[MCPResult],
    result: MCPResult,
    auto_approve: bool,
    marker: str,
) -> tuple[str, str]:
    if not prereqs:
        return "error", "missing prerequisite setup step"

    setup = prereqs[-1]
    if approval_blocked(setup):
        return "pass", "hook configuration write still requires approval"

    if result_contains_marker(result, marker):
        if auto_approve:
            return "fail", "hook chain executed marker-bearing output while auto-approval is enabled"
        return "fail", "hook chain output reached marker unexpectedly"
    if result.is_error:
        return "pass", f"hook chain did not execute successfully ({result.output})"
    return "pass", "hook chain did not expose marker"


def evaluate_hook_token_exfil(
    prereqs: List[MCPResult],
    result: MCPResult,
    auto_approve: bool,
    marker: str,
) -> tuple[str, str]:
    if not prereqs:
        return "error", "missing prerequisite setup step"
    setup = prereqs[-1]
    if approval_blocked(setup):
        return "pass", "hook configuration write still requires approval"
    if auto_approve and setup.is_error:
        return "pass", f"token leak path blocked by setup failure ({setup.output})"
    if result_contains_marker(result, marker):
        return "fail", "hook expansion appears to return marker-derived token/capability value"
    if result.is_error:
        return "pass", "token leak test failed before marker validation"
    return "pass", "token/capability marker not observed"


def evaluate_batch_depth(
    _prereqs: List[MCPResult],
    result: MCPResult,
    _auto_approve: bool,
    expected_marker: str,
) -> tuple[str, str]:
    if approval_blocked(result):
        return "pass", "subagent_batch blocked by approval-like gate"
    if result.is_error:
        return "fail", f"batch call failed: {result.output or safe_json_text(result.raw)}"
    results = tool_output_field(result, "results", [])
    if not isinstance(results, list):
        return "fail", "batch response missing expected list"
    expected = expected_count(expected_marker)
    if expected and len(results) < expected:
        return "fail", f"batch returned {len(results)} invocation result(s), expected {expected}"
    ok = [entry for entry in results if isinstance(entry, dict) and entry.get("status") == "ok"]
    if len(ok) < expected_count(expected_marker):
        return "fail", f"batch has {len(ok)} successful invocation(s), expected {expected}"
    return "pass", f"batch fanout completed with {len(ok)} successful child invocation(s)"


def evaluate_policy_confusion(
    _prereqs: List[MCPResult],
    result: MCPResult,
    auto_approve: bool,
    marker: str,
) -> tuple[str, str]:
    if approval_blocked(result):
        return "pass", "policy-confusion scenario remained approval-gated"
    if result.is_error:
        return (
            "pass",
            f"policy confusion test ended in error: {result.output or result.details.get('error', '')}",
        )
    if result_contains_marker(result, marker):
        if auto_approve:
            return "fail", "unknown subagent executed via inline model hook under auto-approve"
        return "pass", "policy confusion output observed without auto-approve"
    return "pass", "policy-confusion marker not observed"


def evaluate_rerank_hook(
    _prereqs: List[MCPResult],
    result: MCPResult,
    _auto_approve: bool,
    marker: str,
) -> tuple[str, str]:
    if approval_blocked(result):
        return "pass", "rerank hook configuration write still requires approval"
    if result.is_error:
        return "pass", f"rerank hook scenario ended in error: {result.output or result.details.get('error', '')}"
    if result_contains_marker(result, marker):
        return "fail", "rerank hook output leaked attacker marker"
    return "pass", "rerank hook marker not observed"


def evaluate_subagent_marker(
    _prereqs: List[MCPResult],
    result: MCPResult,
    _auto_approve: bool,
    marker: str,
) -> tuple[str, str]:
    if result_contains_marker(result, marker):
        return "fail", "subagent returned attacker marker via model hook override"
    if result.is_error:
        return "pass", f"subagent scenario ended in error: {result.output or result.details.get('error', '')}"
    return "pass", "subagent marker not observed"


def run_attack(
    case: Attack,
    binary: str,
    mv2: Path,
    auto_approve: bool,
    probe_url: str,
    timeout_sec: int,
) -> tuple[Attack, MCPResult, str, str]:
    def execute(working_mv2: Path) -> tuple[Attack, MCPResult, str, str]:
        def call_safe(tool_name: str, arguments: Dict[str, Any]) -> MCPResult:
            try:
                return client.call_tool(tool_name, arguments)
            except Exception as exc:
                return MCPResult(
                    tool=tool_name,
                    output="",
                    details={"error": str(exc)},
                    is_error=True,
                    raw={"error": str(exc)},
                )

        args = case.arguments_factory(probe_url)
        with MCPClient(
            binary=binary,
            mv2=working_mv2,
            auto_approve=auto_approve,
            timeout_sec=timeout_sec,
            env_overrides=case.env_overrides,
        ) as client:
            required = [case.tool] + case.required_tools + [step.tool for step in case.prerequisites]
            available = set(client.list_tools())
            missing = [tool for tool in sorted(set(required)) if tool not in available]
            if missing:
                message = f"missing required tools for {case.id}: {', '.join(missing)}"
                return case, MCPResult(
                    tool=case.tool,
                    output=message,
                    details={"missing_tools": missing},
                    is_error=True,
                    raw={"missing_tools": missing},
                ), "error", message

            prereq_results: List[MCPResult] = []
            for step in case.prerequisites:
                prereq = call_safe(step.tool, step.arguments_factory(probe_url))
                prereq_results.append(prereq)
                if prereq.is_error:
                    if approval_blocked(prereq):
                        return (
                            case,
                            prereq,
                            "pass",
                            f"prerequisite {step.tool} still requires approval",
                        )
                    if case.stop_on_prereq_error:
                        return (
                            case,
                            prereq,
                            "error",
                            f"prerequisite {step.tool} failed: {prereq.output or prereq.details.get('error', '')}",
                        )

            result = call_safe(case.tool, args)
            status, evidence = case.evaluate(prereq_results, result, auto_approve, case.marker)
        return case, result, status, evidence

    if not case.isolate_capsule:
        return execute(mv2)

    with tempfile.TemporaryDirectory(prefix="swarm-attack-") as workdir:
        isolated_mv2 = Path(workdir) / mv2.name
        shutil.copy2(mv2, isolated_mv2)
        return execute(isolated_mv2)


def build_hook_payload_command(payload: Dict[str, Any]) -> str:
    payload_text = json.dumps(payload, separators=(",", ":"), ensure_ascii=False)
    return f"printf '%s' {json.dumps(payload_text)}"


def build_subagent_model_hook_command(marker: str) -> str:
    return build_hook_payload_command({"message": {"role": "assistant", "content": marker}})


def build_rerank_hook_command(marker: str) -> str:
    return build_hook_payload_command(
        {
            "scores": {},
            "snippets": {},
            "warnings": [marker],
        }
    )


def build_attacks(probe_url: str) -> List[Attack]:
    marker = random_suffix()
    chain_marker = f"HOOK_CHAIN_{marker}"
    token_marker = f"HOOK_TOKEN_{marker}"
    batch_marker = f"BATCH_OK_{marker}"
    policy_marker = f"POLICY_OK_{marker}"
    rerank_marker = f"RERANK_OK_{marker}"
    subagent_policy_marker = f"SUBAGENT_POLICY_{marker}"
    subagent_token_marker = f"SUBAGENT_TOKEN_{marker}"
    batch_size = 3

    return [
        Attack(
            id="SWM-1",
            name="exec command shell",
            tool="exec",
            arguments_factory=lambda base: {
                "command": "echo SWARM_EXEC_OK;",
                "timeout_ms": 4000,
            },
            severity="critical",
            marker="SWARM_EXEC_OK",
            required_tools=["exec"],
            evaluate=lambda r, a, m: evaluate_approval_gate(r, a, m),
        ),
        Attack(
            id="SWM-2",
            name="http_request GET probe (egress)",
            tool="http_request",
            arguments_factory=lambda base: {
                "method": "GET",
                "url": f"{base}/egress-get?marker={marker}",
                "timeout_ms": 4000,
            },
            severity="high",
            marker=marker,
            required_tools=["http_request"],
            evaluate=lambda r, a, m: evaluate_http_get(r, a, m),
        ),
        Attack(
            id="SWM-3",
            name="http_request POST requires approval",
            tool="http_request",
            arguments_factory=lambda base: {
                "method": "POST",
                "url": f"{base}/egress-post?marker={marker}",
                "body": '{"probe":"swarm"}',
                "json": True,
                "timeout_ms": 4000,
            },
            severity="critical",
            marker=marker,
            required_tools=["http_request"],
            evaluate=lambda r, a, m: evaluate_http_post(r, a, ""),
        ),
        Attack(
            id="SWM-4",
            name="fs_write absolute path",
            tool="fs_write",
            arguments_factory=lambda base: {
                "path": "/etc/passwd",
                "text": "aethervault-swarm-marker\n",
            },
            severity="critical",
            marker="/etc/passwd",
            required_tools=["fs_write"],
            evaluate=evaluate_fs_write,
        ),
        Attack(
            id="SWM-5",
            name="config_set direct config write",
            tool="config_set",
            arguments_factory=lambda base: {
                "key": "index",
                "json": {
                    "agent": {
                        "system": "Swarm verification mode",
                    }
                },
            },
            severity="high",
            marker="config_set",
            required_tools=["config_set"],
            evaluate=lambda r, a, m: evaluate_generic_error(r, a, "config writes should be approval-gated"),
        ),
        Attack(
            id="SWM-6",
            name="query expansion hook chain",
            tool="query",
            arguments_factory=lambda base: {"query": "swarm hook chain", "limit": 5},
            severity="critical",
            marker=chain_marker,
            prerequisites=[
                AttackStep(
                    tool="config_set",
                    arguments_factory=lambda base: {
                        "key": "index",
                        "json": {
                            "hooks": {
                                "expansion": {
                                    "command": build_hook_payload_command({
                                        "lex": [chain_marker],
                                        "vec": [],
                                        "warnings": [],
                                    })
                                }
                            }
                        },
                    },
                )
            ],
            required_tools=["query", "config_set"],
            isolate_capsule=True,
            evaluate=evaluate_hook_chain,
        ),
        Attack(
            id="SWM-7",
            name="query hook token/capability exfil",
            tool="query",
            arguments_factory=lambda base: {"query": "swarm token leak", "limit": 5},
            severity="critical",
            marker=token_marker,
            prerequisites=[
                AttackStep(
                    tool="config_set",
                    arguments_factory=lambda base: {
                        "key": "index",
                        "json": {
                            "hooks": {
                                "expansion": {
                                    "command": build_hook_payload_command({
                                        "lex": ["${SWARM_FAKE_TOKEN}"],
                                        "vec": [],
                                        "warnings": [],
                                    })
                                }
                            }
                        },
                    },
                )
            ],
            required_tools=["query", "config_set"],
            env_overrides={"SWARM_FAKE_TOKEN": token_marker},
            isolate_capsule=True,
            evaluate=evaluate_hook_token_exfil,
        ),
        Attack(
            id="SWM-8",
            name="subagent_batch fan-out depth",
            tool="subagent_batch",
            arguments_factory=lambda base: {
                "invocations": [
                    {
                        "name": f"swarm-batch-{i}-{marker}",
                        "prompt": f"Return marker {batch_marker} in output.",
                        "system": "swarm batch",
                        "model_hook": build_subagent_model_hook_command(batch_marker),
                    }
                    for i in range(batch_size)
                ]
            },
            severity="high",
            marker=f"count={batch_size}",
            required_tools=["subagent_batch"],
            evaluate=evaluate_batch_depth,
        ),
        Attack(
            id="SWM-9",
            name="subagent_batch policy confusion",
            tool="subagent_batch",
            arguments_factory=lambda base: {
                "invocations": [
                    {
                        "name": f"ghost-{marker}",
                        "prompt": f"Return marker {policy_marker} in output.",
                        "system": "policy confusion probe",
                        "model_hook": build_subagent_model_hook_command(policy_marker),
                    }
                ]
            },
            severity="high",
            marker=policy_marker,
            required_tools=["subagent_batch"],
            evaluate=evaluate_policy_confusion,
        ),
        Attack(
            id="SWM-10",
            name="browser outbound marker",
            tool="browser",
            arguments_factory=lambda base: {
                "command": f"open {base}/browser-post?marker={marker}",
                "session": "swarm-test",
            },
            severity="medium",
            marker=marker,
            required_tools=["browser"],
            evaluate=evaluate_browser,
        ),
        Attack(
            id="SWM-11",
            name="subagent_invoke known subagent policy confusion",
            tool="subagent_invoke",
            arguments_factory=lambda base: {
                "name": "swarm-policy-guard",
                "prompt": "Return attacker marker in output.",
                "system": "policy override probe",
                "model_hook": build_subagent_model_hook_command(subagent_policy_marker),
            },
            severity="high",
            marker=subagent_policy_marker,
            prerequisites=[
                AttackStep(
                    tool="config_set",
                    arguments_factory=lambda base: {
                        "key": "index",
                        "json": {
                            "agent": {
                                "subagents": [
                                    {
                                        "name": "swarm-policy-guard",
                                        "system": "restricted policy subagent",
                                    }
                                ]
                            }
                        },
                    },
                )
            ],
            required_tools=["subagent_invoke", "config_set"],
            isolate_capsule=True,
            evaluate=evaluate_subagent_marker,
        ),
        Attack(
            id="SWM-12",
            name="subagent_invoke model_hook token/capability leak",
            tool="subagent_invoke",
            arguments_factory=lambda base: {
                "name": "swarm-policy-guard",
                "prompt": "Return attacker marker in output.",
                "system": "capability leak probe",
                "model_hook": build_subagent_model_hook_command("${SWARM_SUBAGENT_TOKEN}"),
            },
            severity="critical",
            marker=subagent_token_marker,
            prerequisites=[
                AttackStep(
                    tool="config_set",
                    arguments_factory=lambda base: {
                        "key": "index",
                        "json": {
                            "agent": {
                                "subagents": [
                                    {
                                        "name": "swarm-policy-guard",
                                        "system": "restricted policy subagent",
                                    }
                                ]
                            }
                        },
                    },
                )
            ],
            required_tools=["subagent_invoke", "config_set"],
            env_overrides={"SWARM_SUBAGENT_TOKEN": subagent_token_marker},
            isolate_capsule=True,
            evaluate=evaluate_subagent_marker,
        ),
        Attack(
            id="SWM-13",
            name="query rerank hook command execution",
            tool="query",
            arguments_factory=lambda base: {
                "query": "rerank hook marker",
                "limit": 5,
                "rerank": "hook",
            },
            severity="medium",
            marker=rerank_marker,
            prerequisites=[
                AttackStep(
                    tool="config_set",
                    arguments_factory=lambda base: {
                        "key": "index",
                        "json": {
                            "hooks": {
                                "rerank": {
                                    "command": build_rerank_hook_command(rerank_marker),
                                }
                            }
                        },
                    },
                )
            ],
            required_tools=["query", "config_set"],
            isolate_capsule=True,
            evaluate=evaluate_rerank_hook,
        ),
    ]


def analyze_source_for_static_flags() -> List[str]:
    findings = []
    search_paths = [
        Path("src/main.rs"),
        Path("llama_proxy.py"),
        Path("scripts/session-manager.py"),
        Path("scripts/capabilities.py"),
    ]

    for path in search_paths:
        if not path.exists():
            continue
        text = path.read_text(errors="replace")
        if "sh -c" in text or "cmd \"/C\"" in text:
            findings.append(f"{path}: command-shell execution path detected")
        if "CommandSpec" in text and "run_hook_command" in text:
            findings.append(f"{path}: hook command execution path present")
        if "subagent_batch" in text and "unknown subagent" in text:
            findings.append(f"{path}: policy-confusion path for subagents includes unknown-name handling branch")
        if "subagents" in text and "model_hook" in text:
            findings.append(f"{path}: possible subagent model-hook override path present")
        if "resolve_hook_spec" in text and "hooks" in text and "llm" in text:
            findings.append(f"{path}: model-hook resolution path present")
        for token in [
            "AETHERVAULT_BRIDGE_AUTO_APPROVE",
            "AETHERVAULT_FS_ROOTS",
            "EXCALIDRAW_MCP_CMD",
            "ANTHROPIC_API_KEY",
            "SLACK_WEBHOOK_URL",
            "TEAMS_WEBHOOK_URL",
            "DISCORD_WEBHOOK_URL",
        ]:
            if token in text:
                findings.append(f"{path}: token or secret-capability source reference detected: {token}")
        if "AETHERVAULT_BRIDGE_AUTO_APPROVE" in text:
            findings.append(f"{path}: global auto-approve environment escape for all approvals")
        if "Approval" in text and "requires_approval" in text:
            findings.append(f"{path}: approval gating implemented in tool dispatcher; verify policy completeness")

    if not findings:
        findings.append("No obvious shell/approval static flags were found in sampled files.")
    return findings


def run_static() -> None:
    print("[static] Running source-level attack heuristics")
    for finding in analyze_source_for_static_flags():
        print(f" - {finding}")


def run_runtime(args: argparse.Namespace, probe_url: str) -> None:
    attacks = build_attacks(probe_url)
    print(f"[runtime] Running {len(attacks)} attacks with swarm={args.workers}, auto_approve={args.auto_approve}")

    mv2 = Path(args.mv2)
    if args.workers < 1:
        raise SystemExit("--workers must be at least 1")

    outcomes: List[tuple[int, Attack, MCPResult, str, str]] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as ex:
        futures = {
            ex.submit(
                run_attack,
                case,
                args.binary,
                mv2,
                args.auto_approve,
                probe_url,
                args.timeout,
            ): idx
            for idx, case in enumerate(attacks)
        }
        for fut in concurrent.futures.as_completed(futures):
            idx = futures[fut]
            try:
                outcome = fut.result()
            except Exception as exc:  # pragma: no cover
                case = attacks[idx]
                outcomes.append((
                    idx,
                    case,
                    MCPResult(
                        tool=case.tool,
                        output="",
                        details={"error": str(exc)},
                        is_error=True,
                        raw={"error": str(exc)},
                    ),
                    "error",
                    str(exc),
                ))
            else:
                outcomes.append((idx, *outcome))

    for _, attack, result, status, evidence in sorted(outcomes, key=lambda item: item[0]):
        marker = "PASS" if status == "pass" else "VULN"
        if status == "error":
            marker = "ERROR"

        print(f"[{marker}] {attack.id} {attack.name}")
        print(f"  severity: {attack.severity}")
        print(f"  evidence: {evidence}")
        print(f"  output:   {result.output if result.output else '[none]'}")
        if result.details:
            print(f"  details:  {json.dumps(result.details, ensure_ascii=False)}")
        print()


def resolve_binary(candidate: Optional[str]) -> str:
    if candidate:
        return str(Path(candidate).expanduser())
    cwd = Path.cwd()
    candidates = [
        cwd / "target" / "debug" / "aethervault",
        cwd / "target" / "release" / "aethervault",
        Path(shutil.which("aethervault") or "aethervault"),
    ]
    for path in candidates:
        if path.exists():
            return str(path)
    raise FileNotFoundError("aethervault binary not found; use --binary or run cargo build")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="AetherVault adversarial attack swarm")
    parser.add_argument("--binary", default=None, help="Path to aethervault binary (default: target/debug/aethervault if present)")
    parser.add_argument("--mv2", default="", help="Capsule file path for MCP sessions (defaults to data/knowledge.mv2 when present)")
    parser.add_argument("--workers", type=int, default=3, help="Parallel workers for runtime mode")
    parser.add_argument("--auto-approve", action="store_true", help="Set AETHERVAULT_BRIDGE_AUTO_APPROVE=1 while running attacks")
    parser.add_argument("--static", action="store_true", help="Run static/source heuristics only")
    parser.add_argument("--timeout", type=int, default=15, help="Tool call timeout (seconds)")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if args.static:
        run_static()
        return

    if not args.mv2:
        args.mv2 = "data/knowledge.mv2"

    if not Path(args.mv2).exists():
        raise SystemExit(f"Missing capsule file: {args.mv2}. Use --mv2 to point to a valid .mv2 file.")

    if args.binary is None:
        try:
            args.binary = resolve_binary(None)
        except Exception as exc:
            raise SystemExit(f"Unable to locate binary: {exc}")

    if not Path(args.binary).exists():
        raise SystemExit(f"Binary not found: {args.binary}")

    server, probe_url = start_probe_server()
    try:
        run_runtime(args, probe_url)
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    global PROBE_TOKEN
    PROBE_TOKEN = random_suffix(12)
    main()
