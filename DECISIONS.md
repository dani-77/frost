# Integration decisions

## Palette and wallpaper

- Frost reads `~/.cache/d77/palette.json` and `~/.config/d77/theme-mix`, shared with quickshell-d77, Utumno, and fabric-d77. It ignores a palette whose source does not match the active wallpaper.
- When a wallpaper is selected, Frost applies it with `awww` or `swww`, stores the choice in `~/.cache/frost/wallpaper/current`, and generates the palette with Matugen. The Matugen template is embedded in the binary so `cargo install` remains sufficient.
- Wallpaper jobs run one at a time; rapid clicks replace the pending target, and the final target runs after the current job. The picker stays open and highlights the target. The wallpaper state is replaced atomically, and Matugen writes to a Frost-owned temporary file before publishing `d77/palette.json`.
- An empty Frost state file means the user cleared the wallpaper. Without its own state, Frost can also recognize wallpaper choices from the other shells.

## Backdrop and display scaling

- Each output gets a `Layer::Bottom` surface only while no wallpaper is active. It uses `LayerSize::FILL`, all four anchors, and exclusive zone `-1` so it also covers the area reserved by the bar. The compositor assigns its logical size; Frost does not divide physical resolutions by the scale factor.
- The shell uses the WGPU renderer for alpha compositing around its rounded surfaces. Tiny Skia remains available for headless rendering tests and the locker.
- The surface passes through pointer events and sits above wallpaper daemons on the Background layer.
- The backdrop shows the owl emblem in the lower left corner at low opacity. Rendering is checked at scale factors 1 and 2.
- The crystal marks the launcher in the bar, the owl appears on the lock screen and backdrop, and the terminal emblem introduces the README.

## Workspaces

- Spitfire, Wasp, and Whiplash announce `Active` and `Urgent` through `ext-workspace-v1`. Occupancy comes from `$XDG_RUNTIME_DIR/d77-workspaces-$WAYLAND_DISPLAY`, keyed by protocol ID. Without the counts file, workspaces are not marked empty.
- The bar accepts a sole `ext-workspace-v1` group with no output assignment. Spitfire exposes one such group; output-scoped groups remain preferred for Wasp and Whiplash.
- Sway uses `representation` for occupancy and `urgent` for urgency. Hyprland uses `windows`; niri associates each window with its workspace ID.
- Niri sends both `is_active` and `is_focused` for each workspace. The bar maps `is_active` to the highlighted state and ignores `is_focused` during deserialization so both fields can coexist.
- Hyprland logout tries the Lua dispatcher `hl.dsp.exit()` used by the user's configuration, then the classic `exit` dispatcher for older configurations.

## Polling and events

- The one-second tick reads only local files: clock, CPU, memory, load, battery, and backlight (from `/sys/class/backlight`, choosing the device by the kernel's preference of `firmware`, `platform`, then `raw`, which Frost also passes to `brightnessctl -d`). Music is read only while the dashboard is open.
- Workspaces refresh on compositor events: `swaymsg -t subscribe`, Hyprland's `.socket2.sock`, and `niri msg event-stream`. Volume refreshes on `pactl subscribe` sink and server events. Bursts within 40 ms produce one read.
- When an event source is missing or exits, Frost polls it every second and tries the source again after 30 seconds, so it never does worse than plain polling.
- A volume or brightness read that started before Frost's own last change, or while a change is in flight, is discarded so it cannot revert the slider or show a stale OSD.
- Failed volume reads share one pending retry timer. The fallback polling can fail repeatedly while audio is absent, so each failure must not schedule another timer.

## Dashboard and terminal

- The dashboard follows the other shells' 8–12 px spacing and includes weather updated every 15 minutes, cmus controls, and session actions with confirmation for exit actions.
- The dashboard fits all sections in a 420 × 520 logical pixel panel without a scroll area. The volume and brightness sliders replace redundant plus/minus rows.
- Volume and brightness sliders apply the latest requested value in sequence, so a rapid drag cannot leave an older value applied last.
- The launcher honors `Terminal=true` desktop entries. It and the `nmtui` button resolve a terminal in the same order: Frost config, `TERMINAL`, `xdg-terminal-exec`, then installed terminals.
- A `/` query switches the launcher to local file search. The background index is bounded to five directory levels and 20,000 non-hidden files, and file opening uses `xdg-open`.
- Clicking CPU, memory, or network in the dashboard opens an in-place detail overlay. Process and interface commands run only when requested.
- CPU and memory in the bar call the same portable `d77-monitor` script. It toggles a centered Eww surface and selects the clicked metric; the dashboard details remain independent.
- The Eww configuration uses dgop's JSON CLI for CPU, memory, and sorted processes, with no HTTP server. It reads the shared Matugen palette every 10 seconds. The script and configuration can be copied together to other shells; each shell only supplies its own click binding.
- `~/.config/frost/config.toml` can set bar height and margin, visible bar modules, terminal executable, and weather location. Defaults preserve the original layout, and the configured terminal takes priority over `TERMINAL`.

## Clipboard

- `frost clipboard` opens a separate panel with no bar module. It reads the cliphist database only when opened, searches previews and MIME types, and restores selected data through `wl-copy`.
- The panel shows image thumbnails for visible matches, limits background decoding, and asks before `cliphist wipe`. It does not start another history daemon; the compositor runs the text and image `wl-paste --watch cliphist store` jobs once per session.

## Notifications

- The clock opens notifications through the daemon already running. Dunst gets a Frost panel backed by `dunstctl history` with recall and do not disturb controls; SwayNotificationCenter keeps its own control center through `swaync-client -t`.
- The Dunst history clear action requires confirmation. Frost needs one of these daemons running and does not register as a notification daemon itself.

## Installation

- The Makefile builds a locked release binary as the regular user, then installs the binary under `$(PREFIX)/bin`. `DESTDIR` supports package staging.
- The locker requires a root-owned policy at `/etc/pam.d/frost-locker`. The Makefile selects `void`, `arch`, `debian`, or `chimera` from `/etc/os-release`, including hybrid-d77 through `ID_LIKE`; `PAM_VARIANT` overrides the detected value. Chimera follows hybrid-d77's Utumno policy with `pam_unix.so` and adds an account rule because Frost calls `pam_acct_mgmt`. The example config and license texts are installed under `$(PREFIX)/share`.
- Runtime art, the Matugen template, and the icon font are embedded in the binary, so the system-wide install does not need a separate assets directory.

## Development workflow

- Built-in hot reload was considered and abandoned. It adds runtime complexity without a useful role in the finished shell. Developers who want automatic rebuilds and restarts can configure external tooling themselves.
