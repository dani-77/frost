# Frost

<p align="center"><img src="assets/terminal.png" alt="Frost terminal emblem" width="220"></p>

Frost is a Wayland shell in Rust inspired by [quickshell-d77](https://github.com/dani-77/quickshell-d77/tree/5b5a1b5560a676339d31e6361650b6a5e8899a37). Its panels use layer shell through `iced_exwlshell`; the locker uses Wayland session lock surfaces through Smithay Client Toolkit. Both are drawn with Iced 0.14 and Tiny Skia.

| Component | Current development branch |
| --- | --- |
| Bar | A floating bar per output with CPU, RAM, network, power profile, battery, clock, and workspaces. Click the battery to switch between percentage and estimated time; systems without a built-in battery omit it. Sway, Hyprland, and niri use their IPC; other compositors can use `ext-workspace-v1`. |
| Launcher | Searches XDG `.desktop` entries with application icons. Arrow keys select, Enter launches, and Esc closes. `Terminal=true` uses the configured terminal. |
| Clipboard | Searches cliphist history, previews images, restores an item to the clipboard, and clears the history after confirmation. Opens with `frost clipboard`; no bar module is needed. |
| Dashboard | CPU, memory, load, battery, weather, volume and brightness sliders, cmus controls, and session actions in a single view without scrolling. |
| OSD | Shows volume and brightness changes. Media keys can call Frost directly. |
| Wallpaper | Previews images in `~/Wallpaper`, applies them through `awww` or `swww`, generates a Matugen palette, and shows a backdrop when no wallpaper is active. |
| Session | Native locker, suspend, logout, reboot, and power off. The last three require confirmation. |

## Build and run

Use a Wayland session with `zwlr_layer_shell_v1`. Building requires Rust and Cargo 1.95 or newer, a C compiler, `pkg-config`, and development files for xkbcommon and Linux PAM.

On Void Linux, install the build dependencies, `gio` for the launcher, and the clipboard tools:

```sh
sudo xbps-install -S rust cargo base-devel pkg-config libxkbcommon-devel pam-devel glib cliphist wl-clipboard xdg-utils
```

On Arch Linux, install the matching build tools and clipboard tools before building from GitHub:

```sh
sudo pacman -S --needed base-devel rust cargo pkgconf libxkbcommon pam glib2 cliphist wl-clipboard xdg-utils
```

From the project root:

```sh
cargo run --locked --release -- run
```

To install the command in `~/.cargo/bin`:

```sh
cargo install --locked --path .
~/.cargo/bin/frost run
```

Add `~/.cargo/bin` to `PATH` if you want to call it simply as `frost`.

For a system-wide install, build as your regular user and then install as root:

```sh
make
sudo make install
```

This installs `/usr/bin/frost`, a PAM policy at `/etc/pam.d/frost-locker`, the example configuration at `/usr/share/frost/config.example.toml`, and the licenses under `/usr/share/licenses/frost`. The Makefile detects Void, Arch, Debian derivatives, and Chimera (including hybrid-d77 through `ID_LIKE`) from `/etc/os-release`. Override the choice when staging for another system, for example `make install DESTDIR=/tmp/frost-package PAM_VARIANT=chimera`. The Chimera policy follows hybrid-d77's local `pam_unix.so` authentication and adds the account rule required by Frost. `PREFIX` defaults to `/usr`; `DESTDIR` stages an install for packaging. Run `sudo make uninstall` to remove the installed files. If `~/.cargo/bin` precedes `/usr/bin` in `PATH`, `frost` still resolves to the user-installed copy; check with `command -v frost`.

Versioned packaging recipes are in [`packaging/arch/PKGBUILD`](packaging/arch/PKGBUILD) and [`packaging/void/template`](packaging/void/template). Both use the `v0.1.0` source tag and install the distribution-specific PAM policy.

| Integration | Runtime dependency |
| --- | --- |
| Launcher | `gio` |
| Clipboard | `cliphist` for history, `wl-clipboard` for capture and restoration, and `xdg-utils` for MIME inference with older cliphist versions |
| Wi-Fi | `nmcli` or `iwgetid` for the SSID; route and state come from Linux |
| Power profile | `powerprofilesctl` and a compatible Power Profiles API |
| Volume | `wpctl` with PipeWire/WirePlumber; `pactl` (optional) lets Frost react to changes instead of polling every second |
| Brightness | `brightnessctl` and a user-accessible backlight device; the level is read from `/sys/class/backlight` |
| Music | `cmus-remote`; `tmux` or `screen` to start cmus from the dashboard |
| Wallpaper | `awww` or `swww` with its daemon running |
| Dynamic palette | `matugen` (optional) |
| Weather | `curl` and access to `wttr.in` (optional) |
| Network menu | `nmtui` and a terminal; the configured terminal, `TERMINAL`, or `xdg-terminal-exec` is preferred |
| Lock | `ext-session-lock-v1` and the `frost-locker` PAM policy |
| Suspend, reboot, power off | `dbus-send` and logind/elogind |

## Shortcuts

Panel commands contact the Frost instance on the current Wayland display. `frost lock` starts the locker as a separate process.

```text
# ~/.config/sway/config
exec frost run
bindsym $mod+d exec frost launcher
bindsym $mod+v exec frost clipboard
bindsym $mod+i exec frost dashboard
bindsym $mod+y exec frost wallpaper
bindsym $mod+Shift+e exec frost session
bindsym $mod+l exec frost lock
bindsym XF86AudioRaiseVolume exec frost volume-up
bindsym XF86AudioLowerVolume exec frost volume-down
bindsym XF86AudioMute exec frost volume-mute
bindsym XF86MonBrightnessUp exec frost brightness-up
bindsym XF86MonBrightnessDown exec frost brightness-down
```

The same commands work with other compositors. `frost close` closes the open panel. Run `frost --help` for the complete command list.

To collect text and images, start these two watchers once per Wayland session in your compositor's autostart:

```sh
wl-paste --type text --watch cliphist store
wl-paste --type image --watch cliphist store
```

If your session already runs cliphist watchers, keep its existing setup. Frost only reads the history when the panel opens. Search with the input field; click an item or press Enter to put it back on the clipboard. The **Clear history** action asks for confirmation. Image thumbnails are loaded in the background, and the panel shows up to 80 matching entries at once.

## Locker

If you installed Frost with Cargo, install the PAM policy separately on Void Linux:

```sh
sudo install -o root -g root -m 0644 pam.d/frost-locker.void /etc/pam.d/frost-locker
~/.cargo/bin/frost lock
```

On Arch Linux, Debian derivatives, or Chimera Linux, use the matching file in `pam.d/` at the same destination. `make install PAM_VARIANT=...` handles the policy automatically. Void and Arch include the distribution's `system-auth` stack, Debian includes `common-auth` and `common-account`, and Chimera uses `pam_unix.so` for local users. The locker refuses to start if the PAM file is missing, is owned by a non-root user, or is writable by its group or others. PAM can request one interactive password prompt per attempt. Install and test the policy in a nested compositor before using it in your main session.

The process waits for the compositor's `locked` confirmation before accepting a password. The keyboard is connected before the lock request. An opaque surface covers every output, including outputs added while locked. The Iced view receives only the character count and authentication state. The password buffer is zeroed in memory and protected from swap with `mlock` when possible. Enter submits, Backspace deletes the last character, and Esc clears the field.

The PAM worker verifies both the password and account state. Only after both checks does Frost send `unlock_and_destroy`, followed by a Wayland round trip before exit. Compositor refusal or rendering failure follows the error path. If the process dies after the compositor confirms the lock, the session stays locked.

The Wayland backend was adapted from [helium-locker](https://github.com/dani-77/helium-d77/tree/afcbfa4dc09e98a6b3cc7a4829cc934461b5ce2c). The state machine, PAM worker, and password handling come from [Amorphis](https://github.com/dani-77/amorphis/tree/175c5a908727ca0bcc88a78b8da3174bc5efe805), credited in `licenses/Amorphis-MIT.txt`.

## Bar and icons

By default, the bar sits 10 logical pixels from the top and sides and is 40 logical pixels high. Click the power profile icon to cycle through daemon-advertised profiles. Scroll on the volume widget to adjust it; click to mute or unmute. Click the network widget to open `nmtui`. On narrow displays, indicators abbreviate and workspaces scroll horizontally. Hover over the power profile and cmus buttons for action hints.

On Spitfire, Wasp, and Whiplash, the bar reads urgent state from `ext-workspace-v1` and occupancy from `$XDG_RUNTIME_DIR/d77-workspaces-$WAYLAND_DISPLAY`. Sway uses `urgent` and `representation`; Hyprland uses window counts; niri queries windows by workspace.

The bar icons follow [quickshell-d77](https://github.com/dani-77/quickshell-d77) and [fabric-d77](https://github.com/dani-77/fabric-d77). `assets/FrostSymbols.ttf` embeds [Nerd Fonts Symbols Only](https://github.com/ryanoasis/nerd-fonts/tree/v3.5.0/patched-fonts/NerdFontsSymbolsOnly), including the session and cmus glyphs. Licenses are in `licenses/`.

The launcher resolves `Icon=` through the configured GTK theme and then `hicolor`. `FROST_ICON_THEME` can select another installed theme. Wallpaper thumbnails are generated in the background when the panel opens.

## Wallpaper and color

`FROST_WALLPAPERS` sets the picker directory and should be an absolute path. On selection, Frost stores the choice in `${XDG_CACHE_HOME:-$HOME/.cache}/frost/wallpaper/current` and, if Matugen is installed, updates the shared `d77/palette.json` under the same cache root. The picker stays open and marks the selected image. Rapid selections are applied in sequence, with the final selection applied last. Palette strength uses `~/.config/d77/theme-mix`, shared with the other shells, and can be changed in the picker. With no selected image, Frost shows a backdrop per output on the Bottom layer, with the owl emblem in the lower left corner. The compositor sizes it in logical coordinates, including at scales other than 1.

```sh
FROST_WALLPAPERS="$HOME/Pictures/Wallpapers" frost run
```

## Configuration

Copy [config.example.toml](config.example.toml) to `~/.config/frost/config.toml` and restart Frost. The `[bar]` section sets `height`, `margin`, and the visible `modules`. Module names are `launcher`, `workspaces`, `cpu`, `memory`, `network`, `power`, `volume`, `battery`, `clock`, and `actions`. The top-level `terminal` sets the executable used by `Terminal=true` applications and `nmtui`; `TERMINAL` and automatic detection remain fallbacks. `[weather].location` selects the location sent to wttr.in; leave it unset for automatic location.

## Current scope

Panels and the OSD use the Overlay layer. Frost shows up to nine workspaces. IPC integrations expose a global workspace list; the Wayland protocol integration filters by output.

The picker uses an external wallpaper daemon. Greeter, tray, music picker, and Ollama are outside 0.1.

## Code

- `src/main.rs`: Iced loop, surfaces, and state.
- `src/config.rs`: TOML configuration and defaults.
- `Makefile`: release build and system-wide install.
- `src/ui.rs`: widgets, colors, and views.
- `src/services.rs`: XDG applications, compositor IPC, and system services.
- `src/ipc.rs`: local session socket and single instance.
- `src/lock/`: Wayland locker, Iced view, PAM, and authentication state.
- `DECISIONS.md`: integration choices and data contracts.
