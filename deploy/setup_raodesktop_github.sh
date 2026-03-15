#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${GH_TOKEN:-}" ]]; then
  echo "GH_TOKEN is required" >&2
  exit 1
fi

mkdir -p "$HOME/.ssh" "$HOME/.local/bin" "$HOME/.config/gh"
chmod 700 "$HOME/.ssh"
touch "$HOME/.ssh/config"
chmod 600 "$HOME/.ssh/config"

if ! grep -q '^Host ssh.github.com$' "$HOME/.ssh/config" 2>/dev/null; then
  cat >>"$HOME/.ssh/config" <<'EOF'

Host ssh.github.com
  HostName ssh.github.com
  User git
  IdentityFile ~/.ssh/id_rsa_work
  Port 443
  IdentitiesOnly yes
EOF
fi

if ! grep -q '\.local/bin' "$HOME/.bashrc" 2>/dev/null; then
  echo 'export PATH="$HOME/.local/bin:$PATH"' >>"$HOME/.bashrc"
fi
export PATH="$HOME/.local/bin:$PATH"

if ! command -v gh >/dev/null 2>&1; then
  tmp="$(mktemp -d)"
  curl -fsSL -o "$tmp/gh.tgz" \
    "https://github.com/cli/cli/releases/download/v2.88.1/gh_2.88.1_linux_amd64.tar.gz"
  tar -xzf "$tmp/gh.tgz" -C "$tmp"
  install -m 0755 "$tmp/gh_2.88.1_linux_amd64/bin/gh" "$HOME/.local/bin/gh"
  rm -rf "$tmp"
fi

printf '%s' "$GH_TOKEN" | gh auth login --hostname github.com --git-protocol ssh --with-token >/tmp/gh-auth.log 2>&1 || true

gh --version | head -n 1
gh auth status
