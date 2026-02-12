#!/usr/bin/env python3
"""
AetherVault Battle Test - Interactive Test Runner for Telegram Bot on DigitalOcean
==================================================================================

This script orchestrates a comprehensive test suite against a running aethervault/aethervault
Telegram bot deployed on a DigitalOcean droplet. It uses SSH to monitor the droplet's
journalctl output, memory usage, and crash state while guiding the tester through
manual Telegram interactions.

Usage:
    python3 scripts/aethervault-battle-test.py --host <DROPLET_IP> [options]

    Options:
        --host          Droplet IP or hostname (required)
        --user          SSH user (default: root)
        --service       Systemd service name (default: auto-detect aethervault or aethervault)
        --bot-token     Telegram bot token (default: read from droplet env)
        --chat-id       Telegram chat ID to monitor (default: auto-detect from getUpdates)
        --skip-p2       Skip P2 (expected failure) tests
        --timeout        Global per-test timeout in seconds (default: 60)
        --report-dir    Directory for the output report (default: /tmp)

Requirements:
    - SSH access to the droplet (key-based auth)
    - The bot must be running on the droplet
    - Python 3.8+ with `requests` (stdlib otherwise)
    - The tester must have Telegram open to send messages to the bot
"""

import argparse
import datetime
import json
import os
import re
import subprocess
import sys
import textwrap
import time
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Optional

try:
    import requests
except ImportError:
    print("ERROR: 'requests' library is required. Install with: pip install requests")
    sys.exit(1)


# ---------------------------------------------------------------------------
# Constants & Enums
# ---------------------------------------------------------------------------

class Priority(Enum):
    P0 = "P0 - Must Pass"
    P1 = "P1 - Should Pass"
    P2 = "P2 - Known Gaps (Expected Failures)"


class TestResult(Enum):
    PASS = "PASS"
    FAIL = "FAIL"
    SKIP = "SKIP"
    EXPECTED_FAIL = "EXPECTED_FAIL"
    TIMEOUT = "TIMEOUT"


COLORS = {
    "reset": "\033[0m",
    "bold": "\033[1m",
    "red": "\033[91m",
    "green": "\033[92m",
    "yellow": "\033[93m",
    "blue": "\033[94m",
    "cyan": "\033[96m",
    "dim": "\033[2m",
}


def color(text: str, c: str) -> str:
    """Wrap text in ANSI color codes."""
    return f"{COLORS.get(c, '')}{text}{COLORS['reset']}"


def print_banner(text: str):
    width = 70
    border = "=" * width
    print(f"\n{color(border, 'cyan')}")
    print(color(f"  {text}", "bold"))
    print(f"{color(border, 'cyan')}\n")


def print_section(text: str):
    print(f"\n{color('--- ' + text + ' ---', 'blue')}\n")


# ---------------------------------------------------------------------------
# Data Classes
# ---------------------------------------------------------------------------

@dataclass
class TestCase:
    id: str
    name: str
    priority: Priority
    description: str
    user_message: str  # What the tester should send
    validation_hint: str  # How to judge pass/fail
    timeout_seconds: int = 60
    requires_prior: Optional[str] = None  # ID of a prerequisite test
    wait_before_seconds: int = 0  # Delay before this test (e.g., for session persistence)
    expected_fail: bool = False  # True for P2 known-gap tests
    manual_note: str = ""  # Extra instruction for the tester


@dataclass
class TestOutcome:
    test_id: str
    test_name: str
    priority: Priority
    result: TestResult
    response_time_seconds: float = 0.0
    response_text: str = ""
    notes: str = ""
    crash_detected: bool = False


@dataclass
class MonitorSnapshot:
    timestamp: str
    memory_rss_mb: float = 0.0
    memory_percent: float = 0.0
    cpu_percent: float = 0.0
    crash_count: int = 0
    panic_lines: list = field(default_factory=list)


# ---------------------------------------------------------------------------
# SSH Helper
# ---------------------------------------------------------------------------

class SSHRunner:
    """Execute commands on the remote droplet via SSH."""

    def __init__(self, host: str, user: str = "root", timeout: int = 30):
        self.host = host
        self.user = user
        self.timeout = timeout

    def run(self, command: str, timeout: Optional[int] = None) -> subprocess.CompletedProcess:
        t = timeout or self.timeout
        ssh_cmd = [
            "ssh",
            "-o", "ConnectTimeout=10",
            "-o", "StrictHostKeyChecking=no",
            "-o", "BatchMode=yes",
            f"{self.user}@{self.host}",
            command,
        ]
        try:
            result = subprocess.run(
                ssh_cmd,
                capture_output=True,
                text=True,
                timeout=t,
            )
            return result
        except subprocess.TimeoutExpired:
            return subprocess.CompletedProcess(
                args=ssh_cmd, returncode=124, stdout="", stderr="SSH command timed out"
            )

    def test_connection(self) -> bool:
        result = self.run("echo ok", timeout=15)
        return result.returncode == 0 and "ok" in result.stdout

    def get_service_name(self) -> Optional[str]:
        """Auto-detect whether the service is aethervault or aethervault."""
        for name in ["aethervault", "aethervault"]:
            result = self.run(f"systemctl is-active {name} 2>/dev/null || true")
            status = result.stdout.strip()
            if status == "active":
                return name
        # Check if either unit file exists even if not active
        for name in ["aethervault", "aethervault"]:
            result = self.run(f"systemctl cat {name} 2>/dev/null | head -1")
            if result.returncode == 0 and result.stdout.strip():
                return name
        return None

    def get_bot_token(self) -> Optional[str]:
        """Read TELEGRAM_BOT_TOKEN from the droplet's env files."""
        for env_path in [
            "/root/.aethervault/.env",
            "/root/.aethervault/.env",
            "/home/aethervault/.aethervault/.env",
        ]:
            result = self.run(f"grep -s '^TELEGRAM_BOT_TOKEN=' {env_path} 2>/dev/null || true")
            line = result.stdout.strip()
            if line and "=" in line:
                token = line.split("=", 1)[1].strip().strip('"').strip("'")
                if token and token != "YOUR_TELEGRAM_BOT_TOKEN" and len(token) > 10:
                    return token
        return None


