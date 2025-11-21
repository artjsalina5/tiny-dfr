#!/bin/bash
# install-tiny-dfr.sh
# Installation script for tiny-dfr on T2 MacBooks

set -e

SKIP_DEPS=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-deps)
            SKIP_DEPS=true
            shift
            ;;
        *)
            echo "Unknown option: $1" >&2
            echo "Supported: --skip-deps" >&2
            exit 1
            ;;
    esac
done

echo "Installing tiny-dfr for T2 MacBook..."

# Check if running on T2 Mac
if ! grep -q "MacBookPro16,1\|MacBookAir" /sys/class/dmi/id/product_name 2>/dev/null; then
    echo "Warning: This doesn't appear to be a T2 MacBook"
    read -p "Continue anyway? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Check if running as root
if [[ $EUID -eq 0 ]]; then
    echo "This script should not be run as root. Please run as a regular user."
    exit 1
fi

# Install dependencies based on distro (skip if --skip-deps)
if ! $SKIP_DEPS; then
    if command -v pacman &> /dev/null; then
        echo "Installing dependencies (Arch)..."
        sudo pacman -S --needed --noconfirm rust cargo cairo libinput freetype2 fontconfig librsvg
    elif command -v apt &> /dev/null; then
        echo "Installing dependencies (Debian/Ubuntu)..."
        sudo apt update
        sudo apt install -y build-essential rustc cargo libcairo2-dev libinput-dev libfreetype6-dev libfontconfig1-dev librsvg2-dev
    elif command -v dnf &> /dev/null; then
        echo "Installing dependencies (Fedora)..."
        sudo dnf install -y rust cargo cairo-devel libinput-devel freetype-devel fontconfig-devel librsvg2-devel
    else
        echo "Unsupported distribution. Install dependencies manually or use --skip-deps."
        echo "Required: rust, cargo, cairo, libinput, freetype, fontconfig, librsvg"
        exit 1
    fi
else
    echo "Skipping dependency installation (--skip-deps)"
fi

# Build from source
echo "Building tiny-dfr..."
cargo build --release

# Install
echo "Installing tiny-dfr..."
# Stop service if running to avoid "Text file busy" error
sudo systemctl stop tiny-dfr 2>/dev/null || true
sudo cp target/release/tiny-dfr /usr/bin/
sudo mkdir -p /usr/share/tiny-dfr
sudo cp share/tiny-dfr/* /usr/share/tiny-dfr/
sudo cp etc/systemd/system/tiny-dfr.service /etc/systemd/system/
sudo cp etc/systemd/system/tiny-dfr-post-resume.service /etc/systemd/system/
sudo install -Dm755 bin/tiny-dfr-terminal-exec /usr/bin/tiny-dfr-terminal-exec

# Install udev rules
sudo cp etc/udev/rules.d/99-touchbar-seat.rules /etc/udev/rules.d/
sudo cp etc/udev/rules.d/99-touchbar-tiny-dfr.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger

# Setup systemd service
sudo systemctl daemon-reload
sudo systemctl enable tiny-dfr
sudo systemctl enable tiny-dfr-post-resume.service

# Detect user environment for proper configuration
echo "Detecting user environment..."
CURRENT_USER=$(whoami)
USER_HOME="/home/$CURRENT_USER"
USER_UID=$(id -u $CURRENT_USER)
RUNTIME_DIR="/run/user/$USER_UID"

# Detect Wayland display
WAYLAND_DISPLAY_VALUE="wayland-1"  # default
if [ -d "$RUNTIME_DIR" ]; then
    for socket in "$RUNTIME_DIR"/wayland-*; do
        if [ -S "$socket" ] && [[ ! "$socket" == *.lock ]]; then
            WAYLAND_DISPLAY_VALUE=$(basename "$socket")
            break
        fi
    done
fi

# Detect user's actual PATH locations
USER_PATHS=""
for path_candidate in \
    "$USER_HOME/.local/share/omarchy/bin" \
    "$USER_HOME/.local/bin" \
    "$USER_HOME/.config/nvm/versions/node/latest/bin" \
    "$USER_HOME/.local/share/pnpm" \
    "$USER_HOME/.cargo/bin" \
    "$USER_HOME/.npm-global/bin" \
    "$USER_HOME/bin"; do
    if [ -d "$path_candidate" ]; then
        USER_PATHS="$USER_PATHS:$path_candidate"
    fi
done

echo "Detected user: $CURRENT_USER"
echo "Detected UID: $USER_UID"
echo "Detected Wayland display: $WAYLAND_DISPLAY_VALUE"
echo "Detected user paths: $USER_PATHS"

# Install configs (always update with backups)
sudo mkdir -p /etc/tiny-dfr
USER_CFG_DIR="$USER_HOME/.config/tiny-dfr"
mkdir -p "$USER_CFG_DIR"

TS=$(date +%Y%m%d%H%M%S)
for f in config.toml commands.toml expandables.toml hyprland.toml; do
    # System config
    if [ -f "/etc/tiny-dfr/$f" ]; then
        sudo cp "/etc/tiny-dfr/$f" "/etc/tiny-dfr/$f.bak.$TS"
    fi
    sudo cp "/usr/share/tiny-dfr/$f" "/etc/tiny-dfr/$f"
    
    # User config
    if [ -f "$USER_CFG_DIR/$f" ]; then
        cp "$USER_CFG_DIR/$f" "$USER_CFG_DIR/$f.bak.$TS"
    fi
    cp "/usr/share/tiny-dfr/$f" "$USER_CFG_DIR/$f"
done

# Ensure correct ownership
chown -R "$CURRENT_USER":"$CURRENT_USER" "$USER_HOME/.config/tiny-dfr"

# Create user-specific environment configuration
sudo tee /etc/tiny-dfr/user-env.toml > /dev/null <<EOF
# Auto-generated user environment configuration
[user_environment]
username = "$CURRENT_USER"
uid = $USER_UID
home_dir = "$USER_HOME"
runtime_dir = "$RUNTIME_DIR"
wayland_display = "$WAYLAND_DISPLAY_VALUE"
user_paths = "$USER_PATHS"
EOF

# Set MediaLayerDefault = true
echo "Setting MediaLayerDefault = true in config..."
sudo sed -i 's/MediaLayerDefault = false/MediaLayerDefault = true/' /etc/tiny-dfr/config.toml

# Restart the service to apply config changes
echo "Restarting tiny-dfr service..."
sudo systemctl restart tiny-dfr

echo ""
echo "✓ Installation complete!"
echo ""

