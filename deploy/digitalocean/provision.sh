#!/bin/bash
# AetherVault DigitalOcean Provisioning Script
# Run this on your local machine to create and configure the droplet

set -e

# ============================================
# CONFIGURATION - Edit these values
# ============================================
DO_TOKEN="${DO_TOKEN:?DO_TOKEN must be set in environment or .env}"
DROPLET_NAME="${DROPLET_NAME:-aethervault}"
REGION="${REGION:-nyc1}"
SIZE="${SIZE:-s-1vcpu-2gb}"
IMAGE="${IMAGE:-ubuntu-24-04-x64}"

# ============================================
# Colors
# ============================================
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }
log_step() { echo -e "${BLUE}[STEP]${NC} $1"; }

# ============================================
# API Helper Functions
# ============================================
do_api() {
    local method=$1
    local endpoint=$2
    local data=$3

    if [ -n "$data" ]; then
        curl -s -X "$method" \
            "https://api.digitalocean.com/v2$endpoint" \
            -H "Authorization: Bearer $DO_TOKEN" \
            -H "Content-Type: application/json" \
            -d "$data"
    else
        curl -s -X "$method" \
            "https://api.digitalocean.com/v2$endpoint" \
            -H "Authorization: Bearer $DO_TOKEN" \
            -H "Content-Type: application/json"
    fi
}

# ============================================
# Main Script
# ============================================
echo "=========================================="
echo "  AetherVault DigitalOcean Provisioning"
echo "=========================================="
echo ""

# Step 1: Verify API Token
log_step "1/6 Verifying API token..."
ACCOUNT=$(do_api GET "/account")
if echo "$ACCOUNT" | grep -q '"status"'; then
    EMAIL=$(echo "$ACCOUNT" | grep -o '"email":"[^"]*"' | cut -d'"' -f4)
    log_info "Authenticated as: $EMAIL"
else
    log_error "Invalid API token"
    echo "$ACCOUNT"
    exit 1
fi

# Step 2: Check for existing SSH keys
log_step "2/6 Checking SSH keys..."
SSH_KEYS=$(do_api GET "/account/keys")
SSH_KEY_IDS=$(echo "$SSH_KEYS" | grep -o '"id":[0-9]*' | head -3 | cut -d':' -f2 | tr '\n' ',' | sed 's/,$//')

if [ -z "$SSH_KEY_IDS" ]; then
    log_warn "No SSH keys found in your DigitalOcean account!"
    echo ""
    echo "You need to add an SSH key to access the droplet."
    echo "Add one at: https://cloud.digitalocean.com/account/security"
    echo ""

    # Check for local SSH key to upload
    if [ -f "$HOME/.ssh/id_rsa.pub" ]; then
        read -p "Upload ~/.ssh/id_rsa.pub to DigitalOcean? [y/N] " UPLOAD_KEY
        if [ "$UPLOAD_KEY" = "y" ] || [ "$UPLOAD_KEY" = "Y" ]; then
            PUB_KEY=$(cat "$HOME/.ssh/id_rsa.pub")
            UPLOAD_RESULT=$(do_api POST "/account/keys" "{\"name\":\"aethervault-key\",\"public_key\":\"$PUB_KEY\"}")
            SSH_KEY_IDS=$(echo "$UPLOAD_RESULT" | grep -o '"id":[0-9]*' | cut -d':' -f2)
            log_info "SSH key uploaded with ID: $SSH_KEY_IDS"
        else
            log_error "Cannot proceed without SSH key"
            exit 1
        fi
    elif [ -f "$HOME/.ssh/id_ed25519.pub" ]; then
        read -p "Upload ~/.ssh/id_ed25519.pub to DigitalOcean? [y/N] " UPLOAD_KEY
        if [ "$UPLOAD_KEY" = "y" ] || [ "$UPLOAD_KEY" = "Y" ]; then
            PUB_KEY=$(cat "$HOME/.ssh/id_ed25519.pub")
            UPLOAD_RESULT=$(do_api POST "/account/keys" "{\"name\":\"aethervault-key\",\"public_key\":\"$PUB_KEY\"}")
            SSH_KEY_IDS=$(echo "$UPLOAD_RESULT" | grep -o '"id":[0-9]*' | cut -d':' -f2)
            log_info "SSH key uploaded with ID: $SSH_KEY_IDS"
        else
            log_error "Cannot proceed without SSH key"
            exit 1
        fi
    else
        log_error "No local SSH key found. Generate one with: ssh-keygen -t ed25519"
        exit 1
    fi
else
    log_info "Found SSH keys: $SSH_KEY_IDS"
fi

# Step 3: Create user-data script (cloud-init)
log_step "3/6 Preparing cloud-init script..."

