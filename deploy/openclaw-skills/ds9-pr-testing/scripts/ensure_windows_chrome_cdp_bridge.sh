#!/usr/bin/env bash
set -euo pipefail

PS="/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
WIN_PY="/mnt/c/Windows/py.exe"

if [[ ! -x "$PS" ]]; then
  echo "missing powershell.exe at $PS" >&2
  exit 1
fi

if [[ ! -x "$WIN_PY" ]]; then
  echo "missing Windows py.exe at $WIN_PY" >&2
  exit 1
fi

WIN_PY_WIN="$("$PS" -NoProfile -Command "Join-Path \$env:WINDIR 'py.exe'" | tr -d '\r')"
if [[ -z "$WIN_PY_WIN" ]]; then
  echo "failed to resolve Windows py.exe path" >&2
  exit 1
fi

WIN_LOCALAPPDATA_WIN="$("$PS" -NoProfile -Command "[Environment]::GetFolderPath('LocalApplicationData')" | tr -d '\r')"
if [[ -z "$WIN_LOCALAPPDATA_WIN" ]]; then
  echo "failed to resolve LocalApplicationData" >&2
  exit 1
fi

WIN_LOCALAPPDATA="$(wslpath "$WIN_LOCALAPPDATA_WIN")"
WIN_BRIDGE_DIR="$WIN_LOCALAPPDATA/LinusChromeDS9"
WIN_PROXY_SCRIPT="$WIN_BRIDGE_DIR/windows_chrome_cdp_proxy.py"
WIN_STDOUT_LOG="$WIN_BRIDGE_DIR/windows_chrome_cdp_proxy.stdout.log"
WIN_STDERR_LOG="$WIN_BRIDGE_DIR/windows_chrome_cdp_proxy.stderr.log"
WIN_BRIDGE_DIR_WIN="${WIN_LOCALAPPDATA_WIN}\\LinusChromeDS9"
WIN_PROXY_SCRIPT_WIN="${WIN_BRIDGE_DIR_WIN}\\windows_chrome_cdp_proxy.py"
WIN_STDOUT_LOG_WIN="${WIN_BRIDGE_DIR_WIN}\\windows_chrome_cdp_proxy.stdout.log"
WIN_STDERR_LOG_WIN="${WIN_BRIDGE_DIR_WIN}\\windows_chrome_cdp_proxy.stderr.log"

mkdir -p "$WIN_BRIDGE_DIR"
cp "$(dirname "$0")/windows_chrome_cdp_proxy.py" "$WIN_PROXY_SCRIPT"

find_listener() {
  local port="$1"
  "$PS" -NoProfile -Command "(Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty LocalAddress) 2>\$null" | tr -d '\r'
}

chrome_listener="$(find_listener 9222)"
if [[ -z "$chrome_listener" ]]; then
  chrome_path="$("$PS" -NoProfile -Command "@('C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe','C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe') | Where-Object { Test-Path \$_ } | Select-Object -First 1" | tr -d '\r')"
  if [[ -z "$chrome_path" ]]; then
    echo "chrome-exe-not-found" >&2
    exit 2
  fi
  "$PS" -NoProfile -Command \
    "Start-Process -FilePath '$chrome_path' -ArgumentList @('--remote-debugging-address=127.0.0.1','--remote-debugging-port=9222','--user-data-dir=${WIN_BRIDGE_DIR_WIN}','--no-first-run','--no-default-browser-check')"
  for _ in $(seq 1 15); do
    sleep 1
    chrome_listener="$(find_listener 9222)"
    [[ -n "$chrome_listener" ]] && break
  done
  if [[ -z "$chrome_listener" ]]; then
    echo "chrome-cdp-not-listening" >&2
    exit 3
  fi
fi

start_bridge() {
  "$PS" -NoProfile -Command \
    "Start-Process -WindowStyle Hidden -FilePath '$WIN_PY_WIN' -ArgumentList '-3', '$WIN_PROXY_SCRIPT_WIN', '--listen-host', '0.0.0.0', '--listen-port', '9223', '--target-host', '127.0.0.1', '--target-port', '9222' -RedirectStandardOutput '$WIN_STDOUT_LOG_WIN' -RedirectStandardError '$WIN_STDERR_LOG_WIN'"
}

kill_bridge() {
  "$PS" -NoProfile -Command \
    "Get-CimInstance Win32_Process | Where-Object { \$_.CommandLine -like '*windows_chrome_cdp_proxy.py*' } | ForEach-Object { Invoke-CimMethod -InputObject \$_ -MethodName Terminate | Out-Null }"
}

bridge_listener="$(find_listener 9223)"
if [[ -z "$bridge_listener" ]]; then
  start_bridge
  sleep 2
  bridge_listener="$(find_listener 9223)"
fi

if ! python3 "$(dirname "$0")/resolve_windows_chrome_cdp.py" >/tmp/linus_chrome_cdp_endpoint.txt 2>/tmp/linus_chrome_cdp_endpoint.err; then
  kill_bridge
  sleep 1
  start_bridge
  sleep 2
fi

if ! python3 "$(dirname "$0")/resolve_windows_chrome_cdp.py" >/tmp/linus_chrome_cdp_endpoint.txt 2>/tmp/linus_chrome_cdp_endpoint.err; then
  cat /tmp/linus_chrome_cdp_endpoint.err >&2
  exit 4
fi

bridge_listener="$(find_listener 9223)"
if [[ -z "$bridge_listener" ]]; then
  echo "chrome-cdp-bridge-not-listening" >&2
  exit 5
fi

host_ip="$(ip route show default | awk '/default via/ {print $3; exit}')"
if [[ -z "$host_ip" ]]; then
  host_ip="$(awk '/nameserver/ {print $2; exit}' /etc/resolv.conf)"
fi
endpoint="$(cat /tmp/linus_chrome_cdp_endpoint.txt)"

echo "chrome_listener=$chrome_listener"
echo "bridge_listener=$bridge_listener"
echo "host_ip=$host_ip"
echo "endpoint=$endpoint"
