//! faceauth-daemon: owns the camera and face models and serves the
//! `io.github.gvarsanyi.FaceAuth1` D-Bus API. Normally started by D-Bus
//! activation through faceauth-daemon.service.

use std::path::PathBuf;

use faceauth_common::dbus::{BUS_NAME, OBJECT_PATH};
use faceauth_core::store::{Store, STATE_DIR};

mod authz;
mod backoff;
mod camera_actor;
mod manager;
mod polkit;

use std::collections::HashMap;

use faceauth_common::logind;
use futures_lite::StreamExt;
use manager::{Agents, BusKind, Locks, Manager};
use zbus::fdo::DBusProxy;
use zbus::zvariant::OwnedValue;
use zbus::{Connection, MatchRule, MessageStream};

const USAGE: &str = "usage: faceauth-daemon [--session-bus --state-dir DIR]

  --session-bus     serve on the session bus, unprivileged and without polkit
                    (development and testing only)
  --state-dir DIR   state directory (default /var/lib/faceauth; only with --session-bus)";

fn parse_args() -> Result<(BusKind, PathBuf), String> {
    let mut bus = BusKind::System;
    let mut state_dir = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--session-bus" => bus = BusKind::Session,
            "--state-dir" => state_dir = Some(PathBuf::from(args.next().ok_or("--state-dir needs a value")?)),
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if bus == BusKind::System && state_dir.is_some() {
        return Err("--state-dir requires --session-bus".into());
    }
    Ok((bus, state_dir.unwrap_or_else(|| PathBuf::from(STATE_DIR))))
}

/// Forget notification agents when their connection closes.
async fn watch_agents(conn: &Connection, agents: Agents) -> zbus::Result<()> {
    let mut changes = DBusProxy::new(conn).await?.receive_name_owner_changed().await?;
    while let Some(change) = changes.next().await {
        if let Ok(args) = change.args() {
            if args.new_owner().is_none() {
                agents.lock().unwrap_or_else(|p| p.into_inner()).remove(args.name().as_str());
            }
        }
    }
    Ok(())
}

/// Track logind's LockedHint: a session that locks starts disarmed (face
/// unlock waits until nobody is in view); unlocking forgets it.
async fn watch_locks(system: &Connection, locks: Locks) -> zbus::Result<()> {
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")?
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .arg(0, "org.freedesktop.login1.Session")?
        .build();
    let mut stream = MessageStream::for_match_rule(rule, system, None).await?;
    while let Some(msg) = stream.next().await {
        // A malformed message must not end lock tracking.
        let Ok(msg) = msg else { continue };
        let Some(path) = msg.header().path().map(|p| p.to_string()) else { continue };
        let Ok((_, changed, invalidated)) = msg.body().deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>()
        else {
            continue;
        };
        let locked = match changed.get("LockedHint") {
            Some(v) => bool::try_from(v).ok(),
            None if invalidated.iter().any(|p| p == "LockedHint") => {
                let (conn, p) = (zbus::blocking::Connection::from(system.clone()), path.clone());
                blocking::unblock(move || logind::locked_hint(&conn, &p)).await.ok()
            }
            None => None,
        };
        let mut locks = locks.lock().unwrap_or_else(|p| p.into_inner());
        match locked {
            Some(true) => {
                eprintln!("faceauth-daemon: {path} locked; face unlock waits until nobody is in view");
                locks.insert(path, false);
            }
            Some(false) => {
                if locks.remove(&path).is_some() {
                    eprintln!("faceauth-daemon: {path} unlocked");
                }
            }
            None => {}
        }
    }
    Ok(())
}

async fn serve(bus: BusKind, state_dir: PathBuf) -> zbus::Result<()> {
    let agents = Agents::default();
    let locks = Locks::default();
    let manager = Manager::new(bus, Store::new(&state_dir), camera_actor::start(), agents.clone(), locks.clone());
    let builder = match bus {
        BusKind::System => zbus::connection::Builder::system()?,
        BusKind::Session => zbus::connection::Builder::session()?,
    };
    let conn = builder.name(BUS_NAME)?.serve_at(OBJECT_PATH, manager)?.build().await?;
    eprintln!("faceauth-daemon {} serving {BUS_NAME} (state: {})", env!("CARGO_PKG_VERSION"), state_dir.display());

    let system = match bus {
        BusKind::System => conn.clone(),
        BusKind::Session => Connection::system().await?,
    };
    let (a, l) = futures_lite::future::zip(watch_agents(&conn, agents), watch_locks(&system, locks)).await;
    a.and(l)
}

fn main() {
    let (bus, state_dir) = match parse_args() {
        Ok(v) => v,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("faceauth-daemon: {msg}");
            }
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(e) = std::fs::create_dir_all(&state_dir) {
        eprintln!("faceauth-daemon: cannot create {}: {e}", state_dir.display());
        std::process::exit(1);
    }
    if let Err(e) = zbus::block_on(serve(bus, state_dir)) {
        eprintln!("faceauth-daemon: {e}");
        std::process::exit(1);
    }
}
