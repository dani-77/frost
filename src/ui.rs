mod bar;

use crate::{
    Frost, LAUNCHER_INPUT, Message, Notice, Panel,
    services::{Control, Level, Session},
};
use iced::{
    Alignment, Background, Border, Color, ContentFit, Element, Font, Length, Theme,
    font::Weight,
    theme::Palette as IcedPalette,
    widget::{
        Space, button, column, container, image, progress_bar, row, scrollable, slider, stack, svg,
        text, text_input, tooltip,
    },
};
use std::{env, fs, path::PathBuf};

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub(crate) background: Color,
    surface: Color,
    elevated: Color,
    line: Color,
    foreground: Color,
    muted: Color,
    ice: Color,
    on_ice: Color,
    danger: Color,
    green: Color,
    yellow: Color,
    cyan: Color,
    purple: Color,
}

impl Palette {
    pub fn load() -> Self {
        Self::resolve(
            crate::services::active_wallpaper().as_deref(),
            crate::services::theme_mix(),
        )
    }

    /// Builds the palette for an already known wallpaper and theme mix.
    pub fn resolve(wallpaper: Option<&std::path::Path>, mix: f32) -> Self {
        let mut palette = Self {
            background: rgb(0x101822),
            surface: rgb(0x172331),
            elevated: rgb(0x223246),
            line: rgb(0x32465a),
            foreground: rgb(0xe4effa),
            muted: rgb(0xa1b3c8),
            ice: rgb(0x8fcef4),
            on_ice: rgb(0x0e2539),
            danger: rgb(0xffa8af),
            green: rgb(0x9edbbd),
            yellow: rgb(0xf3cf9b),
            cyan: rgb(0x8fdfde),
            purple: rgb(0xc9b9f5),
        };
        let path = env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")));
        let data = path
            .and_then(|path| fs::read_to_string(path.join("d77/palette.json")).ok())
            .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok());
        if let Some(data) = data
            && data.get("source").and_then(|value| value.as_str())
                == wallpaper.and_then(|path| path.to_str())
        {
            let color = |key: &str, fallback: Color| {
                data.get(key)
                    .and_then(|value| value.as_str())
                    .filter(|hex| hex.len() == 7 && hex.starts_with('#'))
                    .and_then(|hex| u32::from_str_radix(&hex[1..], 16).ok())
                    .map_or(fallback, |value| {
                        let target = rgb(value);
                        Color {
                            r: fallback.r + (target.r - fallback.r) * mix,
                            g: fallback.g + (target.g - fallback.g) * mix,
                            b: fallback.b + (target.b - fallback.b) * mix,
                            a: fallback.a,
                        }
                    })
            };
            palette.background = color("bgDark", palette.background);
            palette.surface = color("bg", palette.surface);
            palette.elevated = color("bgHighlight", palette.elevated);
            palette.line = color("muted", palette.line);
            palette.foreground = color("fg", palette.foreground);
            palette.muted = color("fgDark", palette.muted);
            palette.ice = color("blue", palette.ice);
            palette.danger = color("red", palette.danger);
            palette.green = color("green", palette.green);
            palette.yellow = color("yellow", palette.yellow);
            palette.cyan = color("cyan", palette.cyan);
            palette.purple = color("magenta", palette.purple);
            palette.on_ice =
                if palette.ice.r * 0.299 + palette.ice.g * 0.587 + palette.ice.b * 0.114 > 0.60 {
                    Color::BLACK
                } else {
                    Color::WHITE
                };
        }
        palette
    }

    pub fn theme(self) -> Theme {
        Theme::custom(
            String::from("Frost"),
            IcedPalette {
                background: self.surface,
                text: self.foreground,
                primary: self.ice,
                success: self.green,
                warning: self.yellow,
                danger: self.danger,
            },
        )
    }

    fn tile(self, background: Color, radius: f32) -> container::Style {
        container::Style {
            background: Some(background.into()),
            text_color: Some(self.foreground),
            border: Border {
                color: self.line,
                width: 1.0,
                radius: radius.into(),
            },
            ..Default::default()
        }
    }

    fn button<'a>(
        self,
        content: impl Into<Element<'a, Message>>,
        active: bool,
    ) -> button::Button<'a, Message> {
        button(content).padding([6, 10]).style(move |_, status| {
            use button::Status;
            let (background, foreground) = match (active, status) {
                (true, _) => (self.ice, self.on_ice),
                (_, Status::Hovered | Status::Pressed) => (self.elevated, self.foreground),
                (_, Status::Disabled) => (Color::TRANSPARENT, self.muted),
                _ => (Color::TRANSPARENT, self.foreground),
            };
            button::Style {
                background: Some(background.into()),
                text_color: foreground,
                border: Border {
                    radius: 7.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
    }

    fn action<'a>(self, label: impl Into<String>, message: Message) -> button::Button<'a, Message> {
        self.button(text(label.into()).size(13), false)
            .on_press(message)
    }

    fn icon_action(
        self,
        glyph: &'static str,
        color: Color,
        message: Message,
    ) -> button::Button<'static, Message> {
        self.button(
            container(icon(glyph, 18, color)).center_x(38).center_y(30),
            false,
        )
        .padding(0)
        .on_press(message)
    }

    fn workspace<'a>(
        self,
        label: String,
        active: bool,
        urgent: bool,
        occupied: bool,
    ) -> button::Button<'a, Message> {
        button(text(label).size(12))
            .padding([5, 8])
            .style(move |_, status| {
                let background = if active {
                    self.ice
                } else if urgent {
                    self.danger
                } else if occupied {
                    self.elevated
                } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                    self.elevated
                } else {
                    Color::TRANSPARENT
                };
                button::Style {
                    background: Some(background.into()),
                    text_color: if active || urgent {
                        self.on_ice
                    } else if occupied {
                        self.ice
                    } else {
                        self.muted
                    },
                    border: Border {
                        radius: 7.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
    }

    fn label(self, value: impl Into<String>) -> text::Text<'static> {
        text(value.into()).size(12).color(self.muted)
    }

    fn session_tile(self, action: Session) -> button::Button<'static, Message> {
        let (glyph, tone) = match action {
            Session::Lock => ("󰌾", self.ice),
            Session::Suspend => ("󰤄", self.cyan),
            Session::Logout => ("󰍃", self.muted),
            Session::Reboot => ("󰑓", self.yellow),
            Session::PowerOff => ("󰐥", self.danger),
        };
        self.button(
            column![icon(glyph, 22, tone), text(action.label()).size(11)]
                .spacing(5)
                .align_x(Alignment::Center),
            false,
        )
        .width(Length::Fill)
        .padding([7, 4])
        .on_press(Message::Session(action))
    }

    fn meter(self, percent: u16) -> progress_bar::ProgressBar<'static> {
        progress_bar(0.0..=100.0, f32::from(percent))
            .girth(5)
            .style(move |_| progress_bar::Style {
                background: Background::Color(self.elevated),
                bar: Background::Color(self.ice),
                border: Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
            })
    }
}

