//! Wayland session lock, adapted from helium-locker (MIT, Daniel Azevedo).
//! Iced draws the view into shared-memory buffers. SCTK owns the lock protocol.

mod auth;
mod secret;
mod state;
mod system;
mod view;

use std::fmt;
use std::fs::OpenOptions;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Duration;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    output::{OutputHandler, OutputState},
    reexports::{calloop::EventLoop, calloop_wayland_source::WaylandSource},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
    },
    session_lock::{
        SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface,
        SessionLockSurfaceConfigure,
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface},
};

use self::auth::{AuthOutcome, AuthWorker, PamProvider, SubmitError};
use self::secret::SecretBuffer;
use self::state::{LockEvent, LockMachine, LockState};

const PAM_SERVICE: &str = "frost-locker";
const PAM_SERVICE_FILE: &str = "/etc/pam.d/frost-locker";

/// How long one event-loop dispatch may block. Bounds the latency of PAM
/// results and of the clock, which are polled rather than event-driven.
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Refuse absurd configure sizes instead of trying to allocate them.
const MAX_SURFACE_DIMENSION: u32 = 16_384;
const MAX_BUFFER_SCALE: i32 = 8;

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Path::new(PAM_SERVICE_FILE)
        .metadata()
        .map_err(|err| format!("PAM configuration {PAM_SERVICE_FILE}: {err}"))?;
    if !policy.is_file() || policy.uid() != 0 || policy.mode() & 0o022 != 0 {
        return Err(format!("{PAM_SERVICE_FILE} must be a root-owned PAM file with no group or world write permissions").into());
    }

    // Only the standalone locker process is hardened. The shell can restart
    // independently while the compositor still holds the session lock.
    system::harden_process()?;
    let username = system::current_username()?;
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(crate::ipc::path()?.with_extension("locker"))?;
    lock.try_lock().map_err(|err| match err {
        std::fs::TryLockError::WouldBlock => {
            std::io::Error::other("Frost is already locking this display")
        }
        std::fs::TryLockError::Error(err) => err,
    })?;

    // Keep the advisory lock alive until the Wayland connection is closed.
    let result = run_wayland(&username);
    drop(lock);
    result.map_err(Into::into)
}

#[derive(Debug)]
enum LockerError {
    Connect(String),
    Registry(String),
    MissingGlobal(&'static str),
    EventLoop(String),
    EventSource(String),
    ConnectionLost(String),
    NoKeyboard,
    LockProtocolUnavailable,
    LockRefused,
    ProtocolState,
    SharedMemory(String),
    AuthenticationWorker(String),
    AuthenticationStopped,
}

impl fmt::Display for LockerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "could not connect to the Wayland display: {e}"),
            Self::Registry(e) => write!(f, "could not read the Wayland registry: {e}"),
            Self::MissingGlobal(name) => write!(f, "compositor does not provide {name}"),
            Self::EventLoop(e) => write!(f, "could not create the event loop: {e}"),
            Self::EventSource(e) => write!(f, "could not watch the Wayland connection: {e}"),
            Self::ConnectionLost(e) => write!(f, "lost the Wayland connection: {e}"),
            Self::NoKeyboard => f.write_str("refusing to lock: no keyboard is available"),
            Self::LockProtocolUnavailable => {
                f.write_str("compositor does not support ext-session-lock-v1")
            }
            Self::LockRefused => f.write_str("compositor refused the session lock"),
            Self::ProtocolState => f.write_str(
                "session lock ended unexpectedly; the compositor keeps the session locked",
            ),
            Self::SharedMemory(e) => f.write_str(e),
            Self::AuthenticationWorker(e) => {
                write!(f, "could not start the authentication worker: {e}")
            }
            Self::AuthenticationStopped => {
                f.write_str("authentication worker stopped while the session was locked")
            }
        }
    }
}

impl std::error::Error for LockerError {}

