//! polkit authorization checks for D-Bus callers.

use std::collections::HashMap;

use zbus::zvariant::Value;

const POLKIT: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_IFACE: &str = "org.freedesktop.PolicyKit1.Authority";
const ALLOW_USER_INTERACTION: u32 = 1;

/// Outcome of a polkit check.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Authorized,
    /// Authorization possible but requires interaction the caller did not allow.
    Challenge,
    Denied,
}

/// Ask polkit whether the D-Bus peer `sender` may perform `action`.
///
/// The subject is the caller's unique bus name, which polkit resolves to the
/// process and session itself; this avoids PID-reuse races.
pub async fn check(conn: &zbus::Connection, sender: &str, action: &str, interactive: bool) -> zbus::Result<Outcome> {
    let subject = ("system-bus-name", HashMap::from([("name", Value::from(sender))]));
    let details: HashMap<&str, &str> = HashMap::new();
    let flags = if interactive { ALLOW_USER_INTERACTION } else { 0 };
    let reply = conn
        .call_method(
            Some(POLKIT),
            AUTHORITY_PATH,
            Some(AUTHORITY_IFACE),
            "CheckAuthorization",
            &(subject, action, details, flags, ""),
        )
        .await?;
    let (authorized, challenge, _details): (bool, bool, HashMap<String, String>) = reply.body().deserialize()?;
    Ok(match (authorized, challenge) {
        (true, _) => Outcome::Authorized,
        (false, true) => Outcome::Challenge,
        (false, false) => Outcome::Denied,
    })
}
