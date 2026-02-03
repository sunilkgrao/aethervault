# Vault Installer for Windows
# v1 - System package managers only, install-if-missing

$ErrorActionPreference = "Stop"

# Colors for output
function Write-Success {
    Write-Host "✔ $args" -ForegroundColor Green
}

function Write-Error {
    Write-Host "✖ $args" -ForegroundColor Red
}

function Write-Info {
    Write-Host "→ $args" -ForegroundColor Cyan
}

function Write-Warning {
    Write-Host "⚠ $args" -ForegroundColor Yellow
}

# Check if command exists
function Test-Command {
    param([string]$Command)
    $null -ne (Get-Command $Command -ErrorAction SilentlyContinue)
}

# Check for winget
function Test-Winget {
    if (Test-Command winget) {
        $wingetVersion = (winget --version)
        Write-Success "winget already installed (version $wingetVersion)"
        return $true
    } else {
        Write-Error "winget not found"
        Write-Info "winget is required for v1 installer"
        Write-Info "Please install winget first:"
        Write-Info "  https://aka.ms/getwinget"
        Write-Info ""
        Write-Info "Or install git and node manually, then run:"
        Write-Info "  npm install -g vault-cli@latest"
        exit 1
    }
}

# Check for git
function Test-Git {
    if (Test-Command git) {
        $gitVersion = (git --version)
        Write-Success "git already installed ($gitVersion)"
        return $true
    } else {
        Write-Error "git not found"
        return $false
    }
}

# Check for node
function Test-Node {
    if (Test-Command node) {
        $nodeVersion = (node --version)
        Write-Success "node already installed ($nodeVersion)"
        
        if (Test-Command npm) {
            $npmVersion = (npm --version)
            Write-Success "npm already installed (version $npmVersion)"
            return $true
        } else {
            Write-Error "npm not found (should come with node)"
            return $false
        }
    } else {
        Write-Error "node not found"
        return $false
    }
}

# Install git
function Install-Git {
    Write-Info "Installing git using winget..."
    
    try {
        winget install --id Git.Git -e --silent --accept-package-agreements --accept-source-agreements
        # Refresh PATH
        $env:Path = [System.Environment]::GetEnvironmentVariable("Path","Machine") + ";" + [System.Environment]::GetEnvironmentVariable("Path","User")
        
        if (Test-Command git) {
            Write-Success "git installed successfully"
        } else {
            Write-Error "git installation completed but not found in PATH"
            Write-Info "Please restart your terminal and run the installer again"
            exit 1
        }
    } catch {
        Write-Error "git installation failed: $_"
        exit 1
    }
}

# Install node (LTS)
function Install-Node {
    Write-Info "Installing node (LTS) using winget..."
    
    try {
        winget install --id OpenJS.NodeJS.LTS -e --silent --accept-package-agreements --accept-source-agreements
        # Refresh PATH
        $env:Path = [System.Environment]::GetEnvironmentVariable("Path","Machine") + ";" + [System.Environment]::GetEnvironmentVariable("Path","User")
        
        if (Test-Command node) {
            $nodeVersion = (node --version)
            $npmVersion = (npm --version)
            Write-Success "node installed successfully ($nodeVersion)"
            Write-Success "npm installed successfully (version $npmVersion)"
        } else {
            Write-Error "node installation completed but not found in PATH"
            Write-Info "Please restart your terminal and run the installer again"
            exit 1
        }
    } catch {
        Write-Error "node installation failed: $_"
        exit 1
    }
}

# Check if vault is already installed
function Test-Vault {
    if (Test-Command vault) {
        try {
            $vaultVersion = vault --version 2>$null
            Write-Success "vault already installed ($vaultVersion)"
            return $true
        } catch {
            Write-Success "vault already installed"
            return $true
        }
    } else {
        Write-Error "vault not found"
        return $false
    }
}

# Install missing tools
function Install-Missing {
    $needsGit = -not (Test-Git)
    $needsNode = -not (Test-Node)
    $needsVault = -not (Test-Vault)
    
    if (-not $needsGit -and -not $needsNode -and -not $needsVault) {
        Write-Info "All dependencies are already installed"
        return
    }
    
    # Show what will be installed
    Write-Host ""
    Write-Warning "The following tools will be installed:"
    if ($needsGit) { Write-Host "  - git" }
    if ($needsNode) { Write-Host "  - node (LTS)" }
    if ($needsVault) { Write-Host "  - vault-cli (latest)" }
    Write-Host ""
    
    # Ask for confirmation
    $response = Read-Host "Continue? [Y/n]"
    if ($response -ne "" -and $response -notmatch "^[Yy]$") {
        Write-Info "Installation cancelled"
        exit 0
    }
    
    # Install missing tools
    if ($needsGit) { Install-Git }
    if ($needsNode) { Install-Node }
    if ($needsVault) { Install-Vault }
}

# Install vault
function Install-Vault {
    Write-Info "Installing vault globally..."
    
    try {
        npm install -g vault-cli@latest
        Write-Success "vault installed successfully"
    } catch {
        Write-Error "vault installation failed: $_"
        exit 1
    }
}

# Verify installation
function Verify-Installation {
    Write-Info "Verifying installation..."
    
    if (Test-Command vault) {
        try {
            $vaultVersion = vault --version 2>$null
            Write-Success "vault is installed and accessible"
            Write-Info "Version: $vaultVersion"
            Write-Host ""
            Write-Success "Installation complete! You can now use 'vault' command."
        } catch {
            Write-Success "vault is installed and accessible"
            Write-Host ""
            Write-Success "Installation complete! You can now use 'vault' command."
        }
    } else {
        Write-Error "vault verification failed"
        Write-Info "The installation may have completed, but 'vault' command is not in PATH"
        Write-Info "Please check your npm global bin directory and add it to PATH if needed"
        Write-Info "Or try: npm list -g vault-cli"
        exit 1
    }
}

# Main execution
function Main {
    Write-Host "Vault Installer"
    Write-Host "Checking system requirements…"
    Write-Host ""
    
    Test-Winget
    Write-Host ""
    
    Install-Missing
    Write-Host ""
    
    Verify-Installation
}

# Run main function
Main
