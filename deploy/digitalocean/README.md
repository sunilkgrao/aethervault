# Deploying AetherVault on DigitalOcean

This guide walks you through deploying [AetherVault](https://github.com/aethervault/aethervault) on a DigitalOcean droplet.

## Prerequisites

- A DigitalOcean account
- SSH key added to your DigitalOcean account
- API keys for the services you want to use (Anthropic, Telegram, Discord, etc.)

## Step 1: Create a DigitalOcean Droplet

### Recommended Specs

| Use Case | Droplet Size | Monthly Cost |
|----------|--------------|--------------|
| Light usage (1-2 channels) | Basic 1GB RAM / 1 vCPU | $6/month |
| Moderate usage (3-5 channels) | Basic 2GB RAM / 1 vCPU | $12/month |
| Heavy usage (all channels + voice) | Basic 4GB RAM / 2 vCPUs | $24/month |

### Create via DigitalOcean Console

1. Log in to [DigitalOcean](https://cloud.digitalocean.com/)
2. Click **Create** > **Droplets**
3. Choose settings:
   - **Region**: Choose closest to you for low latency
   - **Image**: Ubuntu 24.04 LTS
   - **Size**: See recommendations above
   - **Authentication**: SSH Key (recommended) or Password
   - **Hostname**: `aethervault` or your preferred name
4. Click **Create Droplet**

### Create via CLI (doctl)

```bash
# Install doctl if needed
brew install doctl  # macOS
# or: snap install doctl  # Linux

# Authenticate
doctl auth init

# Create droplet
doctl compute droplet create aethervault \
  --region nyc1 \
  --size s-1vcpu-2gb \
  --image ubuntu-24-04-x64 \
  --ssh-keys $(doctl compute ssh-key list --format ID --no-header | head -1) \
  --wait
```

## Step 2: Connect to Your Droplet

```bash
# Get your droplet's IP
doctl compute droplet list --format Name,PublicIPv4

# SSH into the droplet
ssh root@YOUR_DROPLET_IP
```

## Step 3: Upload and Run Setup Scripts

### Option A: Clone This Repository

```bash
# On your droplet
git clone https://github.com/YOUR_USERNAME/aethervault.git
cd aethervault/deploy/digitalocean
chmod +x *.sh
```

### Option B: Copy Scripts via SCP

```bash
# From your local machine
scp -r deploy/digitalocean root@YOUR_DROPLET_IP:/root/
ssh root@YOUR_DROPLET_IP
cd /root/digitalocean
chmod +x *.sh
```

## Step 4: Run the Setup Script

```bash
# Run as root
sudo ./setup-droplet.sh
```

This script will:
- Update system packages
- Install Node.js 22.x and pnpm
- Configure firewall (UFW)
- Set up fail2ban for security
- Create swap space
- Install Tailscale for secure remote access
- Apply system optimizations

## Step 5: Install AetherVault

```bash
# Switch to aethervault user (or your user)
su - aethervault  # or your username

# Run installation script
./install.sh
```

Choose option 1 (npm install) for the simplest setup.

## Step 6: Configure AetherVault

### Edit Configuration

```bash
nano ~/.aethervault/aethervault.json
```

### Set Up Environment Variables

```bash
# Copy template
cp config/.env.template ~/.aethervault/.env

# Edit with your API keys
nano ~/.aethervault/.env
```

### Example Configuration for Telegram

```json
{
  "agent": {
    "model": "anthropic/claude-sonnet-4"
  },
  "gateway": {
    "port": 18789
  },
  "channels": {
    "telegram": {
      "enabled": true
    }
  }
}
```

Then set your Telegram token:
```bash
export TELEGRAM_BOT_TOKEN="your_token_here"
```

## Step 7: Set Up Systemd Service

```bash
# Copy service file
sudo cp aethervault.service /etc/systemd/system/

# Edit to match your username if not 'aethervault'
sudo nano /etc/systemd/system/aethervault.service

# Reload systemd and enable service
sudo systemctl daemon-reload
sudo systemctl enable aethervault
sudo systemctl start aethervault

# Check status
sudo systemctl status aethervault
```

## Step 8: Set Up Secure Remote Access (Tailscale)

Tailscale provides secure remote access without exposing ports to the internet.

```bash
# Start Tailscale
sudo tailscale up

# Follow the authentication link

# Optional: Enable Tailscale Funnel for public HTTPS access
sudo tailscale funnel 18789
```

## Useful Commands

### Service Management

```bash
# Start/stop/restart service
sudo systemctl start aethervault
sudo systemctl stop aethervault
sudo systemctl restart aethervault

# View logs
sudo journalctl -u aethervault -f

# Check status
sudo systemctl status aethervault
```

### AetherVault CLI Commands

```bash
# Start gateway manually
aethervault gateway --port 18789 --verbose

# Send a test message
aethervault message send --to YOUR_PHONE --message "Hello from AetherVault!"

# Run diagnostics
aethervault doctor

# Update aethervault
aethervault update --channel stable
```

### Firewall

```bash
# Check firewall status
sudo ufw status

# Allow additional port
sudo ufw allow PORT/tcp
```

## Troubleshooting

### Gateway won't start

```bash
# Check logs
sudo journalctl -u aethervault -n 50

# Run manually to see errors
aethervault gateway --verbose
```

### Permission errors

```bash
# Fix ownership
sudo chown -R aethervault:aethervault ~/.aethervault ~/aethervault-workspace
```

### Node.js version issues

```bash
# Check version
node -v

# Should be 22.x or higher
# If not, reinstall:
curl -fsSL https://deb.nodesource.com/setup_22.x | sudo -E bash -
sudo apt-get install -y nodejs
```

### Can't connect remotely

1. Check firewall: `sudo ufw status`
2. Check if gateway is running: `sudo systemctl status aethervault`
3. Use Tailscale for secure access instead of opening ports

## Security Best Practices

1. **Use Tailscale** instead of exposing ports directly to the internet
2. **Keep system updated**: `sudo apt update && sudo apt upgrade`
3. **Monitor logs**: `sudo journalctl -u aethervault -f`
4. **Use strong API tokens** and rotate them periodically
5. **Enable DM pairing mode** (default) to control who can message your bot
6. **Regular backups** of `~/.aethervault` directory

## Backup and Restore

### Backup

```bash
# Backup configuration and credentials
tar -czvf aethervault-backup-$(date +%Y%m%d).tar.gz ~/.aethervault
```

### Restore

```bash
# Restore from backup
tar -xzvf aethervault-backup-YYYYMMDD.tar.gz -C ~/
```

## Updating AetherVault

```bash
# Update to latest stable
aethervault update --channel stable

# Restart service after update
sudo systemctl restart aethervault
```

## Cost Optimization

- Use the smallest droplet that meets your needs
- Consider reserved droplets for long-term savings
- Monitor resource usage with `htop`
- Scale up only when needed

## Support

- [AetherVault GitHub](https://github.com/aethervault/aethervault)
- [AetherVault Documentation](https://github.com/aethervault/aethervault#readme)
- [DigitalOcean Support](https://www.digitalocean.com/support/)
