#!/usr/bin/env python3
"""
AetherVault Infrastructure Scaling Tool
Monitor resource usage and scale the DigitalOcean droplet up/down.

Usage (called by the agent via exec tool):
    python3 /root/.aethervault/hooks/scale.py status
    python3 /root/.aethervault/hooks/scale.py sizes
    python3 /root/.aethervault/hooks/scale.py resize --size s-2vcpu-4gb
    python3 /root/.aethervault/hooks/scale.py resize --size s-2vcpu-4gb --confirm

Requires:
    DO_TOKEN          - DigitalOcean API token (for sizes/resize)
    DO_DROPLET_ID     - Droplet ID (for resize; auto-detected via metadata API if unset)
"""

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

AETHERVAULT_HOME = os.environ.get("AETHERVAULT_HOME", os.path.expanduser("~/.aethervault"))
ENV_FILE = os.path.join(AETHERVAULT_HOME, ".env")

# Safety limits: max size the agent can scale to (prevent runaway costs)
MAX_VCPUS = 8
MAX_MEMORY_MB = 32768  # 32 GB
BUDGET_MONTHLY_MAX = 96  # USD


def load_env():
    """Load environment variables from .env file."""
    env = os.environ.copy()
    if os.path.exists(ENV_FILE):
        with open(ENV_FILE) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith('#') and '=' in line:
                    key, _, value = line.partition('=')
                    env[key.strip()] = value.strip()
    return env


def get_do_token():
    """Get DO_TOKEN from env, loading .env if needed."""
    env = load_env()
    token = env.get("DO_TOKEN", "")
    if not token:
        return None
    return token


def get_droplet_id():
    """Get droplet ID from env or auto-detect via DO metadata API."""
    env = load_env()
    droplet_id = env.get("DO_DROPLET_ID", "")
    if droplet_id:
        return droplet_id

    # Auto-detect via DigitalOcean metadata API (only works on DO droplets)
    try:
        result = subprocess.run(
            ["curl", "-s", "--connect-timeout", "2",
             "http://169.254.169.254/metadata/v1/id"],
            capture_output=True, text=True, timeout=5
        )
        if result.returncode == 0 and result.stdout.strip().isdigit():
            return result.stdout.strip()
    except Exception:
        pass

    return None


def do_api(method, endpoint, data=None):
    """Call the DigitalOcean API. Returns parsed JSON or raises."""
    token = get_do_token()
    if not token:
        return {"error": "DO_TOKEN not set. Add it to ~/.aethervault/.env"}

    url = f"https://api.digitalocean.com/v2{endpoint}"
    cmd = [
        "curl", "-s", "-X", method, url,
        "-H", f"Authorization: Bearer {token}",
        "-H", "Content-Type: application/json",
    ]
    if data:
        cmd.extend(["-d", json.dumps(data)])

    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
        if result.returncode != 0:
            return {"error": f"curl failed: {result.stderr}"}
        return json.loads(result.stdout) if result.stdout.strip() else {}
    except subprocess.TimeoutExpired:
        return {"error": "API request timed out"}
    except json.JSONDecodeError:
        return {"error": f"Invalid JSON response: {result.stdout[:200]}"}