# ---------------------------------------------------------------------------
# Telegram Bot API Helper
# ---------------------------------------------------------------------------

class TelegramAPI:
    """Minimal Telegram Bot API client using requests."""

    BASE = "https://api.telegram.org"

    def __init__(self, token: str):
        self.token = token
        self.base_url = f"{self.BASE}/bot{token}"
        self._last_update_id = 0

    def get_me(self) -> dict:
        resp = requests.get(f"{self.base_url}/getMe", timeout=10)
        resp.raise_for_status()
        data = resp.json()
        if not data.get("ok"):
            raise RuntimeError(f"getMe failed: {data}")
        return data["result"]

    def get_updates(self, offset: int = 0, timeout: int = 5) -> list:
        params = {"timeout": timeout, "allowed_updates": ["message"]}
        if offset:
            params["offset"] = offset
        try:
            resp = requests.get(
                f"{self.base_url}/getUpdates", params=params, timeout=timeout + 10
            )
            resp.raise_for_status()
            data = resp.json()
            if data.get("ok"):
                return data.get("result", [])
        except requests.RequestException:
            pass
        return []

    def flush_updates(self):
        """Consume all pending updates so we start fresh."""
        updates = self.get_updates(offset=0, timeout=1)
        if updates:
            max_id = max(u["update_id"] for u in updates)
            self._last_update_id = max_id + 1
            # Confirm the offset
            self.get_updates(offset=self._last_update_id, timeout=1)

    def poll_for_response(
        self, from_bot: bool, chat_id: int, after_time: float, timeout: int = 60
    ) -> Optional[dict]:
        """
        Poll getUpdates for a message in the given chat.

        If from_bot=True, look for messages where message.from.is_bot is True
        (i.e., the bot's own responses seen via getUpdates -- note: this only
        works if the bot uses the same token, which it does).

        In practice, getUpdates returns messages sent TO the bot by users.
        The bot's outgoing messages are NOT visible via getUpdates.
        So we use journalctl monitoring as the primary response detection.

        This method is kept for supplementary monitoring of user-sent messages.
        """
        deadline = time.time() + timeout
        while time.time() < deadline:
            updates = self.get_updates(offset=self._last_update_id, timeout=3)
            for update in updates:
                self._last_update_id = update["update_id"] + 1
                msg = update.get("message", {})
                msg_time = msg.get("date", 0)
                msg_chat = msg.get("chat", {}).get("id", 0)
                is_bot = msg.get("from", {}).get("is_bot", False)

                if msg_chat == chat_id and msg_time >= after_time:
                    if from_bot == is_bot:
                        return msg
            time.sleep(1)
        return None

    def detect_chat_id(self) -> Optional[int]:
        """Try to find the owner's chat ID from recent updates."""
        updates = self.get_updates(offset=0, timeout=2)
        for update in updates:
            msg = update.get("message", {})
            chat = msg.get("chat", {})
            if chat.get("type") == "private":
                return chat["id"]
        return None


# ---------------------------------------------------------------------------
# Journalctl Monitor
# ---------------------------------------------------------------------------

