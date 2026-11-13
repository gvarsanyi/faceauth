# Security

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub's
[private vulnerability reporting](https://github.com/gvarsanyi/faceauth/security/advisories/new)
rather than in public issues. Include the version, distribution, and steps to
reproduce. You should receive a response within a week.

## Threat model

faceauth is a convenience factor. It is designed so that:

- **It never weakens the password path.** Every outcome other than a match
  returns `PAM_AUTHINFO_UNAVAIL`; the stack continues with the password.
- **It only vouches for the user being authenticated.** The module matches
  the face of `PAM_USER`, never the invoking user, so `su` and `sudo` with
  `targetpw`/`rootpw` cannot be satisfied by your own face.
- **Remote users cannot use it or switch on the camera.** `PAM_RHOST`,
  logind session locality (remote flag, seat, foreground), and, in the
  daemon, a logind check of every non-root caller of `Authenticate`.
- **Local processes cannot plant a face.** Enrollment captures happen inside
  the daemon; clients cannot submit encodings. Every change needs polkit
  authorization (your password by default).
- **Injected video is refused.** v4l2loopback, akvcam, and vivid devices are
  never opened.
- **The daemon is contained.** It runs as `faceauthd` with an empty
  capability set, no network, read-only system, and only video devices
  allowed. The PAM module connects to the fixed system bus socket and
  ignores `DBUS_SYSTEM_BUS_ADDRESS`.

## Known limitations

- **No liveness detection.** With a colour camera, a photo or video of the
  user can succeed. IR cameras defeat ordinary photos and screens but not
  every printed or crafted IR attack. Set `require_ir = yes` if you have an
  IR camera.
- **Similar faces.** dlib's model has measurably higher false-accept rates
  for close relatives and some demographics. Lower `threshold` if this
  matters to you.
- **Physical presence is the whole check.** Anyone who can get the enrolled
  user in front of the camera (asleep, coerced) can authenticate.
- **Local same-session triggers.** A process already running as you in your
  foreground session (for example inside a tmux session started locally and
  later attached over SSH, or a job started with `systemd-run --user`) can
  trigger a face check while you sit at the machine: processes outside any
  login session are judged by your graphical session, as polkit does. The
  desktop notification makes this visible.
- **Lock screens.** KDE starts face authentication as soon as the screen
  locks. While the session is locked (logind `LockedHint`, set by KDE Plasma
  and GNOME), a face counts only after the camera has seen nobody for two
  seconds since the lock: locking while seated keeps the screen locked
  until you look away or leave. Looking away briefly and back therefore
  unlocks; the screen lock protects against others, not against you. COSMIC
  does not report the lock to logind, so this does not apply there. Failed
  attempts back off (up to 20 s apart), so the camera is not on
  continuously while you are away.
