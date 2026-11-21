use super::hint;
use super::{Palette, heading, icon, percent, shortened};
use crate::{
    Frost, Message, Panel,
    services::Control,
    telemetry::{Network, Power},
};
use iced::{
    Alignment, Color, ContentFit, Element, Length, mouse,
    widget::{Space, button, container, image, mouse_area, responsive, row, scrollable, text},
    window::Id,
};

fn tinted(color: Color, strength: f32) -> Color {
    Color {
        a: strength,
        ..color
    }
}

fn panel_button(
    p: Palette,
    glyph: &'static str,
    color: Color,
    panel: Panel,
    active: bool,
) -> button::Button<'static, Message> {
    let foreground = if active { p.background } else { color };
    panel_button_content(icon(glyph, 17, foreground).into(), color, panel, active)
}

fn panel_button_content(
    content: Element<'static, Message>,
    color: Color,
    panel: Panel,
    active: bool,
) -> button::Button<'static, Message> {
    button(container(content).center_x(28).center_y(26))
        .padding(0)
        .style(move |_, status| button::Style {
            background: Some(
                if active {
                    color
                } else if status == button::Status::Hovered {
                    tinted(color, 0.27)
                } else {
                    tinted(color, 0.13)
                }
                .into(),
            ),
            border: iced::Border {
                radius: 6.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .on_press(Message::Toggle(panel))
}

fn indicator(
    glyph: &'static str,
    value: String,
    tone: Color,
    text_color: Color,
) -> Element<'static, Message> {
    let mut content = row![icon(glyph, 15, tone)]
        .spacing(5)
        .align_y(Alignment::Center);
    if !value.is_empty() {
        content = content.push(text(value).size(12).color(text_color));
    }
    content.into()
}

impl Frost {
    pub(crate) fn bar_view(&self, id: Id) -> Element<'_, Message> {
        responsive(move |size| self.bar_layout(id, size.width)).into()
    }

    fn bar_layout(&self, id: Id, width: f32) -> Element<'_, Message> {
        let p = self.palette;
        let show = |module| self.config.bar.shows(module);
        let active = |panel| self.panel.is_some_and(|(current, _)| current == panel);
        let panel = |glyph, color, panel| panel_button(p, glyph, color, panel, active(panel));
        let divider = || {
            container(Space::new())
                .width(1)
                .height(17)
                .style(move |_| container::Style::default().background(p.line))
        };
        let mut workspaces_row = row![].spacing(2).align_y(Alignment::Center);
        for workspace in self.snapshot.workspaces.iter().take(9) {
            let label = workspace
                .name
                .as_deref()
                .map(|name| shortened(name, 5))
                .unwrap_or_else(|| {
                    workspace
                        .idx
                        .map_or_else(|| workspace.id.to_string(), |idx| idx.to_string())
                });
            workspaces_row = workspaces_row.push(
                p.workspace(
                    label,
                    workspace.focused,
                    workspace.urgent,
                    workspace.occupied(self.compositor),
                )
                .on_press(Message::Switch(workspace.clone())),
            );
        }
        if let Some(snapshot) = &self.native_workspaces
            && let Some(group) = self
                .outputs
                .get(&id)
                .and_then(|output| snapshot.group_for_output(*output))
        {
            use iced_wayland_subscriber::workspace::{State, WorkspaceCapabilities};
            let counts = self.window_counts.as_ref();
            let mut workspaces: Vec<_> = snapshot
                .workspaces_in(&group.id)
                .filter(|workspace| !workspace.state.contains(State::Hidden))
                .collect();
            workspaces.sort_by(|a, b| a.coordinates.cmp(&b.coordinates));
            for (index, workspace) in workspaces.into_iter().take(9).enumerate() {
                let label = if workspace.name.is_empty() {
                    (index + 1).to_string()
                } else {
                    shortened(&workspace.name, 5)
                };
                let action = workspace
                    .capabilities
                    .contains(WorkspaceCapabilities::Activate)
                    .then(|| Message::NativeActivate(workspace.id.clone()));
                let occupied = counts
                    .and_then(|counts| workspace.protocol_id.as_ref().and_then(|id| counts.get(id)))
                    .is_none_or(|count| *count > 0);
                workspaces_row = workspaces_row.push(
                    p.workspace(
                        label,
                        workspace.state.contains(State::Active),
                        workspace.state.contains(State::Urgent),
                        occupied,
                    )
                    .on_press_maybe(action),
                );
            }
        }
        let workspaces = scrollable(workspaces_row)
            .direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::new().width(0).scroller_width(0),
            ))
            .width(Length::Fill)
            .height(Length::Shrink);
        let thresholds = |value: Option<u16>, warning, normal| match value {
            Some(v) if v >= 80 => p.danger,
            Some(v) if v >= warning => p.yellow,
            Some(_) => normal,
            None => p.muted,
        };
        let usage = |glyph, percent_used, tone| indicator(glyph, percent(percent_used), tone, tone);
        let cpu = usage("", self.cpu_usage, thresholds(self.cpu_usage, 50, p.green));
        let ram = usage(
            "󰍛",
            self.snapshot.memory,
            thresholds(self.snapshot.memory, 60, p.cyan),
        );
        let (network_glyph, network_label, network_tone) = match &self.network {
            Network::Offline => ("󰤭", "Offline".into(), p.muted),
            Network::Wired => ("󰈀", "Wired".into(), p.ice),
            Network::Wifi { ssid, signal } => {
                let glyph = match signal {
                    Some(0..=24) => "󰤟",
                    Some(25..=49) => "󰤢",
                    Some(50..=74) => "󰤥",
                    _ => "󰤨",
                };
                let ssid = if ssid.is_empty() {
                    "Wi-Fi".into()
                } else {
                    shortened(ssid, 15)
                };
                let value = match signal {
                    Some(signal) if width >= 815.0 => format!("{ssid} {signal}%"),
                    Some(signal) => format!("{signal}%"),
                    None => ssid,
                };
                (glyph, value, p.ice)
            }
            Network::Other(name) => ("󰈀", shortened(name, 12), p.ice),
        };
        let network = p
            .button(
                indicator(
                    network_glyph,
                    if width >= 530.0 {
                        network_label
                    } else {
                        String::new()
                    },
                    network_tone,
                    p.foreground,
                ),
                false,
            )
            .padding([4, 5])
            .on_press(Message::OpenNetwork);
        let (power_glyph, power_color) = match self.power.as_ref().map(|state| state.active) {
            Some(Power::Balanced) => ("󰗑", p.ice),
            Some(Power::Performance) => ("󰓅", p.danger),
            Some(Power::Saver) => ("󰌪", p.green),
            None => ("󰗑", p.muted),
        };
        let power_label = if width >= 755.0 {
            self.power
                .as_ref()
                .map(|state| state.active.label())
                .unwrap_or("—")
                .to_owned()
        } else {
            String::new()
        };
        let power_button = p
            .button(
                indicator(power_glyph, power_label, power_color, p.foreground),
                false,
            )
            .padding([4, 5])
            .on_press_maybe(
                (!self.power_pending && self.power.as_ref().is_some_and(|state| state.can_cycle()))
                    .then_some(Message::CyclePower),
            );
        let power = hint(
            power_button,
            self.power.as_ref().map_or_else(
                || "Power profile unavailable".to_owned(),
                |power| format!("{} · click to cycle", power.active.label()),
            ),
            p,
            iced::widget::tooltip::Position::Bottom,
        );
        let compact = width < 650.0;
        let mut status = row![]
            .spacing(if compact { 6 } else { 10 })
            .align_y(Alignment::Center);
        if show("cpu") {
            status = status.push(cpu);
        }
        if show("memory") {
            status = status.push(ram);
        }
        if show("network") {
            status = status.push(network);
        }
        if show("power") {
            status = status.push(power);
        }
        if show("volume")
            && width >= 900.0
            && let Some(level) = self.snapshot.volume
        {
            let (glyph, color) = if level.muted {
                ("󰝟", p.danger)
            } else {
                ("󰕾", p.purple)
            };
            status = status.push(
                mouse_area(
                    p.button(
                        indicator(glyph, format!("{}%", level.percent), color, p.foreground),
                        false,
                    )
                    .padding([4, 5])
                    .on_press(Message::Adjust(Control::VolumeMute)),
                )
                .on_scroll(|delta| {
                    let y = match delta {
                        mouse::ScrollDelta::Lines { y, .. }
                        | mouse::ScrollDelta::Pixels { y, .. } => y,
                    };
                    Message::Adjust(if y >= 0.0 {
                        Control::VolumeUp
                    } else {
                        Control::VolumeDown
                    })
                }),
            );
        }
        if show("battery")
            && let Some(battery) = self.snapshot.battery
        {
            let glyph = match battery.status {
                crate::services::BatteryStatus::Charging => "󰂄",
                _ => match battery.percent {
                    0..=19 => "󰁺",
                    20..=39 => "󰁼",
                    40..=59 => "󰁾",
                    60..=79 => "󰂀",
                    _ => "󰁹",
                },
            };
            let tone = if battery.percent < 20
                && battery.status == crate::services::BatteryStatus::Discharging
            {
                p.danger
            } else if battery.percent < 50
                && battery.status == crate::services::BatteryStatus::Discharging
            {
                p.yellow
            } else {
                p.green
            };
            status = status.push(
                p.button(
                    indicator(
                        glyph,
                        battery.label(self.battery_show_time),
                        tone,
                        p.foreground,
                    ),
                    false,
                )
                .padding([4, 5])
                .on_press_maybe(
                    battery
                        .remaining_minutes
                        .is_some()
                        .then_some(Message::ToggleBatteryTime),
                ),
            );
        }
        let clock = if width >= 970.0 {
            self.clock.format("%a %d %b  %H:%M").to_string()
        } else {
            self.clock.format("%H:%M").to_string()
        };
        let clock = heading(clock, 13).color(p.ice);
        let actions = row![
            panel("󰕮", p.ice, Panel::Dashboard),
            panel("󰋩", p.purple, Panel::Wallpaper),
            panel("󰐥", p.danger, Panel::Session),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        let crystal = image(crate::media::ice_icon())
            .width(24)
            .height(24)
            .content_fit(ContentFit::Contain);
        let mut bar = row![]
            .spacing(if compact { 5 } else { 9 })
            .align_y(Alignment::Center);
        if show("launcher") {
            bar = bar.push(panel_button_content(
                crystal.into(),
                p.ice,
                Panel::Launcher,
                active(Panel::Launcher),
            ));
        }
        if show("workspaces") {
            if show("launcher") {
                bar = bar.push(divider());
            }
            bar = bar.push(workspaces);
        } else {
            bar = bar.push(Space::new().width(Length::Fill));
        }
        bar = bar.push(status);
        if show("clock") {
            bar = bar.push(divider()).push(clock);
        }
        if show("actions") {
            bar = bar.push(actions);
        }
        container(bar)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(Alignment::Center)
            .padding([0, if compact { 6 } else { 9 }])
            .style(move |_| p.tile(p.background, 17.0))
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{media, services::Workspace};
    use iced::{Font, Size, mouse};
    use iced_runtime::user_interface::{Cache, UserInterface};
    use iced_tiny_skia::{
        Renderer,
        core::renderer,
        graphics::{Viewport, text::font_system},
        window::compositor,
    };

    #[test]
    fn renders_bar_controls_and_indicators_at_laptop_width() {
        font_system()
            .write()
            .unwrap()
            .load_font(media::SYMBOLS.into());
        let (mut frost, _tasks) = Frost::new(iced_wayland_subscriber::shell::channel().1);
        frost.cpu_usage = Some(54);
        frost.snapshot.memory = Some(68);
        frost.snapshot.battery = Some(crate::services::Battery {
            percent: 74,
            status: crate::services::BatteryStatus::Discharging,
            remaining_minutes: Some(127),
        });
        frost.snapshot.workspaces = (1..=9)
            .map(|id| Workspace {
                id,
                num: Some(id),
                name: None,
                idx: None,
                focused: id == 3,
                urgent: false,
                windows: Some(1),
                representation: Some("H[]".into()),
            })
            .collect();
        frost.network = Network::Wifi {
            ssid: "Frost WLAN".into(),
            signal: Some(72),
        };
        frost.power = crate::telemetry::tests::balanced_profile();
        let theme = frost.palette.theme();

        for width in [600_u32, 1040] {
            let height = frost.config.bar.height;
            let viewport = Viewport::with_physical_size(Size::new(width, height), 1.0);
            let mut renderer = iced::Renderer::Secondary(Renderer::new(Font::DEFAULT, 16.0.into()));
            let mut ui = UserInterface::build(
                frost.bar_view(Id::unique()),
                viewport.logical_size(),
                Cache::new(),
                &mut renderer,
            );
            ui.draw(
                &mut renderer,
                &theme,
                &renderer::Style {
                    text_color: theme.palette().text,
                },
                mouse::Cursor::Unavailable,
            );
            let pixels = match &mut renderer {
                iced::Renderer::Secondary(renderer) => {
                    compositor::screenshot(renderer, &viewport, Color::TRANSPARENT)
                }
                iced::Renderer::Primary(_) => unreachable!(),
            };
            // The far-right session button should remain visible even with all workspaces and indicators.
            let danger = frost.palette.danger.into_rgba8();
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .enumerate()
                    .any(|(index, rgba)| {
                        index % width as usize > width as usize - 48
                            && rgba[0].abs_diff(danger[0]) < 8
                            && rgba[1].abs_diff(danger[1]) < 8
                            && rgba[2].abs_diff(danger[2]) < 8
                    })
            );
        }
    }
}