USER_DATA=$(cat << 'USERDATA'
#!/bin/bash
set -e

# Update and install essentials
apt-get update
apt-get upgrade -y
apt-get install -y curl wget git build-essential ufw fail2ban

# Install Node.js 22
curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
apt-get install -y nodejs

# Install pnpm
npm install -g pnpm@latest

# Configure firewall
ufw default deny incoming
ufw default allow outgoing
ufw allow ssh
ufw allow 18789/tcp
ufw --force enable

# Enable fail2ban
systemctl enable fail2ban
systemctl start fail2ban

# Create swap
if [ ! -f /swapfile ]; then
    fallocate -l 2G /swapfile
    chmod 600 /swapfile
    mkswap /swapfile
    swapon /swapfile
    echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi

# Create aethervault user (legacy username retained for compatibility)
useradd -m -s /bin/bash aethervault || true
usermod -aG sudo aethervault

# Install the aethervault npm package as the aethervault user
sudo -u aethervault bash << 'EOF'
cd ~
npm install -g aethervault@latest
mkdir -p ~/.aethervault ~/aethervault-workspace

# Create default config
cat > ~/.aethervault/aethervault.json << 'CONFIG'
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
CONFIG
EOF

# Create systemd service for AetherVault gateway
cat > /etc/systemd/system/aethervault.service << 'SERVICE'
[Unit]
Description=AetherVault Gateway (aethervault runtime)
After=network.target

[Service]
Type=simple
User=aethervault
WorkingDirectory=/home/aethervault/aethervault-workspace
Environment=NODE_ENV=production
Environment=HOME=/home/aethervault
ExecStart=/usr/bin/aethervault gateway --port 18789 --verbose
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
SERVICE

systemctl daemon-reload
systemctl enable aethervault

# Signal completion (legacy filename retained for compatibility)
touch /root/aethervault-setup-complete
USERDATA
)

# Step 4: Create the droplet
log_step "4/6 Creating droplet..."

# Build SSH keys array
SSH_KEYS_JSON="[$(echo $SSH_KEY_IDS | sed 's/,/,/g')]"

CREATE_RESPONSE=$(do_api POST "/droplets" "{
    \"name\": \"$DROPLET_NAME\",
    \"region\": \"$REGION\",
    \"size\": \"$SIZE\",
    \"image\": \"$IMAGE\",
    \"ssh_keys\": $SSH_KEYS_JSON,
    \"backups\": false,
    \"ipv6\": true,
    \"user_data\": $(echo "$USER_DATA" | jq -Rs .),
    \"tags\": [\"aethervault\"]
}")

DROPLET_ID=$(echo "$CREATE_RESPONSE" | grep -o '"id":[0-9]*' | head -1 | cut -d':' -f2)

if [ -z "$DROPLET_ID" ]; then
    log_error "Failed to create droplet"
    echo "$CREATE_RESPONSE" | jq . 2>/dev/null || echo "$CREATE_RESPONSE"
    exit 1
fi

log_info "Droplet created with ID: $DROPLET_ID"

# Step 5: Wait for droplet to be ready
log_step "5/6 Waiting for droplet to be ready..."

for i in {1..60}; do
    DROPLET_INFO=$(do_api GET "/droplets/$DROPLET_ID")
    STATUS=$(echo "$DROPLET_INFO" | grep -o '"status":"[^"]*"' | head -1 | cut -d'"' -f4)
    IP=$(echo "$DROPLET_INFO" | grep -o '"ip_address":"[^"]*"' | head -1 | cut -d'"' -f4)

    if [ "$STATUS" = "active" ] && [ -n "$IP" ]; then
        log_info "Droplet is active!"
        break
    fi

    echo -n "."
    sleep 5
done
echo ""

if [ -z "$IP" ]; then
    log_error "Timed out waiting for droplet"
    exit 1
fi

log_info "Droplet IP: $IP"

# Step 6: Wait for cloud-init to complete
log_step "6/6 Waiting for AetherVault installation (this may take 3-5 minutes)..."

echo "You can check progress with: ssh root@$IP 'tail -f /var/log/cloud-init-output.log'"
echo ""

for i in {1..60}; do
    if ssh -o ConnectTimeout=5 -o StrictHostKeyChecking=no root@$IP "test -f /root/aethervault-setup-complete" 2>/dev/null; then
        log_info "AetherVault installation complete!"
        break
    fi
    echo -n "."
    sleep 10
done
echo ""

# Final output
echo ""
echo "=========================================="
echo "  AetherVault Deployment Complete!"
echo "=========================================="
echo ""
echo "Droplet IP: $IP"
echo "Droplet ID: $DROPLET_ID"
echo ""
echo "Next steps:"
echo ""
echo "1. SSH into your droplet:"
echo "   ssh root@$IP"
echo ""
echo "2. Switch to aethervault user and configure:"
echo "   su - aethervault"
echo "   nano ~/.aethervault/aethervault.json"
echo ""
echo "3. Add your API keys (e.g., ANTHROPIC_API_KEY, TELEGRAM_BOT_TOKEN)"
echo ""
echo "4. Start aethervault:"
echo "   sudo systemctl start aethervault"
echo "   sudo systemctl status aethervault"
echo ""
echo "5. View logs:"
echo "   sudo journalctl -u aethervault -f"
echo ""
echo "Gateway will be available at: http://$IP:18789"
echo ""
log_warn "Remember to rotate your DigitalOcean API token!"
