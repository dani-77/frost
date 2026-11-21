use freedesktop_desktop_entry::{DesktopEntry, Iter, default_paths, get_languages_from_env};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Clone, Debug)]
pub struct App {
    pub name: String,
    pub detail: String,
    pub search: String,
    pub path: PathBuf,
    pub icon: Option<PathBuf>,
}

pub fn apps() -> Vec<App> {
    let theme = crate::media::icon_theme();
    let desktops = env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let desktops: Vec<_> = desktops.split(':').collect();
    let locales = get_languages_from_env();
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for path in Iter::new(default_paths()) {
        let Ok(entry) = DesktopEntry::from_path(path, Some(&locales)) else {
            continue;
        };
        if !seen.insert(entry.appid.clone())
            || entry.hidden()
            || entry.no_display()
            || entry.type_() != Some("Application")
            || (entry.exec().is_none() && !entry.dbus_activatable())
            || entry.try_exec().is_some_and(|program| !executable(program))
            || entry
                .only_show_in()
                .is_some_and(|list| !list.iter().any(|d| desktops.contains(d)))
            || entry
                .not_show_in()
                .is_some_and(|list| list.iter().any(|d| desktops.contains(d)))
        {
            continue;
        }
        let Some(name) = entry.name(&locales) else {
            continue;
        };
        let detail = entry
            .generic_name(&locales)
            .or_else(|| entry.comment(&locales))
            .unwrap_or_default();
        let search = format!(
            "{name} {detail} {} {}",
            entry.appid,
            entry.keywords(&locales).unwrap_or_default().join(" ")
        )
        .to_lowercase();
        apps.push(App {
            name: name.into_owned(),
            detail: detail.into_owned(),
            search,
            icon: entry
                .icon()
                .and_then(|name| crate::media::app_icon(name, &theme)),
            path: entry.path,
        });
    }
    apps.sort_by_cached_key(|app| app.name.to_lowercase());
    apps
}

