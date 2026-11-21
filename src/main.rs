mod config;
mod ipc;
mod lock;
mod media;
mod services;
mod telemetry;
mod ui;

use iced::{
    Color, Element, Subscription, Task, Theme, event,
    futures::{StreamExt, join, stream},
    keyboard::{self, Key, key::Named},
    widget::{image, operation},
    window::{self, Id},
};
use iced_exwlshell::{
    daemon, get_wlconnection,
    reexport::{
        Anchor, KeyboardInteractivity, Layer, LayerSize, Margin, NewLayerShellSettings,
        OutputOption,
    },
    settings::{ExWlSettings, LayerShellSettings, StartMode},
    to_layer_message,
};
use iced_wayland_subscriber::{
    OutputId,
    shell::{self, ShellEvent, ShellReceiver},
    workspace::{self, WorkspaceEvent, WorkspaceId, WorkspaceSnapshot},
};
use services::{App, Compositor, Control, Level, Music, Session, Workspace};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::{Duration, Instant},
};

const LAUNCHER_INPUT: &str = "frost-search";

#[derive(Debug, Clone, PartialEq, Eq)]
enum WallpaperChoice {
    Image(PathBuf),
    Clear,
}

const HELP: &str = "Usage: frost [run | launcher | dashboard | wallpaper | session | close | lock |
              volume-up | volume-down | volume-mute | brightness-up | brightness-down]";

