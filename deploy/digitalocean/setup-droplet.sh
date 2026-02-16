#!/bin/bash
# DigitalOcean Droplet Setup Script for AetherVault
# This script prepares a fresh Ubuntu droplet for AetherVault deployment

set -e

echo "=========================================="
echo "  AetherVault DigitalOcean Droplet Setup"
echo "=========================================="

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if running as root
if [ "$EUID" -ne 0 ]; then
    log_error "Please run as root (use sudo)"
    exit 1
fi

# Get the non-root user (the user who ran sudo)
SUDO_USER_HOME=$(eval echo ~$SUDO_USER)
if [ -z "$SUDO_USER" ] || [ "$SUDO_USER" = "root" ]; then
    log_warn "No sudo user detected. Creating 'aethervault' user (legacy username)..."
    AETHERVAULT_USER="aethervault"
    if ! id "$AETHERVAULT_USER" &>/dev/null; then
        useradd -m -s /bin/bash "$AETHERVAULT_USER"
        log_info "Created user: $AETHERVAULT_USER"
    fi
    SUDO_USER_HOME="/home/$AETHERVAULT_USER"
else
    AETHERVAULT_USER="$SUDO_USER"
fi

log_info "Setting up for user: $AETHERVAULT_USER"

# Update system packages
log_info "Updating system packages..."
apt-get update
apt-get upgrade -y

# Install essential packages
log_info "Installing essential packages..."
apt-get install -y \
    curl \
    wget \
    git \
    build-essential \
    unzip \
    htop \
    ufw \
    fail2ban \
    ca-certificates \
    gnupg

# Install Node.js 22.x (required by the aethervault npm package)
log_info "Installing Node.js 22.x..."
if ! command -v node &> /dev/null || [[ $(node -v | cut -d'.' -f1 | tr -d 'v') -lt 22 ]]; then
    curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
    apt-get install -y nodejs
    log_info "Node.js $(node -v) installed"
else
    log_info "Node.js $(node -v) already installed"
fi

# Install pnpm (preferred package manager for the aethervault npm package)
log_info "Installing pnpm..."
npm install -g pnpm@latest
log_info "pnpm $(pnpm -v) installed"

# Configure firewall
log_info "Configuring firewall..."
ufw default deny incoming
ufw default allow outgoing
ufw allow ssh
ufw allow 18789/tcp  # AetherVault gateway port
ufw allow 443/tcp    # HTTPS (for Tailscale Funnel if used)
ufw --force enable
log_info "Firewall configured"

# Configure fail2ban
log_info "Configuring fail2ban..."
systemctl enable fail2ban
systemctl start fail2ban

# Set up swap (useful for smaller droplets)
log_info "Setting up swap space..."
if [ ! -f /swapfile ]; then
    fallocate -l 2G /swapfile
    chmod 600 /swapfile
    mkswap /swapfile
    swapon /swapfile
    echo '/swapfile none swap sw 0 0' >> /etc/fstab
    log_info "2GB swap created"
else
    log_info "Swap already exists"
fi

# Create AetherVault directories (legacy aethervault paths retained for compatibility)
log_info "Creating AetherVault directories..."
sudo -u "$AETHERVAULT_USER" mkdir -p "$SUDO_USER_HOME/.aethervault"
sudo -u "$AETHERVAULT_USER" mkdir -p "$SUDO_USER_HOME/aethervault-workspace"

# Install Tailscale (optional but recommended for secure remote access)
log_info "Installing Tailscale..."
curl -fsSL https://tailscale.com/install.sh | sh
log_info "Tailscale installed. Run 'sudo tailscale up' to connect to your tailnet."

# Install agent-browser (browser automation CLI used by aethervault's browser tool)
log_info "Installing agent-browser..."
npm install -g agent-browser
agent-browser install
log_info "agent-browser $(agent-browser --version 2>/dev/null || echo 'unknown') installed"

# System optimizations
log_info "Applying system optimizations..."
cat >> /etc/sysctl.conf << 'EOF'
# AetherVault optimizations
net.core.somaxconn = 65535
net.ipv4.tcp_max_syn_backlog = 65535
fs.file-max = 65535
EOF
sysctl -p

# Increase file limits
cat >> /etc/security/limits.conf << 'EOF'
* soft nofile 65535
* hard nofile 65535
EOF

echo ""
echo "=========================================="
echo "  Droplet Setup Complete!"
echo "=========================================="
echo ""
log_info "Next steps:"
echo "  1. Run the AetherVault installation script:"
echo "     ./install.sh"
echo ""
echo "  2. Configure Tailscale for secure remote access:"
echo "     sudo tailscale up"
echo ""
echo "  3. Set up your API keys and channel tokens in:"
echo "     ~/.aethervault/aethervault.json"
echo ""
