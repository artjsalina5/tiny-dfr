# tiny-dfr (Omarchy Fork)

**v0.4.0**

Touch Bar daemon for Apple T2 and Silicon Macs. Fork of [tiny-dfr](https://github.com/AsahiLinux/tiny-dfr) with working suspend/resume, Hyprland integration, and better configuration.

## Features

- Working suspend/resume on T2 MacBooks
- Touch input persists after wake
- Hyprland window context support
- Multi-level expandable menus
- Keyboard backlight control
- Easy configuration with examples
- Omarchy ecosystem integration

## Requirements (T2 Macs)

Kernel modules: `apple-bce`, `hid-appletb-kbd`, `hid-appletb-bl`, `appletbdrm`

Works with `suspend-fix-t2.service` for proper driver coordination on wake.

## Installation

```bash
./install-tiny-dfr.sh
```

Installs dependencies, builds from source, and sets up systemd services.

## Configuration

Config files load in priority order:
1. `/usr/share/tiny-dfr/` (defaults)
2. `/etc/tiny-dfr/` (system overrides)
3. `~/.config/tiny-dfr/` (user overrides)

Copy files from `share/tiny-dfr/` to customize.

### config.toml

Display settings, brightness, fonts. See [share/tiny-dfr/config.toml](share/tiny-dfr/config.toml).

### commands.toml

Custom commands as `Command_Name = "your-command"`. Terminal apps need wrapper: `alacritty -e btop`. See [share/tiny-dfr/commands.toml](share/tiny-dfr/commands.toml).

### expandables.toml

Multi-level menus. See [share/tiny-dfr/expandables.toml](share/tiny-dfr/expandables.toml).

### hyprland.toml

Per-app button layouts. See [share/tiny-dfr/hyprland.toml](share/tiny-dfr/hyprland.toml).

## Omarchy Integration

Ships with Omarchy defaults: menus, screenshots, screen recording. Customize via `Expand_Omarchy`.

Helper script `tiny-dfr-terminal-exec` finds your terminal automatically (or set `$TERMINAL`).
