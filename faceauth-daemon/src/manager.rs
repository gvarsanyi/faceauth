//! The `io.github.gvarsanyi.FaceAuth1.Manager` D-Bus interface.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use faceauth_common::config::{Config, ENROLL_CAPTURES, ENROLL_CAPTURE_TIMEOUT_SECS};
use faceauth_common::dbus::{CameraTuple, Error, Result, ENROLLED_CAMERA, OBJECT_PATH};
use faceauth_common::{is_valid_service_name, logind, users, v4l};
use faceauth_core::matcher::{AuthParams, EnrollParams};
use faceauth_core::store::{service_states, FaceModel, Store};
use faceauth_core::{Encoding, FaceAuthError};
use zbus::message::{Flags, Header};
use zbus::names::{BusName, UniqueName};
use zbus::object_server::SignalEmitter;
use zbus::{fdo::DBusProxy, interface, Connection};

use crate::authz::{check_unlock_only, decide, Decision, Op};
use crate::backoff::Backoff;
use crate::camera_actor::{Job, Progress};
use crate::polkit;

/// Pause between processed frames; bounds CPU use while the camera is on.
const FRAME_INTERVAL: Duration = Duration::from_millis(50);
/// Captures closer than this are rejected so a batch covers several poses.
const MIN_POSE_DISTANCE: f64 = 0.3;
/// Notification agents one user may register at a time.
const MAX_AGENTS_PER_USER: usize = 8;

/// Registered notification agents: unique bus name -> UID.
pub type Agents = Arc<Mutex<HashMap<String, u32>>>;
/// Locked sessions (logind object path) -> whether face unlock is armed:
/// since the lock, the camera has seen nobody for `LOCK_ABSENCE` (see
/// `matcher::authenticate`).
pub type Locks = Arc<Mutex<HashMap<String, bool>>>;
/// Time without a face in view that re-arms face unlock after a lock.
const LOCK_ABSENCE: Duration = Duration::from_secs(2);

/// Which bus the daemon serves. On the session bus (development only) the
/// daemon runs unprivileged and skips polkit, which only knows the system bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusKind {
    System,
    Session,
}

/// Verified identity of a D-Bus caller.
struct Caller {
    sender: String,
    uid: u32,
    pid: Option<u32>,
    interactive: bool,
}

pub struct Manager {
    bus: BusKind,
    store: Store,
    /// Serializes read-modify-write cycles on the state directory.
    store_lock: Arc<Mutex<()>>,
    camera: Mutex<mpsc::Sender<Job>>,
    /// Cancel flags of running enrollments, keyed by caller unique name.
    enrollments: Mutex<HashMap<String, Arc<AtomicBool>>>,
    agents: Agents,
    locks: Locks,
    /// Correlates AuthenticationStarted with AuthenticationFinished.
    next_auth_id: AtomicU32,
    backoff: Mutex<Backoff>,
}

fn failed(e: impl std::fmt::Display) -> Error {
    Error::Failed(e.to_string())
}

/// Map a failed call to another system service (bus, polkit, logind) to
/// `Failed`; returning the raw zbus error would look to clients as if the
/// faceauth daemon itself were unreachable.
fn service_error(service: &'static str) -> impl FnOnce(zbus::Error) -> Error {
    move |e| failed(format!("{service}: {e}"))
}

/// Map a core error to the D-Bus error returned to clients.
fn core_error(e: FaceAuthError) -> Error {
    match e {
        FaceAuthError::NoModel => Error::NoModel("no face model enrolled".into()),
        FaceAuthError::NoMatch { .. } | FaceAuthError::NoFace(_) => Error::NoMatch(e.to_string()),
        FaceAuthError::Camera(_) => Error::Camera(e.to_string()),
        FaceAuthError::Cancelled => Error::Cancelled(e.to_string()),
        other => Error::Failed(other.to_string()),
    }
}

/// Resolve a username to a UID.
fn resolve_user(username: &str) -> Result<u32> {
    users::by_name(username)
        .map(|u| u.uid)
        .ok_or_else(|| Error::InvalidArgs(format!("unknown user '{username}'")))
}