fn main() -> ExitCode {
    if let Err(err) = run() {
        eprintln!("frost: {err}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "run".into());
    if args.next().is_some() {
        return Err(HELP.into());
    }
    match command.as_str() {
        "-h" | "--help" => println!("{HELP}"),
        "-V" | "--version" => println!("frost {}", env!("CARGO_PKG_VERSION")),
        "lock" => lock::run()?,
        "run" => {
            let config = config::Config::load();
            let socket = ipc::Socket::bind()?;
            let (shell_broadcast, shell_events) = shell::channel();
            daemon(
                move || Frost::new(shell_events.clone()),
                "frost",
                Frost::update,
                Frost::view,
            )
            .wl_settings(ExWlSettings {
                shell_broadcast,
                layer_settings: LayerShellSettings {
                    size: LayerSize::fill_width(config.bar.height),
                    anchor: Anchor::Top | Anchor::Left | Anchor::Right,
                    exclusive_zone: config.bar.height as i32,
                    margin: Margin {
                        top: config.bar.margin,
                        right: config.bar.margin,
                        bottom: 0,
                        left: config.bar.margin,
                    },
                    layer: Layer::Top,
                    keyboard_interactivity: KeyboardInteractivity::None,
                    start_mode: StartMode::AllScreens,
                    ..Default::default()
                },
                ..Default::default()
            })
            .font(media::SYMBOLS)
            .theme(|state: &Frost, _| state.palette.theme())
            .style(|_, _| iced::theme::Style {
                background_color: Color::TRANSPARENT,
                text_color: Color::WHITE,
            })
            .subscription(move |state| {
                Subscription::batch([
                    state.subscription(),
                    Subscription::run_with(socket.clone(), ipc::Socket::messages),
                ])
            })
            .run()?;
        }
        command if parse_command(command).is_some() => ipc::send(command)
            .map_err(|err| format!("could not contact Frost on this Wayland display: {err}"))?,
        _ => return Err(HELP.into()),
    }
    Ok(())
}

fn parse_command(command: &str) -> Option<Message> {
    Some(match command {
        "launcher" => Message::Toggle(Panel::Launcher),
        "dashboard" => Message::Toggle(Panel::Dashboard),
        "wallpaper" => Message::Toggle(Panel::Wallpaper),
        "session" => Message::Toggle(Panel::Session),
        "close" => Message::Dismiss,
        "volume-up" => Message::Adjust(Control::VolumeUp),
        "volume-down" => Message::Adjust(Control::VolumeDown),
        "volume-mute" => Message::Adjust(Control::VolumeMute),
        "brightness-up" => Message::Adjust(Control::BrightnessUp),
        "brightness-down" => Message::Adjust(Control::BrightnessDown),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    Launcher,
    Dashboard,
    Wallpaper,
    Session,
}

impl Panel {
    fn title(self) -> &'static str {
        match self {
            Self::Launcher => "Applications",
            Self::Dashboard => "Dashboard",
            Self::Wallpaper => "Wallpaper",
            Self::Session => "Session",
        }
    }

    fn placement(self) -> (u32, u32, Anchor) {
        match self {
            Self::Launcher => (520, 470, Anchor::Top | Anchor::Left),
            Self::Dashboard => (420, 520, Anchor::Top | Anchor::Right),
            Self::Wallpaper => (550, 480, Anchor::Top | Anchor::Right),
            Self::Session => (310, 330, Anchor::Top | Anchor::Right),
        }
    }
}

#[derive(Debug, Clone)]
struct Notice {
    title: String,
    detail: String,
    level: Option<u16>,
}

impl Notice {
    fn level(title: &str, level: Level) -> Self {
        Self {
            title: title.to_owned(),
            detail: if level.muted {
                "Muted".into()
            } else {
                format!("{}%", level.percent)
            },
            level: Some(if level.muted { 0 } else { level.percent }),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Snapshot {
    workspaces: Vec<Workspace>,
    volume: Option<Level>,
    brightness: Option<Level>,
    music: Option<Music>,
    battery: Option<services::Battery>,
    memory: Option<u16>,
    cpu: Option<telemetry::Cpu>,
    load: Option<String>,
    /// When this read began; older reads must not override local changes.
    started: Option<Instant>,
}

impl Snapshot {
    async fn read(compositor: Compositor, dashboard: bool) -> Self {
        let started = Instant::now();
        let music = async {
            if dashboard {
                services::music().await
            } else {
                None
            }
        };
        let (workspaces, volume, brightness, music) = join!(
            services::workspaces(compositor),
            services::volume(),
            services::brightness(),
            music,
        );
        Self {
            workspaces,
            volume,
            brightness,
            music,
            battery: services::battery(),
            memory: services::memory(),
            cpu: telemetry::Cpu::read(),
            load: services::load(),
            started: Some(started),
        }
    }
}

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
enum Message {
    Tick,
    Snapshot(Snapshot),
    Network(telemetry::Network),
    PowerRead(Option<telemetry::PowerProfiles>),
    Weather(Option<String>),
    CyclePower,
    ToggleBatteryTime,
    OpenNetwork,
    PowerChanged(Result<telemetry::PowerProfiles, String>),
    AppsLoaded(Id, Vec<App>),
    Thumbnail(Id, usize, Option<image::Handle>),
    Opened(Id),
    Closed(Id),
    Key(Id, Named),
    Toggle(Panel),
    Dismiss,
    Search(String),
    Launch(PathBuf),
    Switch(Workspace),
    NativeWorkspaces(Option<Arc<WorkspaceSnapshot>>),
    NativeActivate(WorkspaceId),
    OutputChanged(Id, Option<OutputId>),
    OutputAdded(OutputId),
    OutputRemoved(OutputId),
    Adjust(Control),
    Adjusted(Control, Result<Option<Level>, String>),
    SetVolume(u16),
    SetBrightness(u16),
    LevelSet(bool, u16, Result<Option<Level>, String>),
    Music(&'static str),
    Wallpaper(PathBuf),
    WallpaperClear,
    WallpaperDone(WallpaperChoice, Result<String, String>),
    ThemeMix(f32),
    ThemeMixRelease,
    Session(Session),
    Confirm(Session),
    Cancel,
    Finished(Result<String, String>, Option<&'static str>),
    Expire(u64),
}

struct Frost {
    config: config::Config,
    compositor: Compositor,
    shell_events: ShellReceiver,
    native_workspaces: Option<Arc<WorkspaceSnapshot>>,
    outputs: HashMap<Id, OutputId>,
    known_outputs: HashSet<OutputId>,
    backdrops: HashMap<OutputId, Id>,
    panel: Option<(Panel, Id)>,
    notice: Option<(Notice, Id)>,
    notice_generation: u64,
    palette: ui::Palette,
    /// File-backed state, refreshed on each tick so views never touch disk.
    active_wallpaper: Option<PathBuf>,
    theme_mix: f32,
    /// The slider moved since the theme mix was last saved.
    theme_mix_dirty: bool,
    window_counts: Option<HashMap<String, u32>>,
    confirm: Option<Session>,
    apps: Vec<App>,
    loading_apps: bool,
    query: String,
    selected: usize,
    wallpaper_files: Vec<PathBuf>,
    wallpaper_target: Option<WallpaperChoice>,
    wallpaper_busy: bool,
    thumbnails: Vec<Option<image::Handle>>,
    thumbnail_job: Option<iced::task::Handle>,
    cpu_usage: Option<u16>,
    network: telemetry::Network,
    network_pending: bool,
    network_checked: Option<Instant>,
    power: Option<telemetry::PowerProfiles>,
    power_pending: bool,
    power_checked: Option<Instant>,
    battery_show_time: bool,
    weather: Option<String>,
    weather_checked: Option<Instant>,
    weather_pending: bool,
    snapshot: Snapshot,
    controls: VecDeque<Control>,
    adjusting: bool,
    volume_target: Option<u16>,
    brightness_target: Option<u16>,
    volume_setting: bool,
    brightness_setting: bool,
    /// Last time Frost itself changed the volume or brightness.
    volume_changed: Option<Instant>,
    brightness_changed: Option<Instant>,
    refreshing: bool,
    clock: chrono::DateTime<chrono::Local>,
}

impl Frost {
    fn new(shell_events: ShellReceiver) -> (Self, Task<Message>) {
        let mut frost = Self {
            config: config::Config::load(),
            compositor: Compositor::detect(),
            shell_events,
            native_workspaces: None,
            outputs: HashMap::new(),
            known_outputs: HashSet::new(),
            backdrops: HashMap::new(),
            panel: None,
            notice: None,
            notice_generation: 0,
            palette: ui::Palette::load(),
            active_wallpaper: None,
            theme_mix: 1.0,
            theme_mix_dirty: false,
            window_counts: None,
            confirm: None,
            apps: Vec::new(),
            loading_apps: false,
            query: String::new(),
            selected: 0,
            wallpaper_files: Vec::new(),
            wallpaper_target: None,
            wallpaper_busy: false,
            thumbnails: Vec::new(),
            thumbnail_job: None,
            cpu_usage: None,
            network: telemetry::Network::default(),
            network_pending: false,
            network_checked: None,
            power: None,
            power_pending: false,
            power_checked: None,
            battery_show_time: false,
            weather: None,
            weather_checked: None,
            weather_pending: false,
            snapshot: Snapshot::default(),
            controls: VecDeque::new(),
            adjusting: false,
            volume_target: None,
            brightness_target: None,
            volume_setting: false,
            brightness_setting: false,
            volume_changed: None,
            brightness_changed: None,
            refreshing: false,
            clock: chrono::Local::now(),
        };
        frost.refresh_files();
        let poll = Task::batch([frost.poll(), frost.poll_bar()]);
        (frost, poll)
    }

    fn subscription(&self) -> Subscription<Message> {
        let workspace_events = if self.compositor == Compositor::Other {
            workspace::listen(get_wlconnection()).map(|event| {
                Message::NativeWorkspaces(match event {
                    WorkspaceEvent::Updated(snapshot) => Some(snapshot),
                    _ => None,
                })
            })
        } else {
            Subscription::none()
        };
        Subscription::batch([
            workspace_events,
            self.shell_events.listen().filter_map(|event| match event {
                ShellEvent::WindowOutputChanged { window, output } => Some(Message::OutputChanged(
                    window,
                    output.as_ref().map(OutputId::from),
                )),
                ShellEvent::OutputAdded(output) => {
                    Some(Message::OutputAdded(OutputId::from(&output)))
                }
                ShellEvent::OutputRemoved(output) => {
                    Some(Message::OutputRemoved(OutputId::from(&output)))
                }
                _ => None,
            }),
            iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
            window::open_events().map(Message::Opened),
            window::close_events().map(Message::Closed),
            event::listen_with(|event, _, id| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: Key::Named(key @ (Named::Escape | Named::ArrowUp | Named::ArrowDown)),
                    ..
                }) => Some(Message::Key(id, key)),
                _ => None,
            }),
        ])
    }

    fn poll(&mut self) -> Task<Message> {
        if self.refreshing {
            return Task::none();
        }
        self.refreshing = true;
        let dashboard = self
            .panel
            .is_some_and(|(panel, _)| panel == Panel::Dashboard);
        Task::perform(
            Snapshot::read(self.compositor, dashboard),
            Message::Snapshot,
        )
    }

    fn poll_bar(&mut self) -> Task<Message> {
        let now = Instant::now();
        let due = |last: Option<Instant>| {
            last.is_none_or(|last| now.duration_since(last) >= Duration::from_secs(4))
        };
        let mut jobs = Vec::new();
        if !self.network_pending && due(self.network_checked) {
            self.network_pending = true;
            self.network_checked = Some(now);
            jobs.push(Task::perform(telemetry::network(), Message::Network));
        }
        if !self.power_pending && due(self.power_checked) {
            self.power_pending = true;
            self.power_checked = Some(now);
            jobs.push(Task::perform(
                async { telemetry::PowerProfiles::read().await.ok() },
                Message::PowerRead,
            ));
        }
        Task::batch(jobs)
    }

    fn poll_weather(&mut self) -> Task<Message> {
        if self.weather_pending
            || self
                .panel
                .is_none_or(|(panel, _)| panel != Panel::Dashboard)
            || self
                .weather_checked
                .is_some_and(|last| last.elapsed() < Duration::from_secs(15 * 60))
        {
            return Task::none();
        }
        self.weather_pending = true;
        let location = self.config.weather.location.clone();
        Task::perform(
            async move { services::weather(location.as_deref()).await },
            Message::Weather,
        )
    }

    fn matches(&self) -> impl Iterator<Item = &App> {
        let terms: Vec<_> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        self.apps
            .iter()
            .filter(move |app| terms.iter().all(|term| app.search.contains(term)))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                self.clock = chrono::Local::now();
                self.refresh_files();
                Task::batch([
                    self.poll(),
                    self.poll_bar(),
                    self.poll_weather(),
                    self.reconcile_backdrops(),
                ])
            }
            Message::Snapshot(mut snapshot) => {
                self.refreshing = false;
                // A read that began before Frost changed a level, or while a
                // change is still in flight, carries a stale value: keep ours
                // and do not announce it as an external change.
                let stale = |changed: Option<Instant>, busy: bool| {
                    busy || changed.is_some_and(|changed| {
                        snapshot.started.is_none_or(|started| started <= changed)
                    })
                };
                let volume_stale = stale(
                    self.volume_changed,
                    self.adjusting || self.volume_setting || self.volume_target.is_some(),
                );
                let brightness_stale = stale(
                    self.brightness_changed,
                    self.adjusting || self.brightness_setting || self.brightness_target.is_some(),
                );
                if volume_stale {
                    snapshot.volume = self.snapshot.volume;
                }
                if brightness_stale {
                    snapshot.brightness = self.snapshot.brightness;
                }
                let notice = if let (Some(previous), Some(next)) =
                    (self.snapshot.volume, snapshot.volume)
                    && previous != next
                {
                    Some(Notice::level("Volume", next))
                } else if let (Some(previous), Some(next)) =
                    (self.snapshot.brightness, snapshot.brightness)
                    && previous != next
                {
                    Some(Notice::level("Brightness", next))
                } else {
                    None
                };
                self.cpu_usage = snapshot
                    .cpu
                    .zip(self.snapshot.cpu)
                    .and_then(|(current, previous)| current.usage(previous));
                self.snapshot = snapshot;
                notice.map_or_else(Task::none, |notice| self.show_notice(notice))
            }
            Message::Network(network) => {
                self.network_pending = false;
                self.network = network;
                Task::none()
            }
            Message::PowerRead(power) => {
                self.power_pending = false;
                self.power = power;
                Task::none()
            }
            Message::ToggleBatteryTime => {
                self.battery_show_time = !self.battery_show_time;
                Task::none()
            }
            Message::Weather(weather) => {
                self.weather_pending = false;
                self.weather_checked = Some(Instant::now());
                self.weather = weather;
                Task::none()
            }
            Message::CyclePower if !self.power_pending => {
                self.power_pending = true;
                Task::perform(telemetry::PowerProfiles::cycle(), Message::PowerChanged)
            }
            Message::OpenNetwork => Task::perform(services::open_nmtui(), |result| {
                Message::Finished(result, None)
            }),
            Message::PowerChanged(result) => {
                self.power_pending = false;
                self.power_checked = Some(Instant::now());
                match result {
                    Ok(power) => {
                        let label = power.active.label();
                        self.power = Some(power);
                        self.show_notice(Notice {
                            title: "Power profile".into(),
                            detail: label.into(),
                            level: None,
                        })
                    }
                    Err(err) => self.error(err),
                }
            }
            Message::AppsLoaded(id, apps) if self.panel == Some((Panel::Launcher, id)) => {
                self.loading_apps = false;
                self.apps = apps;
                self.selected = self.selected.min(self.matches().count().saturating_sub(1));
                Task::none()
            }
            Message::Thumbnail(id, index, preview)
                if self.panel == Some((Panel::Wallpaper, id)) =>
            {
                if let Some(item) = self.thumbnails.get_mut(index) {
                    *item = preview;
                }
                Task::none()
            }
            Message::Opened(id) if self.panel == Some((Panel::Launcher, id)) => {
                operation::focus(LAUNCHER_INPUT)
            }
            Message::Closed(id) => {
                self.outputs.remove(&id);
                self.backdrops.retain(|_, current| *current != id);
                if self.panel.is_some_and(|(_, active)| active == id) {
                    self.thumbnail_job = None;
                    self.panel = None;
                }
                if self
                    .notice
                    .as_ref()
                    .is_some_and(|(_, active)| *active == id)
                {
                    self.notice = None;
                }
                Task::none()
            }
            Message::Key(id, key) => match self.panel {
                Some((_, active)) if active == id && key == Named::Escape => self.dismiss(),
                Some((Panel::Launcher, active)) if active == id => {
                    let count = self.matches().count();
                    self.selected = if key == Named::ArrowUp {
                        self.selected.saturating_sub(1)
                    } else {
                        (self.selected + 1).min(count.saturating_sub(1))
                    };
                    self.reveal_selection(count)
                }
                _ => Task::none(),
            },
            Message::Toggle(panel) => self.toggle(panel),
            Message::Dismiss => self.dismiss(),
            Message::Search(query) => {
                self.query = query;
                self.selected = 0;
                self.reveal_selection(0)
            }
            Message::Launch(path) => Task::perform(services::launch(path), |result| {
                Message::Finished(result, Some(""))
            }),
            Message::Switch(workspace) => {
                Task::perform(services::switch(self.compositor, workspace), |result| {
                    Message::Finished(result, None)
                })
            }
            Message::NativeWorkspaces(snapshot) => {
                self.native_workspaces = snapshot;
                Task::none()
            }
            Message::NativeActivate(id) => {
                let result = self
                    .native_workspaces
                    .as_ref()
                    .map(|snapshot| snapshot.activate(&id));
                match result {
                    Some(Err(err)) => self.error(err.to_string()),
                    _ => Task::none(),
                }
            }
            Message::OutputChanged(id, output) => {
                if let Some(output) = output {
                    self.outputs.insert(id, output);
                } else {
                    self.outputs.remove(&id);
                }
                Task::none()
            }
            Message::OutputAdded(output) => {
                self.known_outputs.insert(output);
                self.reconcile_backdrops()
            }
            Message::OutputRemoved(output) => {
                self.known_outputs.remove(&output);
                self.backdrops
                    .remove(&output)
                    .map_or_else(Task::none, window::close)
            }
            Message::Adjust(control) => {
                if self.controls.len() < 32 {
                    self.controls.push_back(control);
                }
                self.next_control()
            }
            Message::SetVolume(value) => {
                self.volume_changed = Some(Instant::now());
                self.volume_target = Some(value.min(100));
                if let Some(level) = &mut self.snapshot.volume {
                    level.percent = value.min(100);
                }
                self.next_level(true)
            }
            Message::SetBrightness(value) => {
                self.brightness_changed = Some(Instant::now());
                self.brightness_target = Some(value.clamp(1, 100));
                if let Some(level) = &mut self.snapshot.brightness {
                    level.percent = value.clamp(1, 100);
                }
                self.next_level(false)
            }
            Message::LevelSet(volume, sent, result) => {
                if volume {
                    self.volume_setting = false;
                    self.volume_changed = Some(Instant::now());
                } else {
                    self.brightness_setting = false;
                    self.brightness_changed = Some(Instant::now());
                }
                match result {
                    Ok(level) => {
                        let target = if volume {
                            self.volume_target
                        } else {
                            self.brightness_target
                        };
                        if target == Some(sent) {
                            if volume {
                                self.volume_target = None;
                                self.snapshot.volume = level;
                            } else {
                                self.brightness_target = None;
                                self.snapshot.brightness = level;
                            }
                        }
                        self.next_level(volume)
                    }
                    Err(err) => {
                        if volume {
                            self.volume_target = None;
                        } else {
                            self.brightness_target = None;
                        }
                        self.error(err)
                    }
                }
            }
            Message::Adjusted(control, result) => {
                self.adjusting = false;
                let notice = match result {
                    Ok(Some(level))
                        if matches!(control, Control::BrightnessUp | Control::BrightnessDown) =>
                    {
                        self.brightness_changed = Some(Instant::now());
                        self.snapshot.brightness = Some(level);
                        self.show_notice(Notice::level("Brightness", level))
                    }
                    Ok(Some(level)) => {
                        self.volume_changed = Some(Instant::now());
                        self.snapshot.volume = Some(level);
                        self.show_notice(Notice::level("Volume", level))
                    }
                    Ok(None) => Task::none(),
                    Err(err) => self.error(err),
                };
                Task::batch([notice, self.next_control()])
            }
            Message::Music(flag) => Task::perform(
                async move { services::music_action(flag).await },
                |result| Message::Finished(result, None),
            ),
            Message::Wallpaper(path) => {
                self.wallpaper_target = Some(WallpaperChoice::Image(path));
                self.next_wallpaper()
            }
            Message::WallpaperClear => {
                self.wallpaper_target = Some(WallpaperChoice::Clear);
                self.next_wallpaper()
            }
            Message::WallpaperDone(choice, result) => {
                self.wallpaper_busy = false;
                self.refresh_files();
                let backdrops = self.reconcile_backdrops();
                if self.wallpaper_target.as_ref() != Some(&choice) {
                    return Task::batch([backdrops, self.next_wallpaper()]);
                }
                self.wallpaper_target = None;
                let notice = match result {
                    Ok(_) => self.show_notice(Notice {
                        title: match choice {
                            WallpaperChoice::Image(_) => "Wallpaper applied",
                            WallpaperChoice::Clear => "Wallpaper cleared",
                        }
                        .into(),
                        detail: String::new(),
                        level: None,
                    }),
                    Err(err) => self.error(err),
                };
                Task::batch([backdrops, notice])
            }
            Message::ThemeMix(value) => {
                // Preview in memory while dragging; the file is written on release.
                self.theme_mix = value.clamp(0.0, 1.0);
                self.theme_mix_dirty = true;
                if !self.wallpaper_busy {
                    self.palette =
                        ui::Palette::resolve(self.active_wallpaper.as_deref(), self.theme_mix);
                }
                Task::none()
            }
            Message::ThemeMixRelease => {
                if !std::mem::take(&mut self.theme_mix_dirty) {
                    return Task::none();
                }
                if let Err(err) = services::save_theme_mix(self.theme_mix) {
                    return self.error(err);
                }
                Task::none()
            }
            Message::Session(Session::Lock) => {
                let dismiss = self.dismiss();
                let locker =
                    Task::perform(services::lock(), |result| Message::Finished(result, None));
                dismiss.chain(locker)
            }
            Message::Session(Session::Suspend) => self.session_action(Session::Suspend),
            Message::Session(action) => {
                if self
                    .panel
                    .is_some_and(|(panel, _)| panel == Panel::Dashboard)
                {
                    let open = self.toggle(Panel::Session);
                    self.confirm = Some(action);
                    return open;
                }
                self.confirm = Some(action);
                Task::none()
            }
            Message::Confirm(action) if self.confirm == Some(action) => {
                self.confirm = None;
                self.session_action(action)
            }
            Message::Cancel => {
                self.confirm = None;
                Task::none()
            }
            Message::Finished(Ok(_), Some(label)) => {
                let close = self.dismiss();
                if label.is_empty() {
                    return close;
                }
                let notice = self.show_notice(Notice {
                    title: label.into(),
                    detail: String::new(),
                    level: None,
                });
                Task::batch([close, notice])
            }
            Message::Finished(Ok(_), None) => self.poll(),
            Message::Finished(Err(err), _) => self.error(err),
            Message::Expire(generation) if generation == self.notice_generation => self
                .notice
                .take()
                .map_or_else(Task::none, |(_, id)| window::close(id)),
            _ => Task::none(),
        }
    }

    /// Re-reads the small state files shared with the other d77 shells.
    fn refresh_files(&mut self) {
        self.active_wallpaper = services::active_wallpaper();
        // Do not let the tick overwrite an unsaved slider value.
        if !self.theme_mix_dirty {
            self.theme_mix = services::theme_mix();
        }
        self.window_counts = if self.native_workspaces.is_some() {
            services::native_window_counts()
        } else {
            None
        };
        // Keep the palette steady while a wallpaper job rewrites palette.json.
        if !self.wallpaper_busy {
            self.palette = ui::Palette::resolve(self.active_wallpaper.as_deref(), self.theme_mix);
        }
    }

    fn reveal_selection(&self, count: usize) -> Task<Message> {
        const VIEWPORT_ROWS: usize = 6;
        let extra = count.saturating_sub(VIEWPORT_ROWS);
        let target = self.selected.saturating_sub(VIEWPORT_ROWS / 2);
        let y = if extra == 0 {
            0.0
        } else {
            (target as f32 / extra as f32).min(1.0)
        };
        operation::snap_to("frost-app-list", operation::RelativeOffset { x: 0.0, y })
    }

    fn next_control(&mut self) -> Task<Message> {
        if self.adjusting {
            return Task::none();
        }
        let Some(control) = self.controls.pop_front() else {
            return Task::none();
        };
        self.adjusting = true;
        Task::perform(
            async move {
                services::adjust(control).await?;
                Ok(
                    if matches!(control, Control::BrightnessUp | Control::BrightnessDown) {
                        services::brightness().await
                    } else {
                        services::volume().await
                    },
                )
            },
            move |result| Message::Adjusted(control, result),
        )
    }

    fn next_wallpaper(&mut self) -> Task<Message> {
        if self.wallpaper_busy {
            return Task::none();
        }
        let Some(choice) = self.wallpaper_target.clone() else {
            return Task::none();
        };
        self.wallpaper_busy = true;
        Task::perform(
            async move {
                let result = match &choice {
                    WallpaperChoice::Image(path) => services::apply_wallpaper(path.clone()).await,
                    WallpaperChoice::Clear => services::clear_wallpaper().await,
                };
                (choice, result)
            },
            |(choice, result)| Message::WallpaperDone(choice, result),
        )
    }

    fn session_action(&self, action: Session) -> Task<Message> {
        Task::perform(services::session(action, self.compositor), |result| {
            Message::Finished(result, Some(""))
        })
    }

    fn next_level(&mut self, volume: bool) -> Task<Message> {
        if volume {
            if self.volume_setting {
                return Task::none();
            }
            let Some(value) = self.volume_target else {
                return Task::none();
            };
            self.volume_setting = true;
            Task::perform(services::set_volume(value), move |result| {
                Message::LevelSet(true, value, result)
            })
        } else {
            if self.brightness_setting {
                return Task::none();
            }
            let Some(value) = self.brightness_target else {
                return Task::none();
            };
            self.brightness_setting = true;
            Task::perform(services::set_brightness(value), move |result| {
                Message::LevelSet(false, value, result)
            })
        }
    }

    fn toggle(&mut self, panel: Panel) -> Task<Message> {
        let previous = self.panel.map(|(panel, _)| panel);
        let close = self.dismiss();
        if previous == Some(panel) {
            return close;
        }
        let id = Id::unique();
        let load = match panel {
            Panel::Launcher => {
                self.loading_apps = true;
                self.query.clear();
                self.selected = 0;
                Task::perform(
                    async {
                        tokio::task::spawn_blocking(services::apps)
                            .await
                            .unwrap_or_default()
                    },
                    move |apps| Message::AppsLoaded(id, apps),
                )
            }
            Panel::Wallpaper => {
                self.wallpaper_files = services::wallpapers();
                self.thumbnails = vec![None; self.wallpaper_files.len()];
                let files = self.wallpaper_files.clone();
                let previews =
                    stream::iter(files.into_iter().enumerate()).then(|(index, path)| async move {
                        let thumb = tokio::task::spawn_blocking(move || media::thumbnail(&path))
                            .await
                            .ok()
                            .flatten();
                        (index, thumb)
                    });
                let (job, handle) = Task::run(previews, move |(index, thumb)| {
                    Message::Thumbnail(id, index, thumb)
                })
                .abortable();
                self.thumbnail_job = Some(handle.abort_on_drop());
                job
            }
            _ => Task::none(),
        };
        self.panel = Some((panel, id));
        let (width, height, anchor) = panel.placement();
        let open = self.open_surface(
            id,
            (width, height),
            anchor,
            KeyboardInteractivity::Exclusive,
            false,
        );
        let poll = if panel == Panel::Dashboard {
            Task::batch([self.poll(), self.poll_weather()])
        } else {
            Task::none()
        };
        Task::batch([close.chain(open), poll, load])
    }

    fn dismiss(&mut self) -> Task<Message> {
        // The slider's release event never arrives if the panel closes mid-drag.
        if std::mem::take(&mut self.theme_mix_dirty)
            && let Err(err) = services::save_theme_mix(self.theme_mix)
        {
            eprintln!("frost: could not save the theme mix: {err}");
        }
        self.confirm = None;
        self.thumbnail_job = None;
        self.thumbnails.clear();
        self.panel
            .take()
            .map_or_else(Task::none, |(_, id)| window::close(id))
    }

    fn error(&mut self, error: String) -> Task<Message> {
        eprintln!("frost: {error}");
        self.show_notice(Notice {
            title: "Action failed".into(),
            detail: error,
            level: None,
        })
    }

    fn show_notice(&mut self, notice: Notice) -> Task<Message> {
        self.notice_generation = self.notice_generation.wrapping_add(1);
        let generation = self.notice_generation;
        let start = Instant::now();
        let timeout = Task::perform(
            async move {
                tokio::time::sleep_until((start + Duration::from_millis(2400)).into()).await;
                generation
            },
            Message::Expire,
        );
        if let Some((current, _)) = &mut self.notice {
            *current = notice;
            return timeout;
        }
        let id = Id::unique();
        self.notice = Some((notice, id));
        let open = self.open_surface(
            id,
            (360, 96),
            Anchor::Top,
            KeyboardInteractivity::None,
            true,
        );
        Task::batch([open, timeout])
    }

    fn open_surface(
        &self,
        id: Id,
        size: (u32, u32),
        anchor: Anchor,
        keyboard_interactivity: KeyboardInteractivity,
        notice: bool,
    ) -> Task<Message> {
        Task::done(Message::NewLayerShell {
            id,
            settings: NewLayerShellSettings {
                size: LayerSize::px(size.0, size.1),
                anchor,
                layer: Layer::Overlay,
                exclusive_zone: Some(0),
                margin: Some(Margin {
                    top: 9,
                    right: 12,
                    bottom: 0,
                    left: 12,
                }),
                keyboard_interactivity,
                output_option: OutputOption::LastOutput,
                events_transparent: notice,
                namespace: Some(if notice { "frost-osd" } else { "frost-panel" }.into()),
                ..Default::default()
            },
        })
    }

    fn reconcile_backdrops(&mut self) -> Task<Message> {
        if self.active_wallpaper.is_some() {
            return Task::batch(self.backdrops.drain().map(|(_, id)| window::close(id)));
        }
        let missing: Vec<_> = self
            .known_outputs
            .iter()
            .filter(|output| !self.backdrops.contains_key(output))
            .copied()
            .collect();
        Task::batch(missing.into_iter().map(|output| {
            let id = Id::unique();
            self.backdrops.insert(output, id);
            Task::done(Message::NewLayerShell {
                id,
                settings: NewLayerShellSettings {
                    size: LayerSize::FILL,
                    anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
                    layer: Layer::Bottom,
                    exclusive_zone: Some(-1),
                    keyboard_interactivity: KeyboardInteractivity::None,
                    output_option: OutputOption::GlobalName(output.0),
                    events_transparent: true,
                    namespace: Some("frost-backdrop".into()),
                    ..Default::default()
                },
            })
        }))
    }

    fn view(&self, id: Id) -> Element<'_, Message, Theme> {
        if let Some((panel, active)) = self.panel
            && active == id
        {
            self.panel_view(panel)
        } else if let Some((notice, active)) = &self.notice
            && *active == id
        {
            self.notice_view(notice)
        } else if self.backdrops.values().any(|active| *active == id) {
            self.backdrop_view()
        } else {
            self.bar_view(id)
        }
    }
}

#[cfg(test)]
mod wallpaper_tests {
    use super::*;

    #[test]
    fn rapid_selections_finish_with_the_last_wallpaper() {
        let (mut frost, _) = Frost::new(shell::channel().1);
        let first = WallpaperChoice::Image(PathBuf::from("first.png"));
        let last = WallpaperChoice::Image(PathBuf::from("last.png"));

        let _ = frost.update(Message::Wallpaper(PathBuf::from("first.png")));
        let _ = frost.update(Message::Wallpaper(PathBuf::from("last.png")));
        assert_eq!(frost.wallpaper_target, Some(last.clone()));
        assert!(frost.wallpaper_busy);

        let _ = frost.update(Message::WallpaperDone(first, Ok(String::new())));
        assert_eq!(frost.wallpaper_target, Some(last.clone()));
        assert!(frost.wallpaper_busy);

        let _ = frost.update(Message::WallpaperDone(last, Ok(String::new())));
        assert_eq!(frost.wallpaper_target, None);
        assert!(!frost.wallpaper_busy);
    }
}

#[cfg(test)]
mod level_tests {
    use super::*;

    fn read_at(started: Instant, percent: u16) -> Snapshot {
        Snapshot {
            volume: Some(Level {
                percent,
                muted: false,
            }),
            started: Some(started),
            ..Snapshot::default()
        }
    }

    #[test]
    fn stale_reads_do_not_override_a_slider_change_or_show_a_notice() {
        let (mut frost, _) = Frost::new(shell::channel().1);
        let _ = frost.update(Message::Snapshot(read_at(Instant::now(), 30)));
        let before_drag = Instant::now();

        let _ = frost.update(Message::SetVolume(70));
        // A read that began before the drag reports the old level.
        let _ = frost.update(Message::Snapshot(read_at(before_drag, 30)));
        assert_eq!(frost.snapshot.volume.map(|level| level.percent), Some(70));
        assert!(frost.notice.is_none());

        let _ = frost.update(Message::LevelSet(
            true,
            70,
            Ok(Some(Level {
                percent: 70,
                muted: false,
            })),
        ));
        // Still stale: it began before the change completed.
        let _ = frost.update(Message::Snapshot(read_at(before_drag, 30)));
        assert_eq!(frost.snapshot.volume.map(|level| level.percent), Some(70));
        assert!(frost.notice.is_none());

        // A fresh read with an external change is shown.
        let _ = frost.update(Message::Snapshot(read_at(Instant::now(), 45)));
        assert_eq!(frost.snapshot.volume.map(|level| level.percent), Some(45));
        assert!(frost.notice.is_some());
    }
}
