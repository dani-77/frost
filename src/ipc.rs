use iced::futures::{SinkExt, Stream, channel::mpsc};
use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::{self, ErrorKind},
    os::unix::net::UnixDatagram,
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
    let mut hash = DefaultHasher::new();
    display.hash(&mut hash);
    Ok(runtime.join(format!("frost-{:016x}.sock", hash.finish())))
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