fn load_config() -> Config {
    let (config, warnings) = Config::load_system();
    for w in warnings {
        eprintln!("faceauth-daemon: config: {w}");
    }
    config
}

/// Display name for a camera: sysfs name, else its stable id.
fn camera_display(index: u32, id: &str) -> String {
    let name = v4l::camera_name(index);
    if name.is_empty() { id.to_string() } else { name }
}

impl Manager {
    pub fn new(bus: BusKind, store: Store, camera: mpsc::Sender<Job>, agents: Agents, locks: Locks) -> Self {
        Manager {
            bus,
            store,
            store_lock: Arc::new(Mutex::new(())),
            camera: Mutex::new(camera),
            enrollments: Mutex::new(HashMap::new()),
            agents,
            locks,
            next_auth_id: AtomicU32::new(1),
            backoff: Mutex::default(),
        }
    }

    /// Signal emitters addressed to each registered agent of `uid`.
    fn agent_emitters<'a>(&self, conn: &'a Connection, uid: u32) -> Vec<SignalEmitter<'a>> {
        let agents = self.agents.lock().unwrap_or_else(|p| p.into_inner());
        agents
            .iter()
            .filter(|(_, &u)| u == uid)
            .filter_map(|(name, _)| {
                let dest = BusName::try_from(name.clone()).ok()?;
                Some(SignalEmitter::new(conn, OBJECT_PATH).ok()?.set_destination(dest))
            })
            .collect()
    }

    /// Run blocking store work on the thread pool under the store lock.
    async fn with_store<T: Send + 'static>(&self, f: impl FnOnce(&Store) -> T + Send + 'static) -> T {
        let store = self.store.clone();
        let lock = self.store_lock.clone();
        blocking::unblock(move || {
            let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
            f(&store)
        })
        .await
    }

    async fn load_model(&self, uid: u32) -> Result<FaceModel> {
        self.with_store(move |s| s.load(uid)).await.map_err(core_error)
    }

    /// Identify the caller from the message header via the bus daemon.
    async fn caller(conn: &Connection, header: &Header<'_>) -> Result<Caller> {
        let sender: UniqueName<'_> = header.sender().ok_or_else(|| failed("message has no sender"))?.to_owned();
        let creds = DBusProxy::new(conn)
            .await
            .map_err(service_error("D-Bus"))?
            .get_connection_credentials(BusName::Unique(sender.clone()))
            .await
            .map_err(|e| service_error("D-Bus")(e.into()))?;
        let uid = creds.unix_user_id().ok_or_else(|| failed("caller UID unavailable"))?;
        Ok(Caller {
            sender: sender.to_string(),
            uid,
            pid: creds.process_id(),
            interactive: header.primary().flags().contains(Flags::AllowInteractiveAuth),
        })
    }

    /// A blocking connection to the system bus (logind lives there even when
    /// the daemon serves the session bus for development).
    async fn system_bus(&self, conn: &Connection) -> Result<zbus::blocking::Connection> {
        Ok(zbus::blocking::Connection::from(if self.bus == BusKind::System {
            conn.clone()
        } else {
            Connection::system().await.map_err(service_error("system bus"))?
        }))
    }

    /// Enforce the authorization decision for `op` on `target_uid`.
    async fn authorize(&self, conn: &Connection, caller: &Caller, op: Op, target_uid: u32) -> Result<()> {
        match decide(op, caller.uid, target_uid) {
            Decision::Allow => Ok(()),
            Decision::Deny(msg) => Err(Error::PermissionDenied(msg)),
            Decision::Polkit(_) if self.bus == BusKind::Session => Ok(()),
            Decision::Polkit(action) => match polkit::check(conn, &caller.sender, action, caller.interactive)
                .await
                .map_err(service_error("polkit"))?
            {
                polkit::Outcome::Authorized => Ok(()),
                polkit::Outcome::Challenge => {
                    Err(Error::NotAuthorized("authentication required; call with interactive authorization allowed".into()))
                }
                polkit::Outcome::Denied => Err(Error::NotAuthorized("not authorized".into())),
            },
            Decision::LocalSession => {
                let pid = caller.pid.ok_or_else(|| failed("caller PID unavailable"))?;
                let bconn = self.system_bus(conn).await?;
                let uid = caller.uid;
                let session = blocking::unblock(move || logind::caller_session(&bconn, pid, uid))
                    .await
                    .map_err(service_error("logind"))?;
                match session {
                    Some(s) => s.check_local_active().map_err(Error::NotLocalSession),
                    None => Err(Error::NotLocalSession("caller is not in a login session".into())),
                }
            }
        }
    }

    /// Object path of `uid`'s graphical session if it is locked.
    async fn locked_session(&self, conn: &Connection, uid: u32) -> Result<Option<String>> {
        let bconn = self.system_bus(conn).await?;
        blocking::unblock(move || logind::locked_display_session(&bconn, uid))
            .await
            .map_err(service_error("logind"))
    }

    /// Verify `service` is allowed by the admin, not disabled by the user,
    /// and, for login screens, used to unlock a locked session.
    async fn check_service(&self, uid: u32, service: &str, config: &Config, locked: bool) -> Result<()> {
        check_allowed(service, config)?;
        let disabled = self.with_store(move |s| s.disabled_services(uid)).await;
        if disabled.iter().any(|s| s == service) {
            return Err(Error::ServiceNotAllowed(format!("service '{service}' is disabled for this user")));
        }
        check_unlock_only(service, locked).map_err(Error::ServiceNotAllowed)
    }

    /// Resolve the model's camera; refuse if absent or not IR when required.
    fn model_camera(model: &FaceModel, config: &Config) -> Result<u32> {
        let index = v4l::resolve_camera(&model.camera)
            .ok_or_else(|| Error::Camera(format!("enrolled camera '{}' is not connected", model.camera)))?;
        check_camera_policy(index, config)?;
        Ok(index)
    }

    fn submit(&self, job: Job) -> Result<()> {
        self.camera
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .send(job)
            .map_err(|_| failed("camera worker stopped"))
    }

    /// Wait for the enrollment result, forwarding progress to the caller and
    /// cancelling if the caller disconnects.
    async fn drive_enrollment(
        conn: &Connection,
        caller: &Caller,
        cancel: &AtomicBool,
        progress: async_channel::Receiver<Progress>,
        reply: async_channel::Receiver<faceauth_core::Result<Vec<Encoding>>>,
    ) -> Result<Vec<Encoding>> {
        let destination = BusName::try_from(caller.sender.clone()).map_err(failed)?;
        let emitter = SignalEmitter::new(conn, OBJECT_PATH)?.set_destination(destination.clone());
        let dbus = DBusProxy::new(conn).await.map_err(service_error("D-Bus"))?;
        enum Event {
            Progress(Progress),
            Done(faceauth_core::Result<Vec<Encoding>>),
            Tick,
        }
        loop {
            let event = futures_lite::future::or(
                async {
                    match progress.recv().await {
                        Ok(p) => Event::Progress(p),
                        Err(_) => futures_lite::future::pending().await,
                    }
                },
                futures_lite::future::or(
                    async {
                        let stopped = || Err(FaceAuthError::Camera("camera worker stopped".into()));
                        Event::Done(reply.recv().await.unwrap_or_else(|_| stopped()))
                    },
                    async {
                        async_io::Timer::after(Duration::from_secs(1)).await;
                        Event::Tick
                    },
                ),
            )
            .await;
            match event {
                Event::Progress((done, total, status)) => {
                    let _ = Self::enroll_progress(&emitter, done, total, &status).await;
                }
                Event::Done(result) => return result.map_err(core_error),
                Event::Tick => {
                    if !dbus.name_has_owner(destination.clone()).await.unwrap_or(true) {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
            }
        }
    }
}

/// Removes an enrollment's cancel flag when the `Enroll` call ends, on
/// every path.
struct EnrollSlot<'a> {
    enrollments: &'a Mutex<HashMap<String, Arc<AtomicBool>>>,
    sender: String,
}