fn run_wayland(username: &str) -> Result<(), LockerError> {
    let mut view = view::ViewState::default();
    view.refresh_clock();
    let palette = crate::ui::Palette::load();
    let renderer = view::new_renderer();

    let conn = Connection::connect_to_env().map_err(|e| LockerError::Connect(e.to_string()))?;
    let (globals, mut event_queue) =
        registry_queue_init(&conn).map_err(|e| LockerError::Registry(e.to_string()))?;
    let qh: QueueHandle<Locker> = event_queue.handle();

    let compositor_state = CompositorState::bind(&globals, &qh)
        .map_err(|_| LockerError::MissingGlobal("wl_compositor"))?;
    let shm = Shm::bind(&globals, &qh).map_err(|_| LockerError::MissingGlobal("wl_shm"))?;
    let pool = SlotPool::new(4 * 1024 * 1024, &shm)
        .map_err(|e| LockerError::SharedMemory(format!("could not allocate shared memory: {e}")))?;
    let session_lock_state = SessionLockState::new(&globals, &qh);
    let auth = AuthWorker::spawn(PamProvider::new(PAM_SERVICE, username))
        .map_err(|e| LockerError::AuthenticationWorker(e.to_string()))?;
    let secret =
        SecretBuffer::standard().map_err(|e| LockerError::AuthenticationWorker(e.to_string()))?;
    if !secret.is_memory_locked() {
        eprintln!("frost-locker: warning: password buffer could not be locked against swap");
    }
    let mut event_loop: EventLoop<Locker> =
        EventLoop::try_new().map_err(|e| LockerError::EventLoop(e.to_string()))?;

    let mut app = Locker {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor_state,
        shm,
        pool,
        session_lock_state,
        session_lock: None,
        renderer,
        palette,
        surfaces: Vec::new(),
        keyboards: Vec::new(),
        modifiers: Modifiers::default(),
        machine: LockMachine::new(),
        auth,
        active_attempt: None,
        secret,
        view,
        exit: None,
    };

    // Step 2: bind keyboards, then make the get_keyboard requests visible to
    // the compositor before the lock request (second round-trip).
    event_queue
        .roundtrip(&mut app)
        .map_err(|e| LockerError::ConnectionLost(e.to_string()))?;
    if app.keyboards.is_empty() {
        return Err(LockerError::NoKeyboard);
    }
    event_queue
        .roundtrip(&mut app)
        .map_err(|e| LockerError::ConnectionLost(e.to_string()))?;

    // Step 3: lock, round-trip, then create the surfaces.
    app.machine
        .apply(LockEvent::RequestLock)
        .map_err(|_| LockerError::ProtocolState)?;
    let session_lock = match app.session_lock_state.lock(&qh) {
        Ok(lock) => lock,
        Err(_) => {
            let _ = app.machine.apply(LockEvent::LockRefused);
            return Err(LockerError::LockProtocolUnavailable);
        }
    };
    app.session_lock = Some(session_lock);

    // Some compositors confirm the lock right away, others only once the
    // surfaces have been committed, so both states are fine here.
    event_queue
        .roundtrip(&mut app)
        .map_err(|e| LockerError::ConnectionLost(e.to_string()))?;
    if let Some(exit) = app.exit.take() {
        return exit;
    }
    if app.machine.state() != LockState::LockRequested && !app.machine.is_locked() {
        return Err(LockerError::ProtocolState);
    }

    let outputs: Vec<_> = app.output_state.outputs().collect();
    for output in outputs {
        app.add_output(&qh, output);
    }

    WaylandSource::new(conn.clone(), event_queue)
        .insert(event_loop.handle())
        .map_err(|e| LockerError::EventSource(e.to_string()))?;

    loop {
        event_loop
            .dispatch(EVENT_POLL_INTERVAL, &mut app)
            .map_err(|e| LockerError::ConnectionLost(e.to_string()))?;
        if let Some(exit) = app.exit.take() {
            return exit;
        }
        app.poll_authentication(&conn)?;
        if let Some(exit) = app.exit.take() {
            return exit;
        }
        if app.view.refresh_clock() {
            app.sync_view();
        }
        app.render_all();
    }
}

