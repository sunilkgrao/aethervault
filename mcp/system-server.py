#!/usr/bin/env python3
"""System MCP Server for AetherVault.

Provides system monitoring tools for the DigitalOcean droplet.
Uses the MCP Python SDK with FastMCP for stdio transport.
"""

import subprocess
import platform
import shutil
from mcp.server import FastMCP

mcp = FastMCP("aethervault-system", log_level="WARNING")


def _run_cmd(cmd: list[str], timeout: int = 10) -> str:
    """Run a shell command and return output."""
    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True, timeout=timeout
        )
        return result.stdout.strip() or result.stderr.strip()
    except subprocess.TimeoutExpired:
        return f"Error: Command timed out after {timeout}s"
    except Exception as e:
        return f"Error running command: {e}"


@mcp.tool()
def get_system_info() -> str:
    """Get system information: CPU, memory, disk usage, and OS details."""
    lines = []

    # OS info
    lines.append(f"OS: {platform.system()} {platform.release()}")
    lines.append(f"Hostname: {platform.node()}")
    lines.append(f"Architecture: {platform.machine()}")
    lines.append(f"Python: {platform.python_version()}")
    lines.append("")

    # CPU info
    cpu_count = _run_cmd(["nproc"])
    load = _run_cmd(["cat", "/proc/loadavg"])
    lines.append(f"CPU cores: {cpu_count}")
    lines.append(f"Load average: {load}")
    lines.append("")

    # Memory
    mem = _run_cmd(["free", "-h"])
    lines.append("Memory:")
    lines.append(mem)
    lines.append("")

    # Disk
    disk = _run_cmd(["df", "-h", "/"])
    lines.append("Disk (/):")
    lines.append(disk)

    return "\n".join(lines)


@mcp.tool()
def get_service_status(service_name: str) -> str:
    """Get the status of a systemd service.

    Args:
        service_name: Name of the systemd service (e.g. 'aethervault', 'nginx', 'sshd')
    """
    output = _run_cmd(["systemctl", "status", service_name, "--no-pager", "-l"])
    return output


@mcp.tool()
def list_services() -> str:
    """List all running systemd services."""
    output = _run_cmd(
        ["systemctl", "list-units", "--type=service", "--state=running", "--no-pager", "--no-legend"]
    )
    if not output:
        return "No running services found"

    lines = output.strip().split("\n")
    result = f"Running services ({len(lines)}):\n"
    for line in lines:
        parts = line.split()
        if parts:
            result += f"  {parts[0]}\n"
    return result


@mcp.tool()
def get_uptime() -> str:
    """Get system uptime and current time."""
    uptime = _run_cmd(["uptime", "-p"])
    since = _run_cmd(["uptime", "-s"])
    now = _run_cmd(["date", "+%Y-%m-%d %H:%M:%S %Z"])
    return f"Current time: {now}\nUp since: {since}\nUptime: {uptime}"


if __name__ == "__main__":
    import asyncio
    asyncio.run(mcp.run_stdio_async())
