use iced::futures::{SinkExt, Stream, channel::mpsc};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::{self, ErrorKind},
    os::unix::{ffi::OsStrExt, net::UnixDatagram},
    path::PathBuf,
    sync::Arc,
};

#[derive(Debug, Clone)]
pub struct Socket {
    path: PathBuf,
    socket: Arc<UnixDatagram>,
    _lock: Arc<File>,
}

impl Hash for Socket {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
    }
}

pub(crate) fn path() -> io::Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other("XDG_RUNTIME_DIR must be an absolute path"))?;
    let display = std::env::var_os("WAYLAND_DISPLAY").unwrap_or_else(|| "wayland-0".into());
    Ok(runtime.join(format!("frost-{}.sock", display_key(&display))))
}

/// A file-name-safe key for a Wayland display that is identical across
/// builds, so a client and a daemon compiled separately agree on the path.
fn display_key(display: &OsStr) -> String {
    let bytes = display.as_bytes();
    let readable = !bytes.is_empty()
        && bytes.len() <= 48
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && bytes[0] != b'.';
    if readable {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    // FNV-1a: a fixed algorithm, unlike std's DefaultHasher.
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

impl Socket {
    pub fn bind() -> io::Result<Self> {
        let path = path()?;
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        lock.try_lock().map_err(|err| match err {
            std::fs::TryLockError::WouldBlock => {
                io::Error::other("Frost is already running on this display")
            }
            std::fs::TryLockError::Error(err) => err,
        })?;
        match fs::remove_file(&path) {
            Ok(()) => (),
            Err(err) if err.kind() == ErrorKind::NotFound => (),
            Err(err) => return Err(err),
        }
        let socket = UnixDatagram::bind(&path)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            path,
            socket: Arc::new(socket),
            _lock: Arc::new(lock),
        })
    }

    pub fn messages(&self) -> impl Stream<Item = crate::Message> + use<> {
        let socket = self.socket.clone();
        iced::stream::channel(32, async move |mut events: mpsc::Sender<_>| {
            let receiver = match socket
                .try_clone()
                .and_then(tokio::net::UnixDatagram::from_std)
            {
                Ok(receiver) => receiver,
                Err(err) => {
                    eprintln!("frost IPC: {err}");
                    return;
                }
            };
            let mut bytes = [0; 128];
            loop {
                let size = match receiver.recv(&mut bytes).await {
                    Ok(size) => size,
                    Err(err) => {
                        eprintln!("frost IPC: {err}");
                        break;
                    }
                };
                if let Some(message) = std::str::from_utf8(&bytes[..size])
                    .ok()
                    .and_then(crate::parse_command)
                    && events.send(message).await.is_err()
                {
                    break;
                }
            }
        })
    }
}

pub fn send(command: &str) -> io::Result<()> {
    UnixDatagram::unbound()?.send_to(command.as_bytes(), path()?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_keys_are_stable_and_file_name_safe() {
        assert_eq!(display_key(OsStr::new("wayland-1")), "wayland-1");
        assert_eq!(
            display_key(OsStr::new("/run/user/1000/wayland-1")),
            "2a61ea9b16ca481e"
        );
        assert_eq!(display_key(OsStr::new("..")).len(), 16);
    }
}
