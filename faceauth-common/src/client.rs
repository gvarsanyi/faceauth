//! Blocking client for the faceauth daemon's D-Bus API.

use std::time::Duration;

use futures_lite::StreamExt;
use zbus::blocking::{connection, Connection, Proxy};
use zbus::proxy::{CacheProperties, MethodFlags};

use crate::dbus::{
    CameraTuple, Error, Result, AGENT_BUS_NAME, BUS_NAME, INTERFACE, OBJECT_PATH, SYSTEM_BUS_ADDRESS,
};

/// Summary of a user's enrolled model as returned by `GetModel`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// Stable camera identifier stored at enrollment.
    pub camera_id: String,
    /// Current `/dev/videoN` index of that camera, if it is present.
    pub camera_index: Option<u32>,
    /// Number of enrollment batches.
    pub batches: u32,
}

/// True if a faceauth-agent runs in the caller's session.
pub fn agent_running() -> zbus::Result<bool> {
    let session = Connection::session()?;
    Ok(zbus::blocking::fdo::DBusProxy::new(&session)?.name_has_owner(AGENT_BUS_NAME.try_into()?)?)
}

/// Connection to the daemon.
pub struct Client {
    conn: Connection,
    proxy: Proxy<'static>,
}

impl Client {
    /// Connect via the fixed system bus socket. Ignores
    /// `DBUS_SYSTEM_BUS_ADDRESS`; required for use inside setuid processes.
    pub fn system(method_timeout: Duration) -> Result<Client> {
        let conn = connection::Builder::address(SYSTEM_BUS_ADDRESS)?
            .method_timeout(method_timeout)
            .build()?;
        Self::with_connection(conn)
    }

    /// Connect for an unprivileged tool. `FACEAUTH_BUS=session` selects the
    /// session bus (for running a development daemon); otherwise this is
    /// identical to [`Client::system`].
    pub fn from_env(method_timeout: Duration) -> Result<Client> {
        if std::env::var("FACEAUTH_BUS").as_deref() == Ok("session") {
            let conn = connection::Builder::session()?.method_timeout(method_timeout).build()?;
            return Self::with_connection(conn);
        }
        Self::system(method_timeout)
    }

    fn with_connection(conn: Connection) -> Result<Client> {
        let proxy = zbus::blocking::proxy::Builder::new(&conn)
            .destination(BUS_NAME)?
            .path(OBJECT_PATH)?
            .interface(INTERFACE)?
            .cache_properties(CacheProperties::No)
            .build()?;
        Ok(Client { conn, proxy })
    }

    /// The underlying bus connection (shared with logind queries).
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    fn call<B, R>(&self, method: &'static str, body: &B) -> Result<R>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        Ok(self.proxy.call(method, body)?)
    }

    /// Call a method that may require polkit authorization; allows the
    /// caller's polkit agent to prompt.
    fn call_interactive<B>(&self, method: &'static str, body: &B) -> Result<()>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        self.proxy
            .call_with_flags::<_, _, ()>(method, MethodFlags::AllowInteractiveAuth.into(), body)?;
        Ok(())
    }

    /// Daemon version string.
    pub fn version(&self) -> Result<String> {
        Ok(self.proxy.get_property("Version")?)
    }

    pub fn list_cameras(&self) -> Result<Vec<CameraTuple>> {
        self.call("ListCameras", &())
    }

    pub fn get_model(&self, user: &str) -> Result<ModelInfo> {
        let (camera_id, index, batches): (String, i32, u32) = self.call("GetModel", &(user,))?;
        Ok(ModelInfo { camera_id, camera_index: u32::try_from(index).ok(), batches })
    }

    /// Enroll a new batch for `user` from `camera_index`
    /// ([`crate::dbus::ENROLLED_CAMERA`] reuses the model's camera).
    /// `progress(done, total, status)` is called for each `EnrollProgress`
    /// signal while the call is running.
    pub fn enroll(&self, user: &str, camera_index: i32, mut progress: impl FnMut(u32, u32, &str)) -> Result<()> {
        let inner = self.proxy.inner();
        zbus::block_on(async {
            let mut signals = inner.receive_signal("EnrollProgress").await?;
            let args = (user, camera_index);
            let call = inner.call_with_flags::<_, _, ()>("Enroll", MethodFlags::AllowInteractiveAuth.into(), &args);
            futures_lite::pin!(call);
            enum Event {
                Done(zbus::Result<Option<()>>),
                Signal(Option<zbus::Message>),
            }
            loop {
                let event = futures_lite::future::or(
                    async { Event::Done((&mut call).await) },
                    async { Event::Signal(signals.next().await) },
                )
                .await;
                match event {
                    Event::Done(r) => return r.map(|_| ()).map_err(Error::from),
                    Event::Signal(Some(msg)) => {
                        if let Ok((done, total, status)) = msg.body().deserialize::<(u32, u32, String)>() {
                            progress(done, total, &status);
                        }
                    }
                    // No more progress signals: just wait for the reply.
                    Event::Signal(None) => return call.await.map(|_| ()).map_err(Error::from),
                }
            }
        })
    }

    /// Cancel this connection's running enrollment, if any.
    pub fn cancel_enroll(&self) -> Result<()> {
        self.call("CancelEnroll", &())
    }

    pub fn delete_model(&self, user: &str) -> Result<()> {
        self.call_interactive("DeleteModel", &(user,))
    }

    pub fn delete_batch(&self, user: &str, index: u32) -> Result<()> {
        self.call_interactive("DeleteBatch", &(user, index))
    }

    /// Admin-allowed services and whether each is enabled for `user`.
    pub fn get_services(&self, user: &str) -> Result<Vec<(String, bool)>> {
        self.call("GetServices", &(user,))
    }

    pub fn set_service_enabled(&self, user: &str, service: &str, enabled: bool) -> Result<()> {
        self.call_interactive("SetServiceEnabled", &(user, service, enabled))
    }

    /// Verify that face authentication can be attempted for `user` via
    /// `service` (service allowed, model enrolled, camera present) without
    /// opening the camera. Returns a display name for the camera.
    pub fn check_authentication(&self, user: &str, service: &str) -> Result<String> {
        self.call("CheckAuthentication", &(user, service))
    }

    /// Register this connection as the notification agent of its user; see
    /// `AuthenticationStarted`/`AuthenticationFinished` on [`Client::proxy`].
    pub fn register_agent(&self) -> Result<()> {
        self.call("RegisterAgent", &())
    }

    /// The daemon proxy, for subscribing to its signals.
    pub fn proxy(&self) -> &Proxy<'static> {
        &self.proxy
    }

    /// Run face authentication. `service` empty = test mode (no service
    /// policy); `timeout_secs` 0 = configured default.
    pub fn authenticate(&self, user: &str, service: &str, timeout_secs: u32) -> Result<()> {
        self.call("Authenticate", &(user, service, timeout_secs))
    }
}