struct LockedOutput {
    output: wl_output::WlOutput,
    // Retaining the protocol object keeps this output covered.
    surface: SessionLockSurface,
    dirty: bool,
    /// Logical size from the last configure; None until the first one.
    configured_size: Option<(u32, u32)>,
    scale: i32,
    /// Keeps the last committed buffer alive until the compositor releases it.
    buffer: Option<Buffer>,
}

struct Locker {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor_state: CompositorState,
    shm: Shm,
    pool: SlotPool,
    session_lock_state: SessionLockState,
    session_lock: Option<SessionLock>,
    renderer: iced_tiny_skia::Renderer,
    palette: crate::ui::Palette,
    surfaces: Vec<LockedOutput>,
    keyboards: Vec<(wl_seat::WlSeat, wl_keyboard::WlKeyboard)>,
    modifiers: Modifiers,
    machine: LockMachine,
    auth: AuthWorker,
    active_attempt: Option<u64>,
    secret: SecretBuffer,
    view: view::ViewState,
    exit: Option<Result<(), LockerError>>,
}

impl Locker {
    fn add_output(&mut self, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if self.surfaces.iter().any(|entry| entry.output == output) {
            return;
        }
        let Some(session_lock) = self.session_lock.as_ref() else {
            return;
        };

        let scale = self
            .output_state
            .info(&output)
            .map_or(1, |info| info.scale_factor)
            .clamp(1, MAX_BUFFER_SCALE);

        let wl_surface = self.compositor_state.create_surface(qh);
        let surface = session_lock.create_lock_surface(wl_surface, &output, qh);
        self.surfaces.push(LockedOutput {
            output,
            surface,
            dirty: true,
            configured_size: None,
            scale,
            buffer: None,
        });
    }

    fn sync_view(&mut self) {
        for entry in &mut self.surfaces {
            entry.dirty = true;
        }
    }

    fn render_all(&mut self) {
        for index in 0..self.surfaces.len() {
            if let Err(error) = self.render(index) {
                eprintln!("frost-locker: could not draw the lock screen: {error}");
                if self.machine.state() == LockState::LockRequested {
                    // The compositor waits for a frame before confirming the
                    // lock; failing to produce one must not look like success.
                    self.exit = Some(Err(LockerError::ProtocolState));
                }
            }
        }
    }

    fn render(&mut self, index: usize) -> Result<(), String> {
        let pool = &mut self.pool;
        let entry = &mut self.surfaces[index];
        let Some((width, height)) = entry.configured_size else {
            return Ok(());
        };
        if !entry.dirty {
            return Ok(());
        }
        entry.dirty = false;
        let scale = entry.scale as u32;
        let (physical_width, physical_height) = (width * scale, height * scale);
        if u64::from(physical_width) * u64::from(physical_height) > 8192 * 4608 {
            return Err("lock surface exceeds the renderer's pixel limit".into());
        }
        let (buffer_width, buffer_height) = (physical_width as i32, physical_height as i32);
        let stride = buffer_width * 4;
        let (buffer, canvas) = pool
            .create_buffer(
                buffer_width,
                buffer_height,
                stride,
                wl_shm::Format::Xrgb8888,
            )
            .map_err(|error| format!("could not allocate a frame buffer: {error}"))?;
        let pixels = view::draw(
            &mut self.renderer,
            &self.view,
            self.palette,
            width,
            height,
            scale,
        );
        let frame_bytes = stride as usize * buffer_height as usize;
        view::copy_xrgb(&pixels, canvas, frame_bytes)?;
        let wl_surface = entry.surface.wl_surface();
        buffer
            .attach_to(wl_surface)
            .map_err(|error| format!("could not attach the frame buffer: {error:?}"))?;
        wl_surface.set_buffer_scale(entry.scale);
        wl_surface.damage_buffer(0, 0, buffer_width, buffer_height);
        wl_surface.commit();
        entry.buffer = Some(buffer);
        Ok(())
    }