impl Drop for EnrollSlot<'_> {
    fn drop(&mut self) {
        self.enrollments.lock().unwrap_or_else(|p| p.into_inner()).remove(&self.sender);
    }
}

/// `service` is a valid name on the administrator's allow-list.
fn check_allowed(service: &str, config: &Config) -> Result<()> {
    if !is_valid_service_name(service) {
        return Err(Error::InvalidArgs(format!("invalid service name '{service}'")));
    }
    if !config.service_allowed(service) {
        return Err(Error::ServiceNotAllowed(format!("service '{service}' is not enabled by the administrator")));
    }
    Ok(())
}

/// Refuse virtual cameras and, if configured, non-IR cameras.
fn check_camera_policy(index: u32, config: &Config) -> Result<()> {
    if v4l::is_virtual(index) {
        return Err(Error::Camera(format!("/dev/video{index} is a virtual camera")));
    }
    if config.require_ir && !v4l::is_greyscale_only(index) {
        return Err(Error::Camera(format!("/dev/video{index} is not an IR camera (require_ir = yes)")));
    }
    Ok(())
}

#[interface(name = "io.github.gvarsanyi.FaceAuth1.Manager")]
impl Manager {
    /// Daemon version.
    #[zbus(property)]
    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    /// Usable capture devices: (index, name, suitability).
    #[zbus(out_args("cameras"))]
    async fn list_cameras(&self) -> Vec<CameraTuple> {
        blocking::unblock(|| v4l::list_cameras().into_iter().map(|c| (c.index, c.name.clone(), c.suitability())).collect())
            .await
    }