class JournalMonitor:
    """Monitor journalctl output on the remote droplet for bot responses and crashes."""

    def __init__(self, ssh: SSHRunner, service: str):
        self.ssh = ssh
        self.service = service

    def get_recent_logs(self, since_seconds: int = 120, lines: int = 200) -> str:
        """Fetch recent journalctl lines."""
        result = self.ssh.run(
            f"journalctl -u {self.service} --since '{since_seconds} seconds ago' "
            f"--no-pager -n {lines} 2>/dev/null || true",
            timeout=20,
        )
        return result.stdout

    def check_for_crashes(self, since_seconds: int = 300) -> list:
        """Scan journalctl for panics, segfaults, fatal errors."""
        patterns = [
            "panic",
            "segfault",
            "SIGSEGV",
            "SIGABRT",
            "fatal error",
            "unhandled exception",
            "JavaScript heap out of memory",
            "ENOMEM",
            "killed",
            "OOMKiller",
        ]
        result = self.ssh.run(
            f"journalctl -u {self.service} --since '{since_seconds} seconds ago' "
            f"--no-pager 2>/dev/null | grep -iE '{'|'.join(patterns)}' || true",
            timeout=20,
        )
        lines = [l.strip() for l in result.stdout.strip().split("\n") if l.strip()]
        return lines

    def wait_for_response_in_logs(
        self, marker: str, timeout: int = 60, poll_interval: int = 3
    ) -> tuple:
        """
        Poll journalctl looking for evidence the bot processed a message.

        We look for log lines that contain keywords suggesting the bot sent a reply,
        such as 'sendMessage', 'reply', or the marker text in outgoing content.

        Returns (found: bool, elapsed_seconds: float, matched_lines: list[str])
        """
        start = time.time()
        seen_lines = set()
        matched = []

        while (time.time() - start) < timeout:
            logs = self.get_recent_logs(since_seconds=timeout + 30, lines=300)
            for line in logs.split("\n"):
                line_stripped = line.strip()
                if not line_stripped or line_stripped in seen_lines:
                    continue
                seen_lines.add(line_stripped)

                # Look for indicators the bot processed and replied
                lower = line_stripped.lower()
                indicators = [
                    "sendmessage",
                    "send_message",
                    "reply sent",
                    "response sent",
                    "telegram: sent",
                    "tg: sent",
                    "message sent to",
                    "outgoing message",
                    "agent response",
                    "agent reply",
                    "completed request",
                    "tool_result",
                    "assistant message",
                ]
                for ind in indicators:
                    if ind in lower:
                        matched.append(line_stripped)
                        break

                # Also check if the marker text appears (case-insensitive)
                if marker.lower() in lower:
                    matched.append(line_stripped)

            if matched:
                elapsed = time.time() - start
                return True, elapsed, matched

            time.sleep(poll_interval)

        elapsed = time.time() - start
        return False, elapsed, matched


# ---------------------------------------------------------------------------
# System Monitor
# ---------------------------------------------------------------------------

class SystemMonitor:
    """Collect system metrics from the droplet."""

    def __init__(self, ssh: SSHRunner, service: str):
        self.ssh = ssh
        self.service = service
        self.snapshots: list = []

    def snapshot(self) -> MonitorSnapshot:
        ts = datetime.datetime.utcnow().isoformat() + "Z"
        snap = MonitorSnapshot(timestamp=ts)

        # Get memory for the main service process
        result = self.ssh.run(
            f"ps -C node -o rss=,pcpu=,%mem= --sort=-rss 2>/dev/null | head -1 || "
            f"ps aux | grep -E '(aethervault|aethervault)' | grep -v grep | "
            f"awk '{{print $6, $3, $4}}' | head -1",
            timeout=15,
        )
        parts = result.stdout.strip().split()
        if len(parts) >= 1:
            try:
                snap.memory_rss_mb = float(parts[0]) / 1024.0
            except (ValueError, IndexError):
                pass
        if len(parts) >= 2:
            try:
                snap.cpu_percent = float(parts[1])
            except (ValueError, IndexError):
                pass
        if len(parts) >= 3:
            try:
                snap.memory_percent = float(parts[2])
            except (ValueError, IndexError):
                pass

        # Crash count
        journal_mon = JournalMonitor(self.ssh, self.service)
        crash_lines = journal_mon.check_for_crashes(since_seconds=3600)
        snap.crash_count = len(crash_lines)
        snap.panic_lines = crash_lines[:5]  # Keep first 5

        self.snapshots.append(snap)
        return snap

    def get_uptime(self) -> str:
        result = self.ssh.run(
            f"systemctl show {self.service} --property=ActiveEnterTimestamp 2>/dev/null || true"
        )
        return result.stdout.strip()


# ---------------------------------------------------------------------------
# Test Definitions
# ---------------------------------------------------------------------------