def cmd_status(args):
    """Report current CPU, RAM, disk, and load average."""
    info = {}

    # CPU count
    try:
        with open("/proc/cpuinfo") as f:
            info["cpu_count"] = sum(1 for line in f if line.startswith("processor"))
    except FileNotFoundError:
        # macOS fallback
        try:
            result = subprocess.run(["sysctl", "-n", "hw.ncpu"],
                                    capture_output=True, text=True, timeout=5)
            info["cpu_count"] = int(result.stdout.strip())
        except Exception:
            info["cpu_count"] = None

    # Load average
    try:
        with open("/proc/loadavg") as f:
            parts = f.read().split()
            info["load_1m"] = float(parts[0])
            info["load_5m"] = float(parts[1])
            info["load_15m"] = float(parts[2])
    except FileNotFoundError:
        try:
            result = subprocess.run(["sysctl", "-n", "vm.loadavg"],
                                    capture_output=True, text=True, timeout=5)
            # macOS format: "{ 1.23 4.56 7.89 }"
            nums = [x for x in result.stdout.strip().strip("{}").split() if x]
            if len(nums) >= 3:
                info["load_1m"] = float(nums[0])
                info["load_5m"] = float(nums[1])
                info["load_15m"] = float(nums[2])
        except Exception:
            pass

    # Memory
    try:
        with open("/proc/meminfo") as f:
            meminfo = {}
            for line in f:
                parts = line.split()
                key = parts[0].rstrip(':')
                meminfo[key] = int(parts[1])  # in kB

            total_mb = meminfo.get("MemTotal", 0) // 1024
            avail_mb = meminfo.get("MemAvailable", 0) // 1024
            used_mb = total_mb - avail_mb
            info["mem_total_mb"] = total_mb
            info["mem_avail_mb"] = avail_mb
            info["mem_used_pct"] = round(used_mb / total_mb * 100, 1) if total_mb else 0
    except FileNotFoundError:
        # macOS fallback
        try:
            result = subprocess.run(["sysctl", "-n", "hw.memsize"],
                                    capture_output=True, text=True, timeout=5)
            total_bytes = int(result.stdout.strip())
            info["mem_total_mb"] = total_bytes // (1024 * 1024)
            # vm_stat for usage on macOS
            result = subprocess.run(["vm_stat"], capture_output=True, text=True, timeout=5)
            pages = {}
            for line in result.stdout.split('\n'):
                if ':' in line:
                    k, _, v = line.partition(':')
                    v = v.strip().rstrip('.')
                    if v.isdigit():
                        pages[k.strip()] = int(v)
            page_size = 16384  # ARM Mac default
            free_pages = pages.get("Pages free", 0) + pages.get("Pages speculative", 0)
            info["mem_avail_mb"] = (free_pages * page_size) // (1024 * 1024)
            used_mb = info["mem_total_mb"] - info["mem_avail_mb"]
            info["mem_used_pct"] = round(used_mb / info["mem_total_mb"] * 100, 1)
        except Exception:
            pass

    # Disk usage
    try:
        result = subprocess.run(["df", "-BG", "/"], capture_output=True, text=True, timeout=5)
        if result.returncode == 0:
            lines = result.stdout.strip().split('\n')
            if len(lines) >= 2:
                parts = lines[1].split()
                info["disk_total_gb"] = int(parts[1].rstrip('G'))
                info["disk_used_gb"] = int(parts[2].rstrip('G'))
                info["disk_used_pct"] = int(parts[4].rstrip('%'))
    except Exception:
        # macOS df doesn't support -BG, use -g
        try:
            result = subprocess.run(["df", "-g", "/"], capture_output=True, text=True, timeout=5)
            if result.returncode == 0:
                lines = result.stdout.strip().split('\n')
                if len(lines) >= 2:
                    parts = lines[1].split()
                    info["disk_total_gb"] = int(parts[1])
                    info["disk_used_gb"] = int(parts[2])
                    pct = parts[4] if len(parts) > 4 else parts[7]
                    info["disk_used_pct"] = int(pct.rstrip('%'))
        except Exception:
            pass

    print(json.dumps(info))


def cmd_sizes(args):
    """List available DigitalOcean droplet sizes with pricing."""
    resp = do_api("GET", "/sizes")
    if "error" in resp:
        print(json.dumps(resp))
        return

    sizes = resp.get("sizes", [])
    filtered = []
    for s in sizes:
        if not s.get("available", False):
            continue
        vcpus = s.get("vcpus", 0)
        memory = s.get("memory", 0)
        price = s.get("price_monthly", 0)

        # Safety filter: only show sizes within budget/resource limits
        if vcpus > MAX_VCPUS or memory > MAX_MEMORY_MB or price > BUDGET_MONTHLY_MAX:
            continue

        filtered.append({
            "slug": s["slug"],
            "vcpus": vcpus,
            "memory_mb": memory,
            "disk_gb": s.get("disk", 0),
            "price_monthly": price,
            "regions": len(s.get("regions", [])),
        })

    # Sort by price
    filtered.sort(key=lambda x: x["price_monthly"])

    print(json.dumps({
        "sizes": filtered,
        "total": len(filtered),
        "limits": {
            "max_vcpus": MAX_VCPUS,
            "max_memory_mb": MAX_MEMORY_MB,
            "budget_monthly_max": BUDGET_MONTHLY_MAX,
        },
    }))


