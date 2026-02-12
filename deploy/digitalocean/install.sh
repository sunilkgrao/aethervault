#!/bin/bash
# AetherVault Installation Script (installs the aethervault npm runtime)
# Run this after setup-droplet.sh

set -e

echo "=========================================="
echo "  AetherVault Installation"
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

# Check Node.js version
NODE_VERSION=$(node -v 2>/dev/null | cut -d'.' -f1 | tr -d 'v')
if [ -z "$NODE_VERSION" ] || [ "$NODE_VERSION" -lt 22 ]; then
    log_error "Node.js 22+ is required. Please run setup-droplet.sh first."
    exit 1
fi
log_info "Node.js version: $(node -v)"

# Installation method selection
echo ""
echo "Select installation method:"
echo "  1) npm global install (recommended)"
echo "  2) pnpm global install"
echo "  3) Build from source"
echo ""
read -p "Enter choice [1-3]: " INSTALL_METHOD

case $INSTALL_METHOD in
    1)
        log_info "Installing aethervault runtime via npm..."
        npm install -g aethervault@latest
        ;;
    2)
        log_info "Installing aethervault runtime via pnpm..."
        pnpm add -g aethervault@latest
        ;;
    3)
        log_info "Building aethervault runtime from source..."

        # Clone repository
        if [ -d "$HOME/aethervault-src" ]; then
            log_info "Source directory exists, pulling latest..."
            cd "$HOME/aethervault-src"
            git pull
        else
            git clone https://github.com/aethervault/aethervault.git "$HOME/aethervault-src"
            cd "$HOME/aethervault-src"
        fi

        # Install dependencies and build
        pnpm install
        pnpm ui:build
        pnpm build

        # Link globally
        pnpm link --global
        ;;
    *)
        log_error "Invalid choice"
        exit 1
        ;;
esac

# Verify installation
if command -v aethervault &> /dev/null; then
    log_info "AetherVault (aethervault runtime) installed successfully!"
    aethervault --version
else
    log_error "AetherVault installation failed"
    exit 1
fi

# Create configuration directory
mkdir -p ~/.aethervault

# Check if config exists
if [ ! -f ~/.aethervault/aethervault.json ]; then
    log_info "Creating default configuration..."
    cat > ~/.aethervault/aethervault.json << 'EOF'
{
  "agent": {
    "model": "anthropic/claude-sonnet-4"
  },
  "gateway": {
    "port": 18789,
    "host": "0.0.0.0"
  },
  "sandbox": {
    "mode": "non-main"
  },
  "channels": {}
}
EOF
    log_info "Configuration created at ~/.aethervault/aethervault.json"
fi

# Run onboarding
echo ""
log_info "Running aethervault onboard..."
echo ""
aethervault onboard --install-daemon

# Run doctor to verify setup
echo ""
log_info "Running aethervault doctor to verify setup..."
aethervault doctor || true

echo ""
echo "=========================================="
echo "  AetherVault Installation Complete!"
echo "=========================================="
echo ""
log_info "Configuration file: ~/.aethervault/aethervault.json"
log_info "Workspace directory: ~/aethervault-workspace"
echo ""
log_info "Next steps:"
echo "  1. Add your API keys to ~/.aethervault/aethervault.json"
echo "  2. Configure your messaging channels (Telegram, Discord, etc.)"
echo "  3. Start the gateway: aethervault gateway --verbose"
echo ""
echo "Useful commands:"
echo "  aethervault gateway --port 18789 --verbose  # Start gateway"
echo "  aethervault doctor                          # Check configuration"
echo "  aethervault update --channel stable         # Update aethervault"
echo "  systemctl status aethervault                # Check service status"
echo ""