def build_test_suite() -> list:
    """Define all P0, P1, P2 test cases."""
    tests = []

    # ---- P0: Must Pass ----

    tests.append(TestCase(
        id="P0-01",
        name="Basic greeting - hello",
        priority=Priority.P0,
        description="Send a simple greeting and expect a coherent reply within 30 seconds.",
        user_message="hello",
        validation_hint="Bot responds with a coherent greeting within 30s. Any text reply counts.",
        timeout_seconds=30,
    ))

    tests.append(TestCase(
        id="P0-02",
        name="Math - what is 2+2?",
        priority=Priority.P0,
        description="Send a simple arithmetic question and expect the correct answer.",
        user_message="what is 2+2?",
        validation_hint="Bot responds with '4' somewhere in the message.",
        timeout_seconds=30,
    ))

    tests.append(TestCase(
        id="P0-03",
        name="Command execution - list files in /tmp",
        priority=Priority.P0,
        description="Ask the bot to list files in /tmp to verify tool/command execution.",
        user_message="list files in /tmp",
        validation_hint="Bot runs a command (ls /tmp or similar) and returns output listing files.",
        timeout_seconds=45,
    ))

    tests.append(TestCase(
        id="P0-04",
        name="Rapid fire - 5 messages quickly",
        priority=Priority.P0,
        description="Send 5 messages in quick succession. All should eventually get responses.",
        user_message="(Send these 5 messages rapidly, ~1 second apart):\n"
                     "  1. 'ping 1'\n"
                     "  2. 'ping 2'\n"
                     "  3. 'ping 3'\n"
                     "  4. 'ping 4'\n"
                     "  5. 'ping 5'",
        validation_hint="All 5 messages get responses (may be batched). No crashes in journalctl.",
        timeout_seconds=120,
        manual_note="Send all 5 messages as fast as you can, about 1 second apart.",
    ))

    tests.append(TestCase(
        id="P0-05",
        name="Long message - 2000+ characters",
        priority=Priority.P0,
        description="Send a very long message (2000+ chars) and expect no truncation errors.",
        user_message=(
            "Please analyze the following long text and give me a one-sentence summary: "
            + "The quick brown fox jumps over the lazy dog. " * 60
            + "What was this text about?"
        ),
        validation_hint="Bot responds without errors or truncation. "
                        "Should reference the fox/dog content.",
        timeout_seconds=60,
        manual_note="Copy the entire long message above and paste it into Telegram.",
    ))

    tests.append(TestCase(
        id="P0-06",
        name="Crash check - no panics or segfaults",
        priority=Priority.P0,
        description="Check journalctl for any panics, segfaults, or fatal errors.",
        user_message="(No message needed - this is a system check)",
        validation_hint="Zero panics/segfaults/OOM in journalctl for the last 10 minutes.",
        timeout_seconds=10,
    ))

    # ---- P1: Should Pass ----

    tests.append(TestCase(
        id="P1-07",
        name="Memory set - remember my name",
        priority=Priority.P1,
        description="Tell the bot to remember a fact, then recall it.",
        user_message="Remember my name is Sunil",
        validation_hint="Bot acknowledges it will remember. This is part 1 of a 2-part test.",
        timeout_seconds=30,
    ))

    tests.append(TestCase(
        id="P1-08",
        name="Memory recall - what's my name?",
        priority=Priority.P1,
        description="Ask the bot to recall the name stored in the previous test.",
        user_message="What's my name?",
        validation_hint="Bot responds with 'Sunil' in the answer.",
        timeout_seconds=30,
        requires_prior="P1-07",
    ))

    tests.append(TestCase(
        id="P1-09",
        name="Web search - latest news",
        priority=Priority.P1,
        description="Ask the bot to search the web to verify web fetch / search tool.",
        user_message="Search the web for latest AI news today",
        validation_hint="Bot uses a web search/fetch tool and returns current news items.",
        timeout_seconds=90,
    ))

    tests.append(TestCase(
        id="P1-10",
        name="File write - create /tmp/test.txt",
        priority=Priority.P1,
        description="Ask the bot to create a file to verify file write tool.",
        user_message="Create a file called /tmp/aethervault-test.txt with the content 'hello world from battle test'",
        validation_hint="Bot confirms file created. We will verify via SSH.",
        timeout_seconds=45,
    ))

    tests.append(TestCase(
        id="P1-11",
        name="File read - read /tmp/test.txt",
        priority=Priority.P1,
        description="Ask the bot to read back the file created in the prior test.",
        user_message="Read the file /tmp/aethervault-test.txt and tell me what it contains",
        validation_hint="Bot returns 'hello world from battle test' or similar.",
        timeout_seconds=45,
        requires_prior="P1-10",
    ))

    tests.append(TestCase(
        id="P1-12",
        name="Multi-turn conversation (5+ exchanges)",
        priority=Priority.P1,
        description="Have a sustained conversation to test context management.",
        user_message=(
            "(Send these messages one at a time, waiting for each response):\n"
            "  1. 'Let's play a counting game. I'll say a number, you say the next.'\n"
            "  2. '1'\n"
            "  3. '3'\n"
            "  4. '5'\n"
            "  5. '7'\n"
            "  6. 'What number did we start with?'"
        ),
        validation_hint="Bot maintains context across all exchanges. "
                        "Final answer should reference starting with 1.",
        timeout_seconds=180,
        manual_note="Wait for each response before sending the next message.",
    ))

    tests.append(TestCase(
        id="P1-13",
        name="Session persistence - 10 minute gap",
        priority=Priority.P1,
        description="Wait 10 minutes then send a message referencing earlier context.",
        user_message="What did we talk about earlier?",
        validation_hint="Bot recalls elements of the earlier conversation "
                        "(counting game, name, files, etc.).",
        timeout_seconds=60,
        wait_before_seconds=600,
        manual_note="Wait a full 10 minutes before sending this message.",
    ))

    tests.append(TestCase(
        id="P1-14",
        name="Memory recall after gap",
        priority=Priority.P1,
        description="Verify long-term memory after the 10-minute gap.",
        user_message="Do you still remember my name?",
        validation_hint="Bot recalls 'Sunil' from the earlier memory test.",
        timeout_seconds=30,
        requires_prior="P1-13",
    ))

    # ---- P2: Known Gaps (Expected Failures) ----

    tests.append(TestCase(
        id="P2-15",
        name="Voice/audio message",
        priority=Priority.P2,
        description="Send a voice/audio message to check if it's handled.",
        user_message="(Record and send a short voice message in Telegram)",
        validation_hint="EXPECTED FAIL: Bot may not support voice. "
                        "Check it fails gracefully (no crash).",
        timeout_seconds=30,
        expected_fail=True,
        manual_note="Use Telegram's voice message feature (hold the mic button).",
    ))

    tests.append(TestCase(
        id="P2-16",
        name="Model switching - switch to sonnet",
        priority=Priority.P2,
        description="Ask to switch models. This may not be supported via Telegram.",
        user_message="Switch to sonnet model",
        validation_hint="EXPECTED FAIL: Bot may not support model switching via chat. "
                        "Should not crash.",
        timeout_seconds=30,
        expected_fail=True,
    ))

    tests.append(TestCase(
        id="P2-17",
        name="Image/vision test",
        priority=Priority.P2,
        description="Send an image to test vision capability.",
        user_message="(Send any image/photo in Telegram, then ask 'What do you see in this image?')",
        validation_hint="EXPECTED FAIL: Vision may not be wired up. "
                        "Should fail gracefully (no crash).",
        timeout_seconds=45,
        expected_fail=True,
        manual_note="Send a photo first, then the text message asking about it.",
    ))

    return tests