    /// (camera id, current camera index or -1, batch count).
    #[zbus(out_args("camera_id", "camera_index", "batches"))]
    async fn get_model(
        &self,
        username: &str,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(String, i32, u32)> {
        let caller = Self::caller(conn, &header).await?;
        let uid = resolve_user(username)?;
        self.authorize(conn, &caller, Op::Read, uid).await?;
        let model = self.load_model(uid).await?;
        let index = v4l::resolve_camera(&model.camera).and_then(|i| i32::try_from(i).ok()).unwrap_or(-1);
        Ok((model.camera, index, model.batches.len() as u32))
    }

    /// Capture and store a new enrollment batch. `camera_index` -1 reuses
    /// the camera of the existing model. Progress is reported to the caller
    /// via `EnrollProgress`.
    async fn enroll(
        &self,
        username: &str,
        camera_index: i32,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<()> {
        let caller = Self::caller(conn, &header).await?;
        let uid = resolve_user(username)?;
        // Register before authorizing, so CancelEnroll also works while the
        // polkit prompt is open.
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut running = self.enrollments.lock().unwrap_or_else(|p| p.into_inner());
            if running.contains_key(&caller.sender) {
                return Err(Error::Busy("an enrollment is already running for this client".into()));
            }
            running.insert(caller.sender.clone(), cancel.clone());
        }
        let _slot = EnrollSlot { enrollments: &self.enrollments, sender: caller.sender.clone() };
        self.authorize(conn, &caller, Op::Manage, uid).await?;
        let config = load_config();

        let existing = match self.load_model(uid).await {
            Ok(m) => Some(m),
            Err(Error::NoModel(_)) => None,
            Err(e) => return Err(e),
        };
        let index = match (camera_index, &existing) {
            (ENROLLED_CAMERA, Some(model)) => Self::model_camera(model, &config)?,
            (ENROLLED_CAMERA, None) => return Err(Error::InvalidArgs("no existing model; choose a camera".into())),
            (i, _) => {
                let index = u32::try_from(i).map_err(|_| Error::InvalidArgs(format!("invalid camera index {i}")))?;
                if !v4l::list_cameras().iter().any(|c| c.index == index) {
                    return Err(Error::InvalidArgs(format!("/dev/video{index} is not a usable camera")));
                }
                check_camera_policy(index, &config)?;
                if let Some(model) = &existing {
                    if model.camera != v4l::camera_id_for_index(index) {
                        return Err(Error::InvalidArgs(format!(
                            "already enrolled with camera '{}'; delete the model first to change cameras",
                            model.camera
                        )));
                    }
                }
                index
            }
        };

        let (progress_tx, progress_rx) = async_channel::unbounded();
        let (reply_tx, reply_rx) = async_channel::bounded(1);
        let params = EnrollParams {
            captures: ENROLL_CAPTURES,
            capture_timeout: Duration::from_secs(ENROLL_CAPTURE_TIMEOUT_SECS.into()),
            min_pose_distance: MIN_POSE_DISTANCE,
            frame_interval: FRAME_INTERVAL,
        };
        let result = match self.submit(Job::Enroll {
            camera_index: index,
            params,
            cancel: cancel.clone(),
            progress: progress_tx,
            reply: reply_tx,
        }) {
            Ok(()) => Self::drive_enrollment(conn, &caller, &cancel, progress_rx, reply_rx).await,
            Err(e) => Err(e),
        };
        let batch = result?;

        let camera_id = v4l::camera_id_for_index(index);
        self.with_store(move |s| {
            let mut model = match s.load(uid) {
                Ok(m) => m,
                Err(FaceAuthError::NoModel) => FaceModel { camera: camera_id.clone(), batches: Vec::new() },
                Err(e) => return Err(e),
            };
            if !model.batches.is_empty() && model.camera != camera_id {
                return Err(FaceAuthError::Camera("model camera changed during enrollment".into()));
            }
            model.camera = camera_id;
            model.batches.push(batch);
            s.save(uid, &model)
        })
        .await
        .map_err(core_error)?;
        eprintln!("faceauth-daemon: enrolled a batch for uid {uid} (requested by uid {})", caller.uid);
        Ok(())
    }

    /// Cancel the caller's running enrollment.
    async fn cancel_enroll(&self, #[zbus(header)] header: Header<'_>) {
        if let Some(sender) = header.sender() {
            if let Some(flag) = self.enrollments.lock().unwrap_or_else(|p| p.into_inner()).get(sender.as_str()) {
                flag.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Delete all of a user's enrolled face data.
    async fn delete_model(
        &self,
        username: &str,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<()> {
        let caller = Self::caller(conn, &header).await?;
        let uid = resolve_user(username)?;
        self.authorize(conn, &caller, Op::Manage, uid).await?;
        self.with_store(move |s| s.delete(uid)).await.map_err(core_error)?;
        eprintln!("faceauth-daemon: deleted model of uid {uid} (requested by uid {})", caller.uid);
        Ok(())
    }

    /// Delete one enrollment batch (0-based).
    async fn delete_batch(
        &self,
        username: &str,
        index: u32,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<()> {
        let caller = Self::caller(conn, &header).await?;
        let uid = resolve_user(username)?;
        self.authorize(conn, &caller, Op::Manage, uid).await?;
        self.with_store(move |s| {
            let mut model = s.load(uid).map_err(core_error)?;
            let i = index as usize;
            if i >= model.batches.len() {
                return Err(Error::InvalidArgs(format!("batch {index} out of range (have {})", model.batches.len())));
            }
            model.batches.remove(i);
            s.save(uid, &model).map_err(core_error)
        })
        .await
    }

    /// Services allowed by the administrator and whether each is enabled
    /// for the user.
    #[zbus(out_args("services"))]
    async fn get_services(
        &self,
        username: &str,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<Vec<(String, bool)>> {
        let caller = Self::caller(conn, &header).await?;
        let uid = resolve_user(username)?;
        self.authorize(conn, &caller, Op::Read, uid).await?;
        let config = load_config();
        let disabled = self.with_store(move |s| s.disabled_services(uid)).await;
        Ok(service_states(&config.services, &disabled))
    }

    /// Enable or disable an admin-allowed service for the user.
    async fn set_service_enabled(
        &self,
        username: &str,
        service: &str,
        enabled: bool,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<()> {
        let caller = Self::caller(conn, &header).await?;
        let uid = resolve_user(username)?;
        check_allowed(service, &load_config())?;
        self.authorize(conn, &caller, Op::Manage, uid).await?;
        let service = service.to_string();
        self.with_store(move |s| s.set_service_enabled(uid, &service, enabled)).await.map_err(core_error)
    }

    /// Check that authentication could run (service allowed, model enrolled,
    /// camera present) without opening the camera. Returns the camera's
    /// display name.
    #[zbus(out_args("camera"))]
    async fn check_authentication(
        &self,
        username: &str,
        service: &str,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<String> {
        let caller = Self::caller(conn, &header).await?;
        let uid = resolve_user(username)?;
        self.authorize(conn, &caller, Op::Read, uid).await?;
        let config = load_config();
        if !service.is_empty() {
            let locked = self.locked_session(conn, uid).await?.is_some();
            self.check_service(uid, service, &config, locked).await?;
        }
        let model = self.load_model(uid).await?;
        let index = Self::model_camera(&model, &config)?;
        Ok(camera_display(index, &model.camera))
    }

    /// Match the live camera against the user's model. `service` is the PAM
    /// service (empty = test mode, no service policy); `timeout_secs` 0 uses
    /// the configured default and is capped at 30.
    async fn authenticate(
        &self,
        username: &str,
        service: &str,
        timeout_secs: u32,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<()> {
        let caller = Self::caller(conn, &header).await?;
        let uid = resolve_user(username)?;
        self.authorize(conn, &caller, Op::Authenticate, uid).await?;
        let config = load_config();
        let locked = self.locked_session(conn, uid).await?;
        if !service.is_empty() {
            self.check_service(uid, service, &config, locked.is_some()).await?;
        }
        let model = self.load_model(uid).await?;
        let index = Self::model_camera(&model, &config)?;
        let params = AuthParams {
            threshold: config.threshold,
            required_matches: config.required_matches,
            timeout: Duration::from_secs(config.effective_timeout(timeout_secs).into()),
            frame_interval: FRAME_INTERVAL,
            absence: LOCK_ABSENCE,
        };
        // On a locked session, a match counts only after the camera has seen
        // nobody for LOCK_ABSENCE since the lock, so locking while seated
        // does not unlock again.
        let armed = match &locked {
            Some(path) => *self.locks.lock().unwrap_or_else(|p| p.into_inner()).entry(path.clone()).or_insert(false),
            None => true,
        };
        let (streak, delay) = self.backoff.lock().unwrap_or_else(|p| p.into_inner()).begin(uid, service, Instant::now());
        if !delay.is_zero() {
            async_io::Timer::after(delay).await;
            // The caller may have given up meanwhile (password typed).
            let dbus = DBusProxy::new(conn).await.map_err(service_error("D-Bus"))?;
            let sender = BusName::try_from(caller.sender.clone()).map_err(failed)?;
            if !dbus.name_has_owner(sender).await.unwrap_or(true) {
                self.backoff.lock().unwrap_or_else(|p| p.into_inner()).finish(uid, service, streak, false, Instant::now());
                return Err(Error::Cancelled("caller went away".into()));
            }
        }
        let id = self.next_auth_id.fetch_add(1, Ordering::Relaxed);
        let agents = self.agent_emitters(conn, uid);
        let caller_pid = caller.pid.unwrap_or(0);
        let secs = params.timeout.as_secs() as u32;
        for emitter in &agents {
            let _ = Self::authentication_started(emitter, id, service, caller_pid, secs).await;
        }
        let (reply_tx, reply_rx) = async_channel::bounded(1);
        let submitted = self.submit(Job::Authenticate {
            camera_index: index,
            enrolled: model.all_encodings(),
            params,
            armed,
            reply: reply_tx,
        });
        let stopped = || (Err(FaceAuthError::Camera("camera worker stopped".into())), armed);
        let (result, armed) = match submitted {
            Ok(()) => reply_rx.recv().await.unwrap_or_else(|_| stopped()),
            Err(_) => stopped(),
        };
        if let Some(path) = &locked {
            let mut locks = self.locks.lock().unwrap_or_else(|p| p.into_inner());
            // Keep the entry only while locked; the lock watcher removes it on unlock.
            if result.is_ok() {
                locks.remove(path);
            } else if let Some(entry) = locks.get_mut(path) {
                *entry = armed;
            }
        }
        for emitter in &agents {
            let _ = Self::authentication_finished(emitter, id, result.is_ok()).await;
        }
        self.backoff.lock().unwrap_or_else(|p| p.into_inner()).finish(uid, service, streak, result.is_ok(), Instant::now());
        let label = if service.is_empty() { "test" } else { service };
        let outcome = match &result {
            Ok(()) => "succeeded".to_string(),
            Err(e @ FaceAuthError::NoMatch { closest, .. }) => match closest {
                Some(d) => format!("failed: {e} (closest distance {d:.3}, threshold {})", config.threshold),
                None => format!("failed: {e} (no single-face frame)"),
            },
            Err(e) => format!("failed: {e}"),
        };
        let mut waited = if delay.is_zero() { String::new() } else { format!(" after a {}s back-off", delay.as_secs()) };
        if !armed {
            waited.push_str(" (screen locked; waiting until nobody is in view)");
        }
        eprintln!("faceauth-daemon: authentication for uid {uid} via '{label}' (caller uid {}){waited} {outcome}", caller.uid);
        result.map_err(core_error)
    }

    /// Register the caller as a notification agent for its own user. It
    /// receives AuthenticationStarted/AuthenticationFinished for
    /// authentications of that user until it disconnects.
    async fn register_agent(
        &self,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<()> {
        let caller = Self::caller(conn, &header).await?;
        let mut agents = self.agents.lock().unwrap_or_else(|p| p.into_inner());
        if !agents.contains_key(&caller.sender) && agents.values().filter(|&&u| u == caller.uid).count() >= MAX_AGENTS_PER_USER {
            return Err(Error::Busy("too many notification agents for this user".into()));
        }
        agents.insert(caller.sender, caller.uid);
        Ok(())
    }

    /// A camera check for the agent's user started. `caller_pid` is the
    /// process that requested it (0 if unknown); `service` is empty for tests.
    #[zbus(signal)]
    async fn authentication_started(
        emitter: &SignalEmitter<'_>,
        id: u32,
        service: &str,
        caller_pid: u32,
        timeout_secs: u32,
    ) -> zbus::Result<()>;

    /// The camera check `id` finished.
    #[zbus(signal)]
    async fn authentication_finished(emitter: &SignalEmitter<'_>, id: u32, success: bool) -> zbus::Result<()>;

    /// Enrollment progress, sent only to the client that called `Enroll`.
    /// `status` is `STATUS_CAPTURED` ("captured") or `STATUS_TOO_SIMILAR`
    /// ("too-similar").
    #[zbus(signal)]
    async fn enroll_progress(emitter: &SignalEmitter<'_>, done: u32, total: u32, status: &str) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_errors_map_to_dbus_errors() {
        let name = |e: FaceAuthError| format!("{:?}", core_error(e)).split('(').next().unwrap_or_default().to_string();
        assert_eq!(name(FaceAuthError::NoModel), "NoModel");
        assert_eq!(name(FaceAuthError::NoMatch { secs: 5, closest: None }), "NoMatch");
        assert_eq!(name(FaceAuthError::NoFace(5)), "NoMatch");
        assert_eq!(name(FaceAuthError::Camera("x".into())), "Camera");
        assert_eq!(name(FaceAuthError::Cancelled), "Cancelled");
        assert_eq!(name(FaceAuthError::Dlib("x".into())), "Failed");
    }

    #[test]
    fn allow_list_check() {
        let (config, _) = Config::parse("services = sudo\n");
        assert!(check_allowed("sudo", &config).is_ok());
        assert!(matches!(check_allowed("su", &config), Err(Error::ServiceNotAllowed(_))));
        assert!(matches!(check_allowed("../x", &config), Err(Error::InvalidArgs(_))));
    }
}