    fn handle_key(&mut self, event: KeyEvent) {
        let input = zeroize::Zeroizing::new(event.utf8);
        if self.machine.state() != LockState::Locked {
            return;
        }

        let mut edited = false;
        match event.keysym {
            Keysym::Escape => {
                self.secret.clear();
                edited = true;
            }
            Keysym::BackSpace => {
                self.secret.pop_char();
                edited = true;
            }
            Keysym::Return | Keysym::KP_Enter => {
                if !self.secret.is_empty() {
                    self.submit_authentication();
                }
                return;
            }
            Keysym::u if self.modifiers.ctrl => {
                self.secret.clear();
                edited = true;
            }
            _ if !self.modifiers.ctrl && !self.modifiers.alt && !self.modifiers.logo => {
                if let Some(text) = input.as_deref()
                    && !text.is_empty()
                    && text.chars().all(|character| !character.is_control())
                    && self.secret.push_str(text).is_ok()
                {
                    edited = true;
                }
            }
            _ => {}
        }

        if edited {
            self.view.password_length = self.secret_char_count();
            self.view.show_error = false;
            self.sync_view();
        }
    }

    fn secret_char_count(&self) -> usize {
        std::str::from_utf8(self.secret.as_bytes()).map_or(0, |text| text.chars().count())
    }

    fn submit_authentication(&mut self) {
        let Ok(replacement) = SecretBuffer::standard() else {
            return;
        };
        if !replacement.is_memory_locked() {
            eprintln!(
                "frost-locker: warning: replacement password buffer could not be locked against swap"
            );
        }
        let submitted = std::mem::replace(&mut self.secret, replacement);
        match self.auth.submit(submitted) {
            Ok(attempt) => {
                if self.machine.apply(LockEvent::BeginAuthentication).is_err() {
                    self.exit = Some(Err(LockerError::ProtocolState));
                    return;
                }
                self.active_attempt = Some(attempt);
                self.view.checking = true;
                self.view.show_error = false;
                self.view.password_length = 0;
                self.sync_view();
            }
            Err(SubmitError::Busy(secret)) => self.secret = secret,
            Err(SubmitError::Stopped(secret)) => {
                self.secret = secret;
                self.exit = Some(Err(LockerError::AuthenticationStopped));
            }
        }
    }

    fn poll_authentication(&mut self, conn: &Connection) -> Result<(), LockerError> {
        let response = self
            .auth
            .try_response()
            .map_err(|_| LockerError::AuthenticationStopped)?;
        let Some(response) = response else {
            return Ok(());
        };
        if self.active_attempt.take() != Some(response.attempt) {
            return Err(LockerError::ProtocolState);
        }

        match response.outcome {
            AuthOutcome::Success => {
                self.machine
                    .apply(LockEvent::AuthenticationSucceeded)
                    .map_err(|_| LockerError::ProtocolState)?;
                self.machine
                    .apply(LockEvent::BeginUnlock)
                    .map_err(|_| LockerError::ProtocolState)?;
                self.session_lock
                    .as_ref()
                    .ok_or(LockerError::ProtocolState)?
                    .unlock();
                // Make sure unlock_and_destroy has reached the compositor
                // before the process (and its connection) goes away.
                conn.roundtrip()
                    .map_err(|e| LockerError::ConnectionLost(e.to_string()))?;
                self.machine
                    .apply(LockEvent::UnlockCompleted)
                    .map_err(|_| LockerError::ProtocolState)?;
                self.exit = Some(Ok(()));
            }
            AuthOutcome::Denied { stage, code } | AuthOutcome::Error { stage, code } => {
                eprintln!("frost-locker: authentication failed ({stage:?}: {code:?})");
                self.machine
                    .apply(LockEvent::AuthenticationFailed)
                    .map_err(|_| LockerError::ProtocolState)?;
                self.view.checking = false;
                self.view.show_error = true;
                self.sync_view();
            }
        }
        Ok(())
    }