# ---------------------------------------------------------------------------
# Test Runner
# ---------------------------------------------------------------------------

class BattleTestRunner:
    """Orchestrates the interactive test execution."""

    def __init__(
        self,
        ssh: SSHRunner,
        service: str,
        bot_token: Optional[str],
        chat_id: Optional[int],
        global_timeout: int,
        skip_p2: bool,
    ):
        self.ssh = ssh
        self.service = service
        self.journal = JournalMonitor(ssh, service)
        self.sysmon = SystemMonitor(ssh, service)
        self.bot_token = bot_token
        self.telegram: Optional[TelegramAPI] = None
        self.chat_id = chat_id
        self.global_timeout = global_timeout
        self.skip_p2 = skip_p2
        self.outcomes: list = []
        self.start_time = None

        if bot_token:
            self.telegram = TelegramAPI(bot_token)

    def run_all(self):
        self.start_time = time.time()
        tests = build_test_suite()

        print_banner("AetherVault Battle Test Suite")
        print(f"  Droplet:    {self.ssh.host}")
        print(f"  Service:    {self.service}")
        print(f"  Bot token:  {'configured' if self.bot_token else 'NOT SET (journal-only mode)'}")
        print(f"  Chat ID:    {self.chat_id or 'auto-detect'}")
        print(f"  Tests:      {len(tests)}")
        print(f"  Skip P2:    {self.skip_p2}")
        print(f"  Started:    {datetime.datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")

        # Pre-flight checks
        self._preflight()

        # Flush telegram updates if available
        if self.telegram:
            print("\nFlushing old Telegram updates...")
            self.telegram.flush_updates()
            if not self.chat_id:
                self.chat_id = self.telegram.detect_chat_id()
                if self.chat_id:
                    print(f"  Auto-detected chat ID: {self.chat_id}")
                else:
                    print(f"  Could not auto-detect chat ID. "
                          f"Send any message to the bot first, then restart.")

        # Take initial snapshot
        print_section("Initial System Snapshot")
        snap = self.sysmon.snapshot()
        self._print_snapshot(snap)

        # Run each test
        for i, test in enumerate(tests, 1):
            if self.skip_p2 and test.priority == Priority.P2:
                self.outcomes.append(TestOutcome(
                    test_id=test.id,
                    test_name=test.name,
                    priority=test.priority,
                    result=TestResult.SKIP,
                    notes="P2 tests skipped via --skip-p2",
                ))
                continue

            self._run_single_test(test, i, len(tests))

        # Final snapshot
        print_section("Final System Snapshot")
        snap = self.sysmon.snapshot()
        self._print_snapshot(snap)

        # Generate report
        report = self._generate_report()
        return report

    def _preflight(self):
        print_section("Pre-flight Checks")

        # SSH connection
        print("  Checking SSH connection...", end=" ")
        if self.ssh.test_connection():
            print(color("OK", "green"))
        else:
            print(color("FAILED", "red"))
            print("  Cannot SSH into the droplet. Aborting.")
            sys.exit(1)

        # Service status
        print(f"  Checking {self.service} service...", end=" ")
        result = self.ssh.run(f"systemctl is-active {self.service}")
        status = result.stdout.strip()
        if status == "active":
            print(color("ACTIVE", "green"))
        else:
            print(color(f"STATUS: {status}", "red"))
            print(f"  WARNING: Service is not active. Tests may fail.")

        # Service uptime
        uptime_info = self.sysmon.get_uptime()
        print(f"  Service uptime: {uptime_info}")

        # Bot token validation
        if self.telegram:
            print("  Validating bot token...", end=" ")
            try:
                me = self.telegram.get_me()
                print(color(f"OK - @{me.get('username', '?')}", "green"))
            except Exception as e:
                print(color(f"FAILED: {e}", "red"))
                self.telegram = None

        # Check for existing crashes
        print("  Checking for pre-existing crashes...", end=" ")
        crashes = self.journal.check_for_crashes(since_seconds=600)
        if crashes:
            print(color(f"FOUND {len(crashes)} crash lines", "yellow"))
            for line in crashes[:3]:
                print(f"    {color(line[:120], 'dim')}")
        else:
            print(color("CLEAN", "green"))

    def _run_single_test(self, test: TestCase, index: int, total: int):
        print_section(f"Test {index}/{total}: [{test.id}] {test.name}")
        print(f"  Priority:    {test.priority.value}")
        print(f"  Description: {test.description}")
        print(f"  Timeout:     {test.timeout_seconds}s")
        if test.expected_fail:
            print(f"  {color('EXPECTED FAILURE - testing graceful handling', 'yellow')}")

        # Handle wait_before
        if test.wait_before_seconds > 0:
            wait_min = test.wait_before_seconds / 60
            print(f"\n  {color(f'WAIT: This test requires a {wait_min:.0f}-minute pause.', 'yellow')}")
            user_input = input(
                f"  Press ENTER to wait {wait_min:.0f} min, or 's' to skip, or 'n' for no wait: "
            ).strip().lower()
            if user_input == "s":
                self.outcomes.append(TestOutcome(
                    test_id=test.id,
                    test_name=test.name,
                    priority=test.priority,
                    result=TestResult.SKIP,
                    notes="Skipped by tester",
                ))
                return
            elif user_input != "n":
                print(f"  Waiting {test.wait_before_seconds} seconds...")
                time.sleep(test.wait_before_seconds)

        # Special case: P0-06 crash check (no user message needed)
        if test.id == "P0-06":
            self._run_crash_check(test)
            return

        # Display what the tester needs to do
        print(f"\n  {color('ACTION REQUIRED:', 'bold')} Send this message to the bot via Telegram:")
        print()
        for line in test.user_message.split("\n"):
            print(f"    {color(line, 'cyan')}")
        print()
        if test.manual_note:
            print(f"  {color('NOTE:', 'yellow')} {test.manual_note}")
            print()
        print(f"  Validation: {test.validation_hint}")
        print()

        # Wait for tester to send the message
        input(f"  Press ENTER after you've sent the message(s) to the bot...")

        send_time = time.time()

        # Monitor for response
        print(f"  Monitoring for response (timeout: {test.timeout_seconds}s)...")

        found, elapsed, matched_lines = self.journal.wait_for_response_in_logs(
            marker=test.user_message[:30] if len(test.user_message) < 100 else test.id,
            timeout=test.timeout_seconds,
            poll_interval=3,
        )

        # Also check for crashes during this test
        crashes = self.journal.check_for_crashes(since_seconds=test.timeout_seconds + 30)
        crash_detected = len(crashes) > 0

        if crash_detected:
            print(f"  {color('CRASH DETECTED during test!', 'red')}")
            for line in crashes[:3]:
                print(f"    {line[:120]}")

        # Ask the tester for the result since we can't see the bot's outgoing messages
        print()
        print(f"  {color('RESULTS CHECK:', 'bold')}")
        print(f"  Did the bot respond? (Check your Telegram chat)")
        print(f"  Validation criteria: {test.validation_hint}")
        print()

        while True:
            verdict = input(
                f"  Enter result - [p]ass / [f]ail / [s]kip / [t]imeout: "
            ).strip().lower()
            if verdict in ("p", "pass"):
                result = TestResult.PASS
                break
            elif verdict in ("f", "fail"):
                result = TestResult.EXPECTED_FAIL if test.expected_fail else TestResult.FAIL
                break
            elif verdict in ("s", "skip"):
                result = TestResult.SKIP
                break
            elif verdict in ("t", "timeout"):
                result = TestResult.TIMEOUT
                break
            else:
                print("  Please enter p, f, s, or t.")

        response_time = elapsed if found else (time.time() - send_time)

        notes_input = input("  Any notes? (press ENTER to skip): ").strip()

        # Record response text summary from logs if available
        response_summary = ""
        if matched_lines:
            response_summary = " | ".join(matched_lines[:3])[:500]

        self.outcomes.append(TestOutcome(
            test_id=test.id,
            test_name=test.name,
            priority=test.priority,
            result=result,
            response_time_seconds=round(response_time, 2),
            response_text=response_summary,
            notes=notes_input if notes_input else "",
            crash_detected=crash_detected,
        ))

        # Result summary
        result_color = {
            TestResult.PASS: "green",
            TestResult.FAIL: "red",
            TestResult.EXPECTED_FAIL: "yellow",
            TestResult.SKIP: "dim",
            TestResult.TIMEOUT: "red",
        }.get(result, "reset")

        print(f"\n  Result: {color(result.value, result_color)}")
        print(f"  Response time: {response_time:.1f}s")
        if crash_detected:
            print(f"  {color('CRASH: Yes', 'red')}")

    def _run_crash_check(self, test: TestCase):
        """Special handler for the crash-check-only test."""
        print("  Scanning journalctl for crashes in the last 10 minutes...")
        crashes = self.journal.check_for_crashes(since_seconds=600)

        if crashes:
            print(f"  {color(f'FOUND {len(crashes)} crash/error lines:', 'red')}")
            for line in crashes:
                print(f"    {line[:150]}")
            result = TestResult.FAIL
        else:
            print(f"  {color('No crashes or panics found.', 'green')}")
            result = TestResult.PASS

        self.outcomes.append(TestOutcome(
            test_id=test.id,
            test_name=test.name,
            priority=test.priority,
            result=result,
            response_time_seconds=0,
            notes=f"{len(crashes)} crash lines found" if crashes else "Clean",
            crash_detected=len(crashes) > 0,
        ))
        print(f"\n  Result: {color(result.value, 'green' if result == TestResult.PASS else 'red')}")

    def _print_snapshot(self, snap: MonitorSnapshot):
        print(f"  Timestamp:    {snap.timestamp}")
        print(f"  Memory RSS:   {snap.memory_rss_mb:.1f} MB")
        print(f"  Memory %:     {snap.memory_percent:.1f}%")
        print(f"  CPU %:        {snap.cpu_percent:.1f}%")
        print(f"  Crash lines:  {snap.crash_count}")

    def _generate_report(self) -> str:
        """Generate the final markdown report."""
        total_time = time.time() - self.start_time
        now = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")

        # Tally results
        p0_pass = sum(1 for o in self.outcomes if o.priority == Priority.P0 and o.result == TestResult.PASS)
        p0_total = sum(1 for o in self.outcomes if o.priority == Priority.P0 and o.result != TestResult.SKIP)
        p1_pass = sum(1 for o in self.outcomes if o.priority == Priority.P1 and o.result == TestResult.PASS)
        p1_total = sum(1 for o in self.outcomes if o.priority == Priority.P1 and o.result != TestResult.SKIP)
        p2_expected = sum(
            1 for o in self.outcomes
            if o.priority == Priority.P2 and o.result in (TestResult.EXPECTED_FAIL, TestResult.FAIL)
        )
        p2_pass = sum(1 for o in self.outcomes if o.priority == Priority.P2 and o.result == TestResult.PASS)
        p2_total = sum(1 for o in self.outcomes if o.priority == Priority.P2 and o.result != TestResult.SKIP)

        total_crashes = sum(1 for o in self.outcomes if o.crash_detected)
        avg_response = 0.0
        timed = [o for o in self.outcomes if o.response_time_seconds > 0 and o.result != TestResult.SKIP]
        if timed:
            avg_response = sum(o.response_time_seconds for o in timed) / len(timed)

        # Memory trend
        mem_values = [s.memory_rss_mb for s in self.sysmon.snapshots if s.memory_rss_mb > 0]
        mem_trend = "N/A"
        if len(mem_values) >= 2:
            delta = mem_values[-1] - mem_values[0]
            if delta > 50:
                mem_trend = f"INCREASING (+{delta:.0f} MB) -- potential leak"
            elif delta < -10:
                mem_trend = f"Decreasing ({delta:.0f} MB)"
            else:
                mem_trend = f"Stable (delta: {delta:+.0f} MB)"
        elif len(mem_values) == 1:
            mem_trend = f"{mem_values[0]:.0f} MB (single reading)"

        # Recommendation
        if p0_pass == p0_total and p0_total > 0 and total_crashes == 0:
            if p1_pass >= p1_total * 0.7:
                recommendation = "MIGRATE - All P0 tests pass, majority of P1 pass, no crashes."
            else:
                recommendation = (
                    "KEEP BOTH - P0 tests pass but P1 has gaps. "
                    "Run both old and new in parallel until P1 stabilizes."
                )
        elif p0_pass >= p0_total * 0.8 and total_crashes == 0:
            recommendation = (
                "KEEP BOTH - Most P0 pass but not all. "
                "Investigate failures before full migration."
            )
        else:
            recommendation = (
                "ROLLBACK - Critical P0 failures or crashes detected. "
                "Do not migrate until resolved."
            )

        lines = []
        lines.append("# AetherVault Battle Test Report")
        lines.append("")
        lines.append(f"**Date:** {now}")
        lines.append(f"**Droplet:** {self.ssh.host}")
        lines.append(f"**Service:** {self.service}")
        lines.append(f"**Total Duration:** {total_time / 60:.1f} minutes")
        lines.append("")
        lines.append("## Summary")
        lines.append("")
        lines.append(f"| Metric | Value |")
        lines.append(f"|--------|-------|")
        lines.append(f"| P0 (Must Pass) | {p0_pass}/{p0_total} |")
        lines.append(f"| P1 (Should Pass) | {p1_pass}/{p1_total} |")
        lines.append(f"| P2 (Expected Fail) | {p2_expected} expected, {p2_pass} surprise pass / {p2_total} total |")
        lines.append(f"| Avg Response Time | {avg_response:.1f}s |")
        lines.append(f"| Crashes Detected | {total_crashes} |")
        lines.append(f"| Memory Trend | {mem_trend} |")
        lines.append("")
        lines.append(f"## Recommendation")
        lines.append("")
        lines.append(f"**{recommendation}**")
        lines.append("")
        lines.append("## Detailed Results")
        lines.append("")
        lines.append("| ID | Test | Priority | Result | Time (s) | Crash | Notes |")
        lines.append("|-----|------|----------|--------|----------|-------|-------|")

        for o in self.outcomes:
            result_str = o.result.value
            crash_str = "YES" if o.crash_detected else "-"
            notes = o.notes[:80] if o.notes else "-"
            time_str = f"{o.response_time_seconds:.1f}" if o.response_time_seconds > 0 else "-"
            lines.append(
                f"| {o.test_id} | {o.test_name} | {o.priority.value.split(' - ')[0]} "
                f"| {result_str} | {time_str} | {crash_str} | {notes} |"
            )

        lines.append("")
        lines.append("## Memory Usage Snapshots")
        lines.append("")
        if self.sysmon.snapshots:
            lines.append("| Timestamp | RSS (MB) | Mem % | CPU % | Crashes |")
            lines.append("|-----------|----------|-------|-------|---------|")
            for s in self.sysmon.snapshots:
                lines.append(
                    f"| {s.timestamp} | {s.memory_rss_mb:.1f} | {s.memory_percent:.1f}% "
                    f"| {s.cpu_percent:.1f}% | {s.crash_count} |"
                )
        else:
            lines.append("No snapshots collected.")

        lines.append("")
        lines.append("## Crash Log Excerpts")
        lines.append("")
        all_panics = []
        for s in self.sysmon.snapshots:
            all_panics.extend(s.panic_lines)
        if all_panics:
            lines.append("```")
            for p in all_panics[:20]:
                lines.append(p[:200])
            lines.append("```")
        else:
            lines.append("No crashes detected.")

        lines.append("")
        lines.append("---")
        lines.append("*Generated by aethervault-battle-test.py*")

        return "\n".join(lines)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="AetherVault Battle Test - Interactive Telegram bot test runner",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=textwrap.dedent("""
            Examples:
              # Basic usage - auto-detect service and token from droplet
              python3 scripts/aethervault-battle-test.py --host YOUR_SERVER_IP

              # Explicit service and token
              python3 scripts/aethervault-battle-test.py --host YOUR_SERVER_IP \\
                  --service aethervault --bot-token "123456:ABC..."

              # Skip P2 tests, custom timeout
              python3 scripts/aethervault-battle-test.py --host YOUR_SERVER_IP \\
                  --skip-p2 --timeout 90

              # With doctl (get IP from droplet name)
              IP=$(doctl compute droplet list --format PublicIPv4 --no-header aethervault)
              python3 scripts/aethervault-battle-test.py --host $IP
        """),
    )
    parser.add_argument("--host", required=True, help="Droplet IP or hostname")
    parser.add_argument("--user", default="root", help="SSH user (default: root)")
    parser.add_argument("--service", default=None, help="Systemd service name (auto-detect if omitted)")
    parser.add_argument("--bot-token", default=None, help="Telegram bot token (reads from droplet if omitted)")
    parser.add_argument("--chat-id", type=int, default=None, help="Telegram chat ID to monitor")
    parser.add_argument("--skip-p2", action="store_true", help="Skip P2 (expected failure) tests")
    parser.add_argument("--timeout", type=int, default=60, help="Global per-test timeout (default: 60s)")
    parser.add_argument("--report-dir", default="/tmp", help="Directory for output report (default: /tmp)")

    args = parser.parse_args()

    # Also check env for bot token
    bot_token = args.bot_token or os.environ.get("TELEGRAM_BOT_TOKEN")

    ssh = SSHRunner(host=args.host, user=args.user)

    # Auto-detect service name
    service = args.service
    if not service:
        print("Auto-detecting service name on droplet...")
        service = ssh.get_service_name()
        if service:
            print(f"  Detected service: {service}")
        else:
            print("  Could not auto-detect. Defaulting to 'aethervault'.")
            service = "aethervault"

    # Auto-detect bot token from droplet if not provided
    if not bot_token:
        print("Reading bot token from droplet...")
        bot_token = ssh.get_bot_token()
        if bot_token:
            print(f"  Bot token found (ends with ...{bot_token[-6:]})")
        else:
            print("  WARNING: No bot token found. Running in journal-only mode.")
            print("  (Telegram API features will be disabled)")

    # Run the test suite
    runner = BattleTestRunner(
        ssh=ssh,
        service=service,
        bot_token=bot_token,
        chat_id=args.chat_id,
        global_timeout=args.timeout,
        skip_p2=args.skip_p2,
    )

    report = runner.run_all()

    # Save report
    timestamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    report_filename = f"aethervault-battle-report-{timestamp}.md"
    report_path = os.path.join(args.report_dir, report_filename)

    with open(report_path, "w") as f:
        f.write(report)

    print_banner("Test Suite Complete")
    print(f"  Report saved to: {report_path}")
    print()

    # Print summary to terminal too
    print(report)

    # Also try saving a copy in the project directory
    project_report = os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "..", "docs", report_filename
    )
    try:
        os.makedirs(os.path.dirname(project_report), exist_ok=True)
        with open(project_report, "w") as f:
            f.write(report)
        print(f"\n  Copy also saved to: {project_report}")
    except OSError:
        pass  # Non-critical

    # Exit code based on P0 results
    p0_failures = sum(
        1 for o in runner.outcomes
        if o.priority == Priority.P0 and o.result in (TestResult.FAIL, TestResult.TIMEOUT)
    )
    sys.exit(1 if p0_failures > 0 else 0)


if __name__ == "__main__":
    main()