fn executable(program: &str) -> bool {
    let is_executable = |path: &Path| {
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    };
    let path = Path::new(program);
    if path.is_absolute() {
        return is_executable(path);
    }
    env::var_os("PATH")
        .is_some_and(|value| env::split_paths(&value).any(|dir| is_executable(&dir.join(path))))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Compositor {
    Sway,
    Hyprland,
    Niri,
    Other,
}

impl Compositor {
    pub fn detect() -> Self {
        if env::var_os("NIRI_SOCKET").is_some() {
            Self::Niri
        } else if env::var_os("SWAYSOCK").is_some() {
            Self::Sway
        } else if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            Self::Hyprland
        } else {
            Self::Other
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Workspace {
    #[serde(default)]
    pub id: i64,
    pub num: Option<i64>,
    pub name: Option<String>,
    pub idx: Option<u32>,
    #[serde(default, alias = "is_focused", alias = "is_active")]
    pub focused: bool,
    #[serde(default)]
    pub urgent: bool,
    #[serde(default)]
    pub windows: Option<u32>,
    #[serde(default)]
    pub representation: Option<String>,
}

impl Workspace {
    pub fn occupied(&self, compositor: Compositor) -> bool {
        match compositor {
            Compositor::Sway => self.representation.is_some(),
            Compositor::Hyprland | Compositor::Niri => self.windows.is_none_or(|count| count > 0),
            Compositor::Other => true,
        }
    }
}

pub async fn workspaces(compositor: Compositor) -> Vec<Workspace> {
    let mut list: Vec<Workspace> = match compositor {
        Compositor::Sway => command("swaymsg", &["-r", "-t", "get_workspaces"]).await,
        Compositor::Niri => command("niri", &["msg", "--json", "workspaces"]).await,
        Compositor::Hyprland => command("hyprctl", &["-j", "workspaces"]).await,
        Compositor::Other => return Vec::new(),
    }
    .ok()
    .and_then(|out| serde_json::from_str(&out).ok())
    .unwrap_or_default();
    if compositor == Compositor::Hyprland {
        let focused = command("hyprctl", &["-j", "activeworkspace"])
            .await
            .ok()
            .and_then(|out| serde_json::from_str::<Workspace>(&out).ok());
        for ws in &mut list {
            ws.focused = focused.as_ref().is_some_and(|active| active.id == ws.id);
        }
    }
    if compositor == Compositor::Niri {
        if let Ok(out) = command("niri", &["msg", "--json", "windows"]).await
            && let Ok(windows) = serde_json::from_str::<Vec<serde_json::Value>>(&out)
        {
            for ws in &mut list {
                ws.windows = Some(
                    windows
                        .iter()
                        .filter(|window| {
                            window.get("workspace_id").and_then(|id| id.as_i64()) == Some(ws.id)
                        })
                        .count() as u32,
                );
                ws.urgent = windows.iter().any(|window| {
                    window.get("workspace_id").and_then(|id| id.as_i64()) == Some(ws.id)
                        && window.get("is_urgent").and_then(|value| value.as_bool()) == Some(true)
                });
            }
        }
    }
    list.retain(|ws| ws.id >= 0 || (compositor == Compositor::Sway && ws.name.is_some()));
    list.sort_by_key(|ws| (ws.num.or(ws.idx.map(i64::from)).unwrap_or(ws.id), ws.id));
    list
}

pub fn native_window_counts() -> Option<HashMap<String, u32>> {
    let runtime = env::var_os("XDG_RUNTIME_DIR")?;
    let display = env::var_os("WAYLAND_DISPLAY")?;
    let filename = format!("d77-workspaces-{}", display.to_string_lossy());
    let data = fs::read_to_string(PathBuf::from(runtime).join(filename)).ok()?;
    Some(
        data.lines()
            .filter_map(|line| {
                let (id, count) = line.split_once(' ')?;
                Some((id.to_owned(), count.trim().parse().ok()?))
            })
            .collect(),
    )
}

pub async fn switch(compositor: Compositor, ws: Workspace) -> Result<String, String> {
    let id = ws.id.to_string();
    match compositor {
        Compositor::Sway => {
            let name = ws
                .name
                .as_deref()
                .unwrap_or(&id)
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            command("swaymsg", &[&format!("workspace \"{name}\"")]).await
        }
        Compositor::Hyprland => command("hyprctl", &["dispatch", "workspace", &id]).await,
        Compositor::Niri => {
            let request = serde_json::json!({
                "Action": { "FocusWorkspace": { "reference": { "Id": ws.id } } }
            });
            niri_action(request).await
        }
        Compositor::Other => Ok(String::new()),
    }
}

async fn niri_action(request: serde_json::Value) -> Result<String, String> {
    let path = env::var_os("NIRI_SOCKET").ok_or("NIRI_SOCKET is missing")?;
    let response = tokio::time::timeout(Duration::from_secs(3), async {
        let mut socket = tokio::net::UnixStream::connect(PathBuf::from(path)).await?;
        socket.write_all(format!("{request}\n").as_bytes()).await?;
        let mut reply = String::new();
        BufReader::new(socket).read_line(&mut reply).await?;
        Ok::<_, std::io::Error>(reply)
    })
    .await
    .map_err(|_| "niri IPC timed out")?
    .map_err(|err| format!("niri IPC: {err}"))?;
    let result = serde_json::from_str::<Result<serde_json::Value, String>>(&response)
        .map_err(|err| format!("niri IPC: {err}"))?;
    result.map(|_| String::new())
}

pub async fn command(program: &str, args: &[&str]) -> Result<String, String> {
    command_with_timeout(program, args, Duration::from_secs(3)).await
}

async fn command_with_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let output = tokio::time::timeout(
        timeout,
        tokio::process::Command::new(program)
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| format!("{program} timed out"))?
    .map_err(|err| format!("{program}: {err}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        Err(format!(
            "{program}: {}",
            if detail.is_empty() {
                output.status.to_string()
            } else {
                detail.lines().next().unwrap_or_default().to_owned()
            }
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    VolumeUp,
    VolumeDown,
    VolumeMute,
    BrightnessUp,
    BrightnessDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level {
    pub percent: u16,
    pub muted: bool,
}

pub async fn volume() -> Option<Level> {
    let out = command("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .await
        .ok()?;
    let raw = out.split_whitespace().nth(1)?.parse::<f32>().ok()?;
    Some(Level {
        percent: (raw * 100.0).round() as u16,
        muted: out.contains("[MUTED]"),
    })
}

const BACKLIGHT: &str = "/sys/class/backlight";

/// The backlight Frost reads and passes to brightnessctl, so both agree on
/// the device when a laptop exposes several.
fn backlight_device() -> Option<String> {
    let mut names: Vec<_> = fs::read_dir(BACKLIGHT)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names.into_iter().next()
}

/// Reads the backlight from sysfs, without starting a process.
pub fn brightness() -> Option<Level> {
    brightness_at(&Path::new(BACKLIGHT).join(backlight_device()?))
}

fn brightness_at(path: &Path) -> Option<Level> {
    let value = |name| -> Option<u64> {
        fs::read_to_string(path.join(name))
            .ok()?
            .trim()
            .parse()
            .ok()
    };
    let max = value("max_brightness").filter(|max| *max > 0)?;
    let current = value("actual_brightness").or_else(|| value("brightness"))?;
    Some(Level {
        percent: ((current.min(max) * 100 + max / 2) / max) as u16,
        muted: false,
    })
}

pub async fn set_volume(percent: u16) -> Result<Option<Level>, String> {
    command(
        "wpctl",
        &[
            "set-volume",
            "-l",
            "1.0",
            "@DEFAULT_AUDIO_SINK@",
            &format!("{}%", percent.min(100)),
        ],
    )
    .await?;
    Ok(volume().await)
}

pub async fn set_brightness(percent: u16) -> Result<Option<Level>, String> {
    let device = backlight_device().ok_or("No backlight device")?;
    command(
        "brightnessctl",
        &["-d", &device, "set", &format!("{}%", percent.clamp(1, 100))],
    )
    .await?;
    Ok(brightness())
}

pub async fn adjust(control: Control) -> Result<String, String> {
    match control {
        Control::VolumeUp => {
            command(
                "wpctl",
                &["set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", "5%+"],
            )
            .await
        }
        Control::VolumeDown => {
            command("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", "5%-"]).await
        }
        Control::VolumeMute => {
            command("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]).await
        }
        Control::BrightnessUp | Control::BrightnessDown => {
            let device = backlight_device().ok_or("No backlight device")?;
            let args: &[&str] = if control == Control::BrightnessUp {
                &["-d", &device, "set", "+5%"]
            } else {
                &["-d", &device, "--min-value=1", "set", "5%-"]
            };
            command("brightnessctl", args).await
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Music {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
}

pub async fn music() -> Option<Music> {
    let out = cmus_remote(&["-Q"]).await.ok()?;
    let mut music = Music::default();
    let mut filename = "";
    for line in out.lines() {
        if let Some(value) = line.strip_prefix("tag title ") {
            music.title = value.to_owned();
        } else if let Some(value) = line.strip_prefix("tag artist ") {
            music.artist = value.to_owned();
        } else if let Some(value) = line.strip_prefix("tag album ") {
            music.album = value.to_owned();
        } else if let Some(value) = line.strip_prefix("file ") {
            filename = value;
        } else if line == "status playing" {
            music.playing = true;
        }
    }
    if music.title.is_empty() {
        music.title = Path::new(filename)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "No track loaded".into());
    }
    Some(music)
}

fn is_cmus(process: &Path) -> bool {
    fs::read_to_string(process.join("comm"))
        .ok()
        .as_deref()
        .map(str::trim)
        == Some("cmus")
}

/// HOME and XDG_RUNTIME_DIR of the running cmus, so cmus-remote finds its
/// socket. The process is cached and /proc is scanned only when it is gone.
fn cmus_environment() -> HashMap<String, String> {
    static CACHE: std::sync::Mutex<Option<(PathBuf, HashMap<String, String>)>> =
        std::sync::Mutex::new(None);
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((process, environment)) = cache.as_ref()
        && is_cmus(process)
    {
        return environment.clone();
    }
    *cache = None;
    let Ok(entries) = fs::read_dir("/proc") else {
        return HashMap::new();
    };
    let Some(process) = entries.flatten().map(|entry| entry.path()).find(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()))
            && is_cmus(path)
    }) else {
        return HashMap::new();
    };
    let mut result = HashMap::new();
    if let Ok(data) = fs::read(process.join("environ")) {
        for item in data.split(|byte| *byte == 0) {
            for key in ["HOME", "XDG_RUNTIME_DIR"] {
                let prefix = format!("{key}=");
                if let Some(value) = item.strip_prefix(prefix.as_bytes()) {
                    result.insert(key.into(), String::from_utf8_lossy(value).into_owned());
                }
            }
        }
    }
    *cache = Some((process, result.clone()));
    result
}

pub async fn cmus_remote(args: &[&str]) -> Result<String, String> {
    let mut command = tokio::process::Command::new("cmus-remote");
    command
        .args(args)
        .envs(cmus_environment())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(3), command.output())
        .await
        .map_err(|_| "cmus-remote timed out".to_string())?
        .map_err(|err| format!("cmus-remote: {err}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

pub async fn music_action(action: &str) -> Result<String, String> {
    match action {
        "start" => {
            let (program, args): (&str, &[&str]) = if executable("tmux") {
                ("tmux", &["new-session", "-d", "-s", "cmus", "cmus"])
            } else if executable("screen") {
                ("screen", &["-dmS", "cmus", "cmus"])
            } else {
                return Err("Install tmux or screen to start cmus".into());
            };
            command(program, args).await
        }
        "album-prev" | "album-next" => {
            let flag = if action == "album-next" { "-n" } else { "-r" };
            let initial = music().await.ok_or("No cmus session")?;
            cmus_remote(&[flag]).await?;
            if !initial.album.is_empty() {
                for _ in 1..300 {
                    let Some(current) = music().await else {
                        break;
                    };
                    if current.album != initial.album {
                        break;
                    }
                    cmus_remote(&[flag]).await?;
                }
            }
            Ok(String::new())
        }
        flag => cmus_remote(&[flag]).await,
    }
}

pub async fn weather(location: Option<&str>) -> Option<String> {
    let location = location.unwrap_or_default();
    let encoded: String = location
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect();
    let url = format!("https://wttr.in/{encoded}?format=3");
    let text = command_with_timeout(
        "curl",
        &["-fsS", "--max-time", "6", &url],
        Duration::from_secs(7),
    )
    .await
    .ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.chars().take(90).collect())
}

pub fn memory() -> Option<u16> {
    let data = fs::read_to_string("/proc/meminfo").ok()?;
    let value = |key: &str| {
        data.lines()
            .find_map(|line| line.strip_prefix(key))?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()
    };
    let total = value("MemTotal:")?;
    let available = value("MemAvailable:")?;
    (total > 0).then(|| ((total.saturating_sub(available) * 100) / total) as u16)
}

pub fn load() -> Option<String> {
    let data = fs::read_to_string("/proc/loadavg").ok()?;
    data.split_whitespace().next().map(str::to_owned)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryStatus {
    Charging,
    Discharging,
    Full,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub percent: u16,
    pub status: BatteryStatus,
    pub remaining_minutes: Option<u64>,
}

impl Battery {
    pub fn label(self, show_time: bool) -> String {
        if show_time && let Some(minutes) = self.remaining_minutes {
            return if minutes >= 60 {
                format!("{}h{:02}", minutes / 60, minutes % 60)
            } else {
                format!("{minutes}m")
            };
        }
        format!("{}%", self.percent)
    }
}

fn battery_value(path: &Path, name: &str) -> Option<u64> {
    fs::read_to_string(path.join(name))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn battery_remaining(path: &Path, status: BatteryStatus) -> Option<u64> {
    let direct = match status {
        BatteryStatus::Charging => "time_to_full_now",
        BatteryStatus::Discharging => "time_to_empty_now",
        _ => return None,
    };
    if let Some(seconds) = battery_value(path, direct).filter(|seconds| *seconds > 0) {
        return Some(seconds.div_ceil(60));
    }
    for (current, full, rate) in [
        ("energy_now", "energy_full", "power_now"),
        ("charge_now", "charge_full", "current_now"),
    ] {
        let Some(now) = battery_value(path, current) else {
            continue;
        };
        let Some(rate) = battery_value(path, rate).filter(|rate| *rate > 0) else {
            continue;
        };
        let remaining = if status == BatteryStatus::Charging {
            let Some(full) = battery_value(path, full) else {
                continue;
            };
            full.saturating_sub(now)
        } else {
            now
        };
        return Some(remaining.saturating_mul(60).div_ceil(rate));
    }
    None
}

fn battery_at(path: &Path) -> Option<Battery> {
    if fs::read_to_string(path.join("type")).ok()?.trim() != "Battery"
        || fs::read_to_string(path.join("scope")).is_ok_and(|scope| scope.trim() == "Device")
        || battery_value(path, "present") == Some(0)
    {
        return None;
    }
    let percent = battery_value(path, "capacity")?.min(100) as u16;
    let status = match fs::read_to_string(path.join("status")).ok()?.trim() {
        "Charging" => BatteryStatus::Charging,
        "Discharging" => BatteryStatus::Discharging,
        "Full" => BatteryStatus::Full,
        _ => BatteryStatus::Unknown,
    };
    Some(Battery {
        percent,
        status,
        remaining_minutes: battery_remaining(path, status),
    })
}

pub fn battery() -> Option<Battery> {
    let batteries = fs::read_dir("/sys/class/power_supply").ok()?;
    batteries
        .flatten()
        .find_map(|entry| battery_at(&entry.path()))
}

#[cfg(test)]
mod battery_tests {
    use super::*;

    #[test]
    fn reads_backlight_percent_from_sysfs() {
        let path = env::temp_dir().join(format!("frost-backlight-test-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("max_brightness"), "19393\n").unwrap();
        fs::write(path.join("brightness"), "9696\n").unwrap();
        assert_eq!(brightness_at(&path).map(|level| level.percent), Some(50));
        fs::write(path.join("actual_brightness"), "19393\n").unwrap();
        assert_eq!(brightness_at(&path).map(|level| level.percent), Some(100));
        fs::write(path.join("max_brightness"), "0\n").unwrap();
        assert!(brightness_at(&path).is_none());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn estimates_remaining_time_and_ignores_device_batteries() {
        let path = env::temp_dir().join(format!("frost-battery-test-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("type"), "Battery\n").unwrap();
        fs::write(path.join("capacity"), "40\n").unwrap();
        fs::write(path.join("status"), "Discharging\n").unwrap();
        fs::write(path.join("energy_now"), "2000000\n").unwrap();
        fs::write(path.join("power_now"), "1000000\n").unwrap();
        let battery = battery_at(&path).unwrap();
        assert_eq!(battery.remaining_minutes, Some(120));
        assert_eq!(battery.label(true), "2h00");

        fs::write(path.join("status"), "Charging\n").unwrap();
        fs::write(path.join("time_to_full_now"), "2700\n").unwrap();
        assert_eq!(battery_at(&path).unwrap().label(true), "45m");

        fs::write(path.join("scope"), "Device\n").unwrap();
        assert!(battery_at(&path).is_none());
        fs::remove_dir_all(path).unwrap();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    Lock,
    Suspend,
    Logout,
    Reboot,
    PowerOff,
}

impl Session {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Lock => "Lock",
            Self::Suspend => "Suspend",
            Self::Logout => "Log out",
            Self::Reboot => "Restart",
            Self::PowerOff => "Shut down",
        }
    }
}

pub async fn lock() -> Result<String, String> {
    // After a package upgrade the running binary is unlinked and current_exe
    // reports "/usr/bin/frost (deleted)"; /proc/self/exe still executes it.
    let proc_exe = Path::new("/proc/self/exe");
    let executable = if proc_exe.exists() {
        proc_exe.to_owned()
    } else {
        env::current_exe().map_err(|err| format!("could not locate Frost: {err}"))?
    };
    let output = tokio::process::Command::new(executable)
        .arg("lock")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|err| format!("could not launch Frost locker: {err}"))?;
    if output.status.success() {
        Ok(String::new())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr
            .lines()
            .last()
            .map_or_else(|| output.status.to_string(), str::to_owned))
    }
}

pub async fn session(action: Session, compositor: Compositor) -> Result<String, String> {
    match (action, compositor) {
        (Session::Lock, _) => lock().await,
        (Session::Suspend, _) => power("org.freedesktop.login1.Manager.Suspend").await,
        (Session::Reboot, _) => power("org.freedesktop.login1.Manager.Reboot").await,
        (Session::PowerOff, _) => power("org.freedesktop.login1.Manager.PowerOff").await,
        (Session::Logout, Compositor::Sway) => command("swaymsg", &["exit"]).await,
        (Session::Logout, Compositor::Hyprland) => command("hyprctl", &["dispatch", "exit"]).await,
        (Session::Logout, Compositor::Niri) => {
            command("niri", &["msg", "action", "quit", "--skip-confirmation"]).await
        }
        (Session::Logout, Compositor::Other) => {
            let id = env::var("XDG_SESSION_ID").unwrap_or_else(|_| "self".into());
            command("loginctl", &["terminate-session", &id]).await
        }
    }
}

async fn power(method: &str) -> Result<String, String> {
    command_with_timeout(
        "dbus-send",
        &[
            "--system",
            "--print-reply",
            "--reply-timeout=30000",
            "--dest=org.freedesktop.login1",
            "/org/freedesktop/login1",
            method,
            "boolean:true",
        ],
        Duration::from_secs(31),
    )
    .await
}

pub async fn launch(path: PathBuf) -> Result<String, String> {
    let locales = get_languages_from_env();
    if let Ok(entry) = DesktopEntry::from_path(&path, Some(&locales))
        && entry.terminal()
    {
        let argv = entry
            .parse_exec()
            .map_err(|err| format!("desktop Exec: {err}"))?;
        return spawn_in_terminal(&argv);
    }
    let status = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new("gio")
            .arg("launch")
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .map_err(|_| "gio launch timed out")?
    .map_err(|err| format!("gio: {err}"))?;
    if status.success() {
        Ok(String::new())
    } else {
        Err(format!("gio launch: {status}"))
    }
}

pub async fn open_nmtui() -> Result<String, String> {
    spawn_in_terminal(&["nmtui".into()])
}

fn spawn_in_terminal(argv: &[String]) -> Result<String, String> {
    let preferred = crate::config::Config::load()
        .terminal
        .filter(|name| executable(name))
        .or_else(|| env::var("TERMINAL").ok().filter(|name| executable(name)));
    let program = preferred
        .or_else(|| {
            [
                "xdg-terminal-exec",
                "alacritty",
                "kitty",
                "foot",
                "wezterm",
                "xterm",
            ]
            .into_iter()
            .find(|name| executable(name))
            .map(str::to_owned)
        })
        .ok_or("No terminal found (set TERMINAL or install xdg-terminal-exec)")?;
    let name = Path::new(&program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&program);
    let mut command = tokio::process::Command::new(&program);
    match name {
        "xdg-terminal-exec" => {}
        "wezterm" => {
            command.args(["start", "--"]);
        }
        "gnome-terminal" => {
            command.arg("--");
        }
        _ => {
            command.arg("-e");
        }
    }
    command
        .args(argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| format!("could not open {program}: {err}"))?;
    Ok(String::new())
}

pub fn wallpaper_dir() -> Option<PathBuf> {
    env::var_os("FROST_WALLPAPERS")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join("Wallpaper")))
}

fn cache_dir() -> Option<PathBuf> {
    env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
}

fn wallpaper_state() -> Option<PathBuf> {
    Some(cache_dir()?.join("frost/wallpaper/current"))
}

pub fn active_wallpaper() -> Option<PathBuf> {
    let cache = cache_dir()?;
    if let Ok(path) = fs::read_to_string(cache.join("frost/wallpaper/current")) {
        let path = PathBuf::from(path.trim());
        return path.is_file().then_some(path);
    }
    [
        "quickshell/wallpaper/current",
        "fabric-d77/wallpaper/current",
        "helium/wallpaper/current",
    ]
    .into_iter()
    .filter_map(|name| fs::read_to_string(cache.join(name)).ok())
    .map(|path| PathBuf::from(path.trim()))
    .find(|path| path.is_file())
}

pub fn theme_mix() -> f32 {
    let path = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    path.and_then(|path| fs::read_to_string(path.join("d77/theme-mix")).ok())
        .and_then(|value| value.trim().parse::<f32>().ok())
        .unwrap_or(1.0)
        .clamp(0.0, 1.0)
}

pub fn save_theme_mix(value: f32) -> Result<(), String> {
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or("Missing config directory")?;
    let dir = config.join("d77");
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let path = dir.join("theme-mix");
    let temporary = dir.join("theme-mix.tmp");
    fs::write(&temporary, format!("{:.2}\n", value.clamp(0.0, 1.0)))
        .map_err(|err| err.to_string())?;
    fs::rename(temporary, path).map_err(|err| err.to_string())
}

pub fn wallpapers() -> Vec<PathBuf> {
    let mut paths: Vec<_> = wallpaper_dir()
        .and_then(|dir| fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        matches!(
                            ext.to_ascii_lowercase().as_str(),
                            "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif"
                        )
                    })
        })
        .collect();
    paths.sort();
    paths
}

pub async fn apply_wallpaper(path: PathBuf) -> Result<String, String> {
    if !path.is_file() {
        return Err("Wallpaper file does not exist".into());
    }
    let path_str = path.to_str().ok_or("Wallpaper path must be valid UTF-8")?;
    let cli = if executable("awww") { "awww" } else { "swww" };
    command(cli, &["img", path_str]).await?;
    write_wallpaper_state(path_str)?;
    generate_palette(&path).await?;
    Ok(String::new())
}

fn write_wallpaper_state(value: &str) -> Result<(), String> {
    let state = wallpaper_state().ok_or("Missing cache directory")?;
    fs::create_dir_all(state.parent().unwrap()).map_err(|err| err.to_string())?;
    let next = state.with_extension("next");
    fs::write(&next, value).map_err(|err| err.to_string())?;
    fs::rename(next, state).map_err(|err| err.to_string())
}

pub async fn clear_wallpaper() -> Result<String, String> {
    let cli = if executable("awww") { "awww" } else { "swww" };
    command(cli, &["clear"]).await?;
    write_wallpaper_state("")?;
    Ok(String::new())
}

async fn generate_palette(path: &Path) -> Result<(), String> {
    if !executable("matugen") {
        return Ok(());
    }
    let cache = cache_dir().ok_or("Missing cache directory")?;
    let frost = cache.join("frost/matugen");
    let shared = cache.join("d77");
    fs::create_dir_all(&frost).map_err(|err| err.to_string())?;
    fs::create_dir_all(&shared).map_err(|err| err.to_string())?;
    let template = frost.join("d77-palette.json");
    let config = frost.join("matugen.toml");
    let output = frost.join("palette.json.next");
    let _ = fs::remove_file(&output);
    fs::write(&template, include_str!("../assets/d77-palette.json"))
        .map_err(|err| err.to_string())?;
    let toml = format!(
        "[config]\n[config.custom_colors]\ngreen = \"#9ece6a\"\nyellow = \"#e0af68\"\norange = \"#ff9e64\"\n[templates.d77]\ninput_path = {:?}\noutput_path = {:?}\n",
        template.display().to_string(),
        output.display().to_string()
    );
    fs::write(&config, toml).map_err(|err| err.to_string())?;
    let status = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new("matugen")
            .kill_on_drop(true)
            .arg("-c")
            .arg(&config)
            .arg("image")
            .arg(path)
            .args([
                "--source-color-index",
                "0",
                "-t",
                "scheme-vibrant",
                "-m",
                "dark",
                "-q",
            ])
            .status(),
    )
    .await;
    match status {
        Ok(Ok(status)) if status.success() => {
            fs::rename(output, shared.join("palette.json")).map_err(|err| err.to_string())
        }
        Ok(Ok(status)) => Err(format!("Matugen exited with {status}")),
        Ok(Err(err)) => Err(format!("Could not start Matugen: {err}")),
        Err(_) => Err("Matugen timed out".into()),
    }
}
