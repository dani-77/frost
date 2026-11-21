//! Event sources that tell the shell when to re-read compositor and audio
//! state. When a source is missing or stops, they fall back to polling at
//! the old one-second rate and retry the source later, so behaviour is never
//! worse than plain polling.

use crate::services::Compositor;
use iced::futures::{SinkExt, Stream, channel::mpsc};
use std::{env, path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};

const FALLBACK_INTERVAL: Duration = Duration::from_secs(1);
/// How long to poll before trying an event source again.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Events arriving this close together produce one refresh.
const COALESCE: Duration = Duration::from_millis(40);

enum Source {
    Command(&'static str, &'static [&'static str]),
    Socket(PathBuf),
}

struct Closed;

/// Signals whenever the compositor's workspaces may have changed.
pub fn workspaces(compositor: &Compositor) -> impl Stream<Item = ()> + use<> {
    let (source, relevant): (_, fn(&str) -> bool) = match compositor {
        Compositor::Sway => (
            Some(Source::Command(
                "swaymsg",
                &["-r", "-t", "subscribe", "-m", r#"["workspace","window"]"#],
            )),
            sway_event,
        ),
        Compositor::Niri => (
            Some(Source::Command("niri", &["msg", "--json", "event-stream"])),
            niri_event,
        ),
        Compositor::Hyprland => (hyprland_socket().map(Source::Socket), hyprland_event),
        Compositor::Other => (None, |_| false),
    };
    watch(source, relevant)
}

/// Signals whenever the default sink's volume or mute state may have changed.
pub fn volume() -> impl Stream<Item = ()> {
    watch(Some(Source::Command("pactl", &["subscribe"])), pulse_event)
}

fn watch(source: Option<Source>, relevant: fn(&str) -> bool) -> impl Stream<Item = ()> {
    iced::stream::channel(1, async move |mut out: mpsc::Sender<()>| {
        loop {
            if let Some(source) = &source
                && follow(source, relevant, &mut out).await.is_err()
            {
                return;
            }
            // The source is unavailable or ended: poll, then try again.
            let mut ticks = tokio::time::interval(FALLBACK_INTERVAL);
            let deadline = tokio::time::Instant::now() + RETRY_AFTER;
            while source.is_none() || tokio::time::Instant::now() < deadline {
                ticks.tick().await;
                if out.send(()).await.is_err() {
                    return;
                }
            }
        }
    })
}

async fn follow(
    source: &Source,
    relevant: fn(&str) -> bool,
    out: &mut mpsc::Sender<()>,
) -> Result<(), Closed> {
    // The child, if any, is killed when this function returns.
    let (reader, _child): (Box<dyn AsyncBufRead + Unpin + Send>, _) = match source {
        Source::Command(program, args) => {
            let Ok(mut child) = tokio::process::Command::new(program)
                .args(*args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
            else {
                return Ok(());
            };
            let Some(stdout) = child.stdout.take() else {
                return Ok(());
            };
            (Box::new(BufReader::new(stdout)), Some(child))
        }
        Source::Socket(path) => {
            let Ok(socket) = tokio::net::UnixStream::connect(path).await else {
                return Ok(());
            };
            (Box::new(BufReader::new(socket)), None)
        }
    };
    // State may have changed while nothing was watching.
    out.send(()).await.map_err(|_| Closed)?;
    let mut lines = reader.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if !relevant(&line) {
            continue;
        }
        // Swallow the rest of a burst; the refresh below covers it.
        let _ = tokio::time::timeout(COALESCE, async {
            while let Ok(Some(_)) = lines.next_line().await {}
        })
        .await;
        out.send(()).await.map_err(|_| Closed)?;
    }
    Ok(())
}

fn hyprland_socket() -> Option<PathBuf> {
    let signature = env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let runtime = env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join("hypr"));
    runtime
        .into_iter()
        .chain([PathBuf::from("/tmp/hypr")])
        .map(|dir| dir.join(&signature).join(".socket2.sock"))
        .find(|path| path.exists())
}

fn hyprland_event(line: &str) -> bool {
    let name = line.split_once(">>").map_or(line, |(name, _)| name);
    matches!(
        name.trim_end_matches("v2"),
        "workspace"
            | "focusedmon"
            | "createworkspace"
            | "destroyworkspace"
            | "moveworkspace"
            | "renameworkspace"
            | "activespecial"
            | "openwindow"
            | "closewindow"
            | "movewindow"
            | "urgent"
    )
}

fn sway_event(line: &str) -> bool {
    let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
        return true;
    };
    // Window focus and title changes are frequent and do not change workspaces.
    let window = event.get("container").is_some();
    let change = event.get("change").and_then(|value| value.as_str());
    !(window && matches!(change, Some("focus" | "title" | "mark")))
}

fn niri_event(line: &str) -> bool {
    [
        "Workspace",
        "WindowOpened",
        "WindowClosed",
        "WindowUrgency",
        "WindowsChanged",
    ]
    .iter()
    .any(|name| line.contains(name))
}

fn pulse_event(line: &str) -> bool {
    // "Event 'change' on sink #56"; "server" covers a new default sink.
    matches!(line.split_whitespace().nth(3), Some("sink" | "server"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_keep_workspace_and_volume_changes() {
        assert!(hyprland_event("workspacev2>>2,2"));
        assert!(hyprland_event("urgent>>5a3f"));
        assert!(!hyprland_event("activewindow>>kitty,~"));
        assert!(sway_event(r#"{"change":"focus","current":{},"old":{}}"#));
        assert!(!sway_event(r#"{"change":"title","container":{}}"#));
        assert!(sway_event(r#"{"change":"urgent","container":{}}"#));
        assert!(niri_event(
            r#"{"WorkspaceActivated":{"id":2,"focused":true}}"#
        ));
        assert!(!niri_event(r#"{"WindowFocusChanged":{"id":7}}"#));
        assert!(pulse_event("Event 'change' on sink #56"));
        assert!(pulse_event("Event 'change' on server #0"));
        assert!(!pulse_event("Event 'change' on sink-input #91"));
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use iced::futures::StreamExt;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn bursts_coalesce_into_one_refresh() {
        runtime().block_on(async {
            let source = Source::Command(
                "sh",
                &["-c", "echo a; echo b; echo c; sleep 0.3; echo d; sleep 5"],
            );
            let stream = watch(Some(source), |_| true);
            let start = tokio::time::Instant::now();
            let events: Vec<_> = stream
                .take(3)
                .map(|()| start.elapsed())
                .collect()
                .await;
            // Initial refresh, one for the a/b/c burst, one for d.
            assert!(events[1] < Duration::from_millis(250), "{events:?}");
            assert!(events[2] >= Duration::from_millis(250), "{events:?}");
        });
    }

    #[test]
    fn a_missing_program_falls_back_to_polling() {
        runtime().block_on(async {
            let source = Source::Command("frost-no-such-watcher", &[]);
            let start = tokio::time::Instant::now();
            watch(Some(source), |_| true).take(2).collect::<Vec<_>>().await;
            let elapsed = start.elapsed();
            assert!(elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(3));
        });
    }
}