    fn remove_keyboard(&mut self, seat: &wl_seat::WlSeat) {
        if let Some(index) = self.keyboards.iter().position(|(known, _)| known == seat) {
            let (_, keyboard) = self.keyboards.swap_remove(index);
            keyboard.release();
        }
    }
}

impl SessionLockHandler for Locker {
    fn locked(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _lock: SessionLock) {
        if self.machine.apply(LockEvent::LockConfirmed).is_err() {
            self.exit = Some(Err(LockerError::ProtocolState));
        }
    }

    fn finished(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _lock: SessionLock) {
        if self.machine.state() == LockState::LockRequested {
            let _ = self.machine.apply(LockEvent::LockRefused);
            self.exit = Some(Err(LockerError::LockRefused));
        } else {
            // Never translate a compositor-side termination into an unlock.
            self.exit = Some(Err(LockerError::ProtocolState));
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _serial: u32,
    ) {
        let (width, height) = configure.new_size;
        let Some(index) = self
            .surfaces
            .iter()
            .position(|entry| entry.surface.wl_surface() == surface.wl_surface())
        else {
            return;
        };
        if width == 0
            || height == 0
            || width > MAX_SURFACE_DIMENSION
            || height > MAX_SURFACE_DIMENSION
        {
            eprintln!("frost-locker: ignoring invalid lock surface size {width}x{height}");
            if self.machine.state() == LockState::LockRequested {
                self.exit = Some(Err(LockerError::ProtocolState));
            }
            return;
        }
        self.surfaces[index].configured_size = Some((width, height));
        self.surfaces[index].dirty = true;
        // Answer the configure with a frame right away (sctk has already
        // acked it); the compositor confirms the lock once every output has
        // a committed buffer.
        if let Err(error) = self.render(index) {
            eprintln!("frost-locker: could not draw the lock screen: {error}");
            if self.machine.state() == LockState::LockRequested {
                self.exit = Some(Err(LockerError::ProtocolState));
            }
        }
    }
}

impl CompositorHandler for Locker {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let Some(index) = self
            .surfaces
            .iter()
            .position(|entry| entry.surface.wl_surface() == surface)
        else {
            return;
        };
        let scale = new_factor.clamp(1, MAX_BUFFER_SCALE);
        if self.surfaces[index].scale == scale {
            return;
        }
        self.surfaces[index].scale = scale;
        self.surfaces[index].dirty = true;
        if let Err(error) = self.render(index) {
            eprintln!("frost-locker: {error}");
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Locker {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        // Hotplugged monitors must be covered too.
        self.add_output(qh, output);
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.surfaces.retain(|entry| entry.output != output);
    }
}

impl SeatHandler for Locker {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && !self.keyboards.iter().any(|(known, _)| *known == seat)
            && let Ok(keyboard) = self.seat_state.get_keyboard(qh, &seat, None)
        {
            self.keyboards.push((seat, keyboard));
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            self.remove_keyboard(&seat);
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        self.remove_keyboard(&seat);
    }
}

impl KeyboardHandler for Locker {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.handle_key(event);
    }

    fn repeat_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.handle_key(event);
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _raw_modifiers: RawModifiers,
        _layout: u32,
    ) {
        self.modifiers = modifiers;
        if self.view.caps_lock != modifiers.caps_lock {
            self.view.caps_lock = modifiers.caps_lock;
            self.sync_view();
        }
    }
}

impl ShmHandler for Locker {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Locker {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_compositor!(Locker);
smithay_client_toolkit::delegate_output!(Locker);
smithay_client_toolkit::delegate_seat!(Locker);
smithay_client_toolkit::delegate_keyboard!(Locker);
smithay_client_toolkit::delegate_session_lock!(Locker);
smithay_client_toolkit::delegate_shm!(Locker);
smithay_client_toolkit::delegate_registry!(Locker);