def cmd_resize(args):
    """Resize the droplet. Requires --size and --confirm."""
    target_size = args.size
    if not target_size:
        print(json.dumps({"error": "Missing --size parameter. Example: --size s-2vcpu-4gb"}))
        return

    if not args.confirm:
        print(json.dumps({
            "error": "Resize requires --confirm flag. This is a destructive operation that may restart the droplet.",
            "hint": f"Re-run with: scale.py resize --size {target_size} --confirm",
        }))
        return

    # Get droplet ID
    droplet_id = get_droplet_id()
    if not droplet_id:
        print(json.dumps({
            "error": "Cannot determine droplet ID. Set DO_DROPLET_ID in ~/.aethervault/.env or run on a DigitalOcean droplet.",
        }))
        return

    # Validate target size exists and is within limits
    sizes_resp = do_api("GET", "/sizes")
    if "error" in sizes_resp:
        print(json.dumps(sizes_resp))
        return

    valid_size = None
    for s in sizes_resp.get("sizes", []):
        if s["slug"] == target_size and s.get("available", False):
            valid_size = s
            break

    if not valid_size:
        print(json.dumps({"error": f"Size '{target_size}' not found or not available"}))
        return

    # Safety checks
    if valid_size.get("vcpus", 0) > MAX_VCPUS:
        print(json.dumps({"error": f"Size exceeds max vCPU limit ({MAX_VCPUS})"}))
        return
    if valid_size.get("memory", 0) > MAX_MEMORY_MB:
        print(json.dumps({"error": f"Size exceeds max memory limit ({MAX_MEMORY_MB} MB)"}))
        return
    if valid_size.get("price_monthly", 0) > BUDGET_MONTHLY_MAX:
        print(json.dumps({"error": f"Size exceeds monthly budget limit (${BUDGET_MONTHLY_MAX})"}))
        return

    # Check current droplet info
    droplet_resp = do_api("GET", f"/droplets/{droplet_id}")
    if "error" in droplet_resp:
        print(json.dumps(droplet_resp))
        return

    current = droplet_resp.get("droplet", {})
    current_size = current.get("size_slug", "unknown")
    current_status = current.get("status", "unknown")

    if current_size == target_size:
        print(json.dumps({
            "message": f"Droplet is already running {target_size}. No resize needed.",
            "current_size": current_size,
        }))
        return

    # Perform the resize
    resize_resp = do_api("POST", f"/droplets/{droplet_id}/actions", {
        "type": "resize",
        "size": target_size,
    })

    if "error" in resize_resp:
        print(json.dumps(resize_resp))
        return

    action = resize_resp.get("action", {})
    print(json.dumps({
        "resize_initiated": True,
        "action_id": action.get("id"),
        "action_status": action.get("status"),
        "from_size": current_size,
        "to_size": target_size,
        "droplet_id": droplet_id,
        "droplet_status": current_status,
        "note": "CPU resizes require the droplet to be powered off first. Disk-only resizes can be live. Check action status via DO dashboard.",
        "new_price_monthly": valid_size.get("price_monthly"),
    }))


def main():
    parser = argparse.ArgumentParser(description="AetherVault Infrastructure Scaling Tool")
    subparsers = parser.add_subparsers(dest="command", required=True)

    # status
    subparsers.add_parser("status", help="Show current CPU, RAM, disk, and load average")

    # sizes
    subparsers.add_parser("sizes", help="List available DigitalOcean droplet sizes")

    # resize
    rp = subparsers.add_parser("resize", help="Resize the droplet")
    rp.add_argument("--size", "-s", required=True, help="Target size slug (e.g. s-2vcpu-4gb)")
    rp.add_argument("--confirm", action="store_true", help="Confirm the resize operation")

    args = parser.parse_args()

    {
        "status": cmd_status,
        "sizes": cmd_sizes,
        "resize": cmd_resize,
    }[args.command](args)


if __name__ == "__main__":
    main()