fn icon(glyph: &'static str, size: u32, color: Color) -> text::Text<'static> {
    text(glyph)
        .font(crate::media::ICON_FONT)
        .size(size)
        .color(color)
}

fn hint<'a>(
    content: impl Into<Element<'a, Message>>,
    label: impl Into<String>,
    palette: Palette,
    position: tooltip::Position,
) -> Element<'a, Message> {
    tooltip(content, text(label.into()).size(12), position)
        .padding(6)
        .delay(std::time::Duration::from_millis(450))
        .style(move |_| palette.tile(palette.surface, 6.0))
        .into()
}

fn themed_icon(path: Option<&std::path::Path>, fallback: Color) -> Element<'static, Message> {
    let size = 28;
    match path {
        Some(path) if path.extension().is_some_and(|ext| ext == "svg") => {
            svg(svg::Handle::from_path(path.to_owned()))
                .width(size)
                .height(size)
                .into()
        }
        Some(path) => image(image::Handle::from_path(path.to_owned()))
            .width(size)
            .height(size)
            .content_fit(ContentFit::Contain)
            .into(),
        None => container(icon("󰍜", 17, fallback))
            .center_x(size)
            .center_y(size)
            .into(),
    }
}

fn rgb(value: u32) -> Color {
    Color::from_rgb8((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn heading(value: impl Into<String>, size: u32) -> text::Text<'static> {
    text(value.into()).size(size).font(Font {
        weight: Weight::Semibold,
        ..Font::DEFAULT
    })
}

fn shortened(value: &str, length: usize) -> String {
    let mut chars = value.chars();
    let mut output: String = chars.by_ref().take(length).collect();
    if chars.next().is_some() {
        output.push('…');
    }
    output
}

fn percent(value: Option<u16>) -> String {
    value.map_or_else(|| "—".into(), |value| format!("{value}%"))
}

fn level_text(value: Option<Level>) -> String {
    match value {
        Some(Level { muted: true, .. }) => "Muted".into(),
        Some(level) => percent(Some(level.percent)),
        None => "Unavailable".into(),
    }
}

impl Frost {
    pub(super) fn backdrop_view(&self) -> Element<'_, Message> {
        let p = self.palette;
        let purple = format!(
            "#{:02x}{:02x}{:02x}",
            (p.purple.r * 255.0) as u8,
            (p.purple.g * 255.0) as u8,
            (p.purple.b * 255.0) as u8
        );
        let art = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="600" viewBox="0 0 1000 600" preserveAspectRatio="none"><polygon points="640,-200 990,-200 580,800 230,800" fill="{purple}" opacity=".16"/><polygon points="900,-200 1130,-200 720,800 490,800" fill="{purple}" opacity=".38"/></svg>"#
        );
        let background: Element<'_, Message> = svg(svg::Handle::from_memory(art.into_bytes()))
            .width(Length::Fill)
            .height(Length::Fill)
            .content_fit(ContentFit::Fill)
            .into();
        let logo: Element<'_, Message> = container(
            image(crate::media::owl_icon())
                .width(180)
                .height(180)
                .content_fit(ContentFit::Contain)
                .opacity(0.25_f32),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(48)
        .align_x(Alignment::Start)
        .align_y(Alignment::End)
        .into();
        container(
            stack([background, logo])
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style::default().background(p.background))
        .into()
    }
    pub(super) fn panel_view(&self, panel: Panel) -> Element<'_, Message> {
        let p = self.palette;
        let title = row![
            heading(panel.title(), 23).width(Length::Fill),
            p.action("Close", Message::Dismiss),
        ]
        .align_y(Alignment::Center);
        let content = match panel {
            Panel::Launcher => self.launcher(),
            Panel::Dashboard => self.dashboard(),
            Panel::Wallpaper => self.wallpaper(),
            Panel::Session => self.session_view(),
        };
        container(column![title, content].spacing(13).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(17)
            .style(move |_| p.tile(p.surface, 13.0))
            .into()
    }

    fn launcher(&self) -> Element<'_, Message> {
        let p = self.palette;
        let mut input = text_input("Search applications", &self.query)
            .id(LAUNCHER_INPUT)
            .on_input(Message::Search)
            .padding(11)
            .size(15);
        if let Some(app) = self.matches().nth(self.selected) {
            input = input.on_submit(Message::Launch(app.path.clone()));
        }
        let mut items = column![].spacing(4);
        let mut count = 0;
        for (index, app) in self.matches().enumerate() {
            let focused = index == self.selected;
            let detail = if app.detail.is_empty() {
                app.path.file_stem().unwrap_or_default().to_string_lossy()
            } else {
                app.detail.as_str().into()
            };
            let body = column![
                heading(shortened(&app.name, 48), 15),
                text(shortened(&detail, 63)).size(11).color(if focused {
                    p.on_ice
                } else {
                    p.muted
                }),
            ]
            .spacing(3);
            items = items.push(
                p.button(
                    row![
                        themed_icon(app.icon.as_deref(), if focused { p.on_ice } else { p.ice }),
                        body
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                    focused,
                )
                .padding([6, 10])
                .height(46)
                .width(Length::Fill)
                .on_press(Message::Launch(app.path.clone())),
            );
            count += 1;
        }
        if count == 0 {
            items = items.push(
                container(p.label(if self.loading_apps {
                    "Loading applications…"
                } else {
                    "No matching applications"
                }))
                .padding(10),
            );
        }
        column![
            input,
            scrollable(items.padding([2, 5]))
                .id("frost-app-list")
                .height(Length::Fill),
            p.label("↑ ↓ Select    Enter Open    Esc Close"),
        ]
        .spacing(9)
        .height(Length::Fill)
        .into()
    }

    fn dashboard(&self) -> Element<'_, Message> {
        let p = self.palette;
        let s = &self.snapshot;
        let metric = |label, value| {
            container(column![p.label(label), heading(value, 20)].spacing(2))
                .width(Length::FillPortion(1))
                .padding([8, 10])
                .style(move |_| p.tile(p.background, 8.0))
        };
        let mut metrics = row![
            metric("CPU", percent(self.cpu_usage)),
            metric("Memory", percent(s.memory)),
            metric("Load · 1 min", s.load.clone().unwrap_or_else(|| "—".into())),
        ]
        .spacing(6);
        if let Some(battery) = s.battery {
            metrics = metrics.push(metric("Battery", percent(Some(battery.percent))));
        }
        let header = |label, level| {
            row![
                heading(label, 14).width(Length::Fill),
                p.label(level_text(level))
            ]
            .align_y(Alignment::Center)
        };
        let audio = column![
            row![
                heading("Volume", 14).width(Length::Fill),
                p.label(level_text(s.volume)),
                p.action(
                    if s.volume.is_some_and(|level| level.muted) {
                        "Unmute"
                    } else {
                        "Mute"
                    },
                    Message::Adjust(Control::VolumeMute)
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            slider(
                0..=100,
                s.volume.map_or(0, |level| level.percent.min(100)),
                Message::SetVolume
            )
            .step(1_u16),
        ]
        .spacing(6);
        let backlight = column![
            header("Brightness", s.brightness),
            slider(
                1..=100,
                s.brightness.map_or(1, |level| level.percent.clamp(1, 100)),
                Message::SetBrightness
            )
            .step(1_u16),
        ]
        .spacing(6);
        let music = if let Some(music) = &s.music {
            column![
                heading(shortened(&music.title, 37), 15),
                p.label(shortened(&music.artist, 48)),
                row![
                    hint(
                        p.icon_action("󰑟", p.foreground, Message::Music("album-prev")),
                        "Previous album",
                        p,
                        tooltip::Position::Top
                    ),
                    hint(
                        p.icon_action("󰒮", p.foreground, Message::Music("-r")),
                        "Previous track",
                        p,
                        tooltip::Position::Top
                    ),
                    hint(
                        p.icon_action(
                            if music.playing { "󰏤" } else { "󰐊" },
                            p.ice,
                            Message::Music("-u"),
                        ),
                        if music.playing { "Pause" } else { "Play" },
                        p,
                        tooltip::Position::Top
                    ),
                    hint(
                        p.icon_action("󰒭", p.foreground, Message::Music("-n")),
                        "Next track",
                        p,
                        tooltip::Position::Top
                    ),
                    hint(
                        p.icon_action("󰈑", p.foreground, Message::Music("album-next")),
                        "Next album",
                        p,
                        tooltip::Position::Top
                    ),
                ]
                .spacing(4),
            ]
            .spacing(4)
        } else {
            column![
                heading("cmus", 15),
                p.action("Start cmus", Message::Music("start"))
            ]
            .spacing(4)
        };
        column![
            metrics,
            container(
                row![
                    icon("󰖐", 22, p.cyan),
                    text(self.weather.as_deref().unwrap_or("Weather unavailable")).size(12)
                ]
                .spacing(12)
                .align_y(Alignment::Center)
            )
            .width(Length::Fill)
            .padding(10)
            .style(move |_| p.tile(p.background, 8.0)),
            audio,
            backlight,
            container(music)
                .width(Length::Fill)
                .padding(10)
                .style(move |_| p.tile(p.background, 8.0)),
            row![
                p.session_tile(Session::Lock),
                p.session_tile(Session::Logout),
                p.session_tile(Session::Reboot),
                p.session_tile(Session::PowerOff),
            ]
            .spacing(8),
        ]
        .spacing(10)
        .height(Length::Fill)
        .into()
    }

    fn wallpaper(&self) -> Element<'_, Message> {
        let p = self.palette;
        let selected = match &self.wallpaper_target {
            Some(crate::WallpaperChoice::Image(path)) => Some(path.clone()),
            Some(crate::WallpaperChoice::Clear) => None,
            None => self.active_wallpaper.clone(),
        };
        let folder = crate::services::wallpaper_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "Set FROST_WALLPAPERS to the wallpaper directory".into());
        let mut items = column![].spacing(7);
        for (row_index, paths) in self.wallpaper_files.chunks(3).enumerate() {
            let mut tiles = row![].spacing(7);
            for (column_index, path) in paths.iter().enumerate() {
                let active = selected.as_ref() == Some(path);
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let preview = self
                    .thumbnails
                    .get(row_index * 3 + column_index)
                    .and_then(Option::as_ref);
                let preview: Element<'_, Message> = match preview {
                    Some(handle) => image(handle.clone())
                        .height(93)
                        .width(Length::Fill)
                        .content_fit(ContentFit::Contain)
                        .into(),
                    None => container(icon("󰋩", 24, p.muted))
                        .center_x(Length::Fill)
                        .center_y(93)
                        .into(),
                };
                let tile = column![preview, text(shortened(&name, 18)).size(11)]
                    .spacing(6)
                    .align_x(Alignment::Center)
                    .width(Length::Fill);
                tiles = tiles.push(
                    button(tile)
                        .padding(5)
                        .width(Length::FillPortion(1))
                        .style(move |_, status| {
                            let hovered =
                                matches!(status, button::Status::Hovered | button::Status::Pressed);
                            button::Style {
                                background: Some(if active {
                                    p.elevated.into()
                                } else if hovered {
                                    p.surface.into()
                                } else {
                                    Color::TRANSPARENT.into()
                                }),
                                text_color: p.foreground,
                                border: Border {
                                    color: if active { p.ice } else { p.line },
                                    width: if active || hovered { 2.0 } else { 0.0 },
                                    radius: 8.0.into(),
                                },
                                ..Default::default()
                            }
                        })
                        .on_press(Message::Wallpaper(path.clone())),
                );
            }
            for _ in paths.len()..3 {
                tiles = tiles.push(Space::new().width(Length::FillPortion(1)));
            }
            items = items.push(tiles);
        }
        if self.wallpaper_files.is_empty() {
            items = items.push(p.label("No wallpapers found"));
        }
        column![
            row![
                p.label(shortened(&folder, 48)).width(Length::Fill),
                p.action("Clear", Message::WallpaperClear)
            ]
            .align_y(Alignment::Center),
            row![
                p.label("Theme mix"),
                slider(0.0..=1.0, self.theme_mix, Message::ThemeMix)
                    .step(0.01_f32)
                    .width(Length::Fill),
                p.label(format!(
                    "{}%",
                    (self.theme_mix * 100.0).round() as u32
                ))
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            scrollable(items).height(Length::Fill),
        ]
        .spacing(8)
        .height(Length::Fill)
        .into()
    }

    fn session_view(&self) -> Element<'_, Message> {
        let p = self.palette;
        if let Some(action) = self.confirm {
            return column![
                heading(format!("{}?", action.label()), 18),
                Space::new().height(Length::Fill),
                row![
                    p.action("Cancel", Message::Cancel),
                    p.button(text(action.label()).size(13), true)
                        .on_press(Message::Confirm(action)),
                ]
                .spacing(8),
            ]
            .spacing(10)
            .height(Length::Fill)
            .into();
        }
        column(
            [
                Session::Lock,
                Session::Suspend,
                Session::Logout,
                Session::Reboot,
                Session::PowerOff,
            ]
            .into_iter()
            .map(|action| {
                let (glyph, tone) = match action {
                    Session::Lock => ("󰌾", p.ice),
                    Session::Suspend => ("󰤄", p.cyan),
                    Session::Logout => ("󰍃", p.muted),
                    Session::Reboot => ("󰑓", p.yellow),
                    Session::PowerOff => ("󰐥", p.danger),
                };
                p.button(
                    row![icon(glyph, 22, tone), text(action.label()).size(14)]
                        .spacing(13)
                        .align_y(Alignment::Center),
                    false,
                )
                .width(Length::Fill)
                .padding([10, 12])
                .on_press(Message::Session(action))
                .into()
            }),
        )
        .spacing(5)
        .into()
    }

    pub(super) fn notice_view(&self, notice: &Notice) -> Element<'_, Message> {
        let p = self.palette;
        let mut content = column![heading(&notice.title, 16)].spacing(7);
        if !notice.detail.is_empty() {
            content = content.push(p.label(shortened(&notice.detail, 70)));
        }
        if let Some(level) = notice.level {
            content = content.push(p.meter(level));
        }
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([10, 15])
            .align_y(Alignment::Center)
            .style(move |_| p.tile(p.surface, 12.0))
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::App;
    use iced::{Size, mouse};
    use iced_runtime::user_interface::{Cache, UserInterface};
    use iced_tiny_skia::core::{layout, widget::Tree};
    use iced_tiny_skia::{Renderer, core::renderer, graphics::Viewport, window::compositor};

    #[test]
    fn backdrop_renders_the_owl_in_the_lower_left_corner() {
        let (frost, _tasks) = Frost::new(iced_wayland_subscriber::shell::channel().1);
        let theme = frost.palette.theme();
        for scale in [1_usize, 2] {
            let width = 1000 * scale;
            let viewport = Viewport::with_physical_size(
                Size::new(width as u32, (600 * scale) as u32),
                scale as f32,
            );
            let mut renderer = iced::Renderer::Secondary(Renderer::new(Font::DEFAULT, 16.0.into()));
            let mut ui = UserInterface::build(
                frost.backdrop_view(),
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
            let frame = match &mut renderer {
                iced::Renderer::Secondary(renderer) => {
                    compositor::screenshot(renderer, &viewport, Color::TRANSPARENT)
                }
                iced::Renderer::Primary(_) => unreachable!(),
            };
            let pixel = |x: usize, y: usize| &frame[4 * (y * width + x)..4 * (y * width + x) + 4];
            let background = pixel(20 * scale, 400 * scale).to_vec();
            let logo_pixels = (48 * scale..228 * scale)
                .flat_map(|x| (372 * scale..552 * scale).map(move |y| (x, y)))
                .filter(|&(x, y)| pixel(x, y) != background)
                .count();
            assert!(
                logo_pixels > 100 * scale * scale,
                "owl did not render at scale {scale}"
            );
        }
    }

    #[test]
    fn dashboard_actions_fit_without_scrolling() {
        let (mut frost, _tasks) = Frost::new(iced_wayland_subscriber::shell::channel().1);
        frost.snapshot.music = Some(crate::services::Music {
            title: "A long example track title".into(),
            artist: "An artist".into(),
            album: "An album".into(),
            playing: true,
        });
        frost.snapshot.volume = Some(Level {
            percent: 40,
            muted: false,
        });
        frost.snapshot.brightness = Some(Level {
            percent: 60,
            muted: false,
        });
        let renderer = iced::Renderer::Secondary(Renderer::new(Font::DEFAULT, 16.0.into()));
        let mut element = frost.panel_view(Panel::Dashboard);
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(420.0, 520.0)),
        );
        let panel = &node.children()[0];
        let dashboard = &panel.children()[1];
        let actions = dashboard.children().last().unwrap();
        let bottom =
            panel.bounds().y + dashboard.bounds().y + actions.bounds().y + actions.bounds().height;
        assert!(
            bottom <= 520.0,
            "dashboard actions extend below the panel: {bottom}"
        );
    }

    #[test]
    fn renders_a_desktop_icon_and_a_wallpaper_thumbnail() {
        let file =
            std::env::temp_dir().join(format!("frost-visual-test-{}.png", std::process::id()));
        let marker = [244, 60, 133, 255];
        let source = ::image::RgbaImage::from_pixel(570, 390, ::image::Rgba(marker));
        source.save(&file).unwrap();

        let (mut frost, _tasks) = Frost::new(iced_wayland_subscriber::shell::channel().1);
        frost.apps.push(App {
            name: "Sample application".into(),
            detail: String::new(),
            search: "sample application".into(),
            path: "sample.desktop".into(),
            icon: crate::media::app_icon(file.to_str().unwrap(), "hicolor"),
        });
        frost.wallpaper_files.push(file.clone());
        frost.thumbnails.push(crate::media::thumbnail(&file));
        assert!(frost.thumbnails[0].is_some());

        for (panel, width, height) in [(Panel::Launcher, 520, 470), (Panel::Wallpaper, 550, 480)] {
            let viewport = Viewport::with_physical_size(Size::new(width, height), 1.0);
            let theme = frost.palette.theme();
            let mut renderer = iced::Renderer::Secondary(Renderer::new(Font::DEFAULT, 16.0.into()));
            let mut ui = UserInterface::build(
                frost.panel_view(panel),
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
            let frame = match &mut renderer {
                iced::Renderer::Secondary(renderer) => {
                    compositor::screenshot(renderer, &viewport, Color::TRANSPARENT)
                }
                iced::Renderer::Primary(_) => unreachable!(),
            };
            assert!(
                frame.as_chunks::<4>().0.contains(&marker),
                "{panel:?} did not render the image"
            );
        }
        std::fs::remove_file(file).unwrap();
    }
}
