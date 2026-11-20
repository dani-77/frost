# Integration decisions

## Palette and wallpaper

- Frost reads `~/.cache/d77/palette.json` and `~/.config/d77/theme-mix`, shared with quickshell-d77, Utumno, and fabric-d77. It ignores a palette whose source does not match the active wallpaper.
- When a wallpaper is selected, Frost applies it with `awww` or `swww`, stores the choice in `~/.cache/frost/wallpaper/current`, and generates the palette with Matugen. The Matugen template is embedded in the binary so `cargo install` remains sufficient.
- An empty Frost state file means the user cleared the wallpaper. Without its own state, Frost can also recognize wallpaper choices from the other shells.

## Backdrop and display scaling

- Each output gets a `Layer::Bottom` surface only while no wallpaper is active. It uses `LayerSize::FILL` and all four anchors. The compositor assigns its logical size; Frost does not divide physical resolutions by the scale factor.
- The surface passes through pointer events and sits above wallpaper daemons on the Background layer.
- The backdrop shows the owl emblem in the lower left corner at low opacity. Rendering is checked at scale factors 1 and 2.
- The crystal marks the launcher in the bar, the owl appears on the lock screen and backdrop, and the terminal emblem introduces the README.

## Workspaces

- Spitfire, Wasp, and Whiplash announce `Active` and `Urgent` through `ext-workspace-v1`. Occupancy comes from `$XDG_RUNTIME_DIR/d77-workspaces-$WAYLAND_DISPLAY`, keyed by protocol ID. Without the counts file, workspaces are not marked empty.
- Sway uses `representation` for occupancy and `urgent` for urgency. Hyprland uses `windows`; niri associates each window with its workspace ID.

## Dashboard and terminal

- The dashboard follows the other shells' 8–12 px spacing and includes weather updated every 15 minutes, cmus controls, and session actions with confirmation for exit actions.
- The dashboard fits all sections in a 420 × 520 logical pixel panel without a scroll area. The volume and brightness sliders replace redundant plus/minus rows.
- Volume and brightness sliders apply the latest requested value in sequence, so a rapid drag cannot leave an older value applied last.
- The launcher honors `Terminal=true` desktop entries. It and the `nmtui` button resolve a terminal in the same order: `TERMINAL`, `xdg-terminal-exec`, then installed terminals.
