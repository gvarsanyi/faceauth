//! Text-mode polkit agent for terminal sessions.
//!
//! When the CLI runs in a terminal, it starts `pkttyagent` registered for
//! this process so polkit can prompt for the password on the terminal. With
//! `--fallback`, a graphical agent of the session takes precedence.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

/// Running `pkttyagent`; terminated on drop.
pub struct TtyAgent(Child);

impl Drop for TtyAgent {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Start `pkttyagent` if stdin is a terminal and we are not root. Waits
/// (up to 5 s) until the agent has registered. Returns `None` if no agent
/// is needed or it could not be started; polkit then uses any session agent.
pub fn start() -> Option<TtyAgent> {
    // SAFETY: trivial libc queries.
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 0 || unsafe { libc::getuid() } == 0 {
        return None;
    }
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: fds has room for two descriptors.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return None;
    }
    // SAFETY: pipe2 returned two fresh descriptors that we now own.
    let (read_end, write_end) =
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let notify_fd = write_end.as_raw_fd();

    let mut cmd = Command::new("pkttyagent");
    cmd.args([
        "--process",
        &std::process::id().to_string(),
        "--notify-fd",
        &notify_fd.to_string(),
        "--fallback",
    ])
    .stdout(Stdio::null());
    // SAFETY: fcntl is async-signal-safe; it makes the notify fd survive exec.
    unsafe {
        cmd.pre_exec(move || {
            if libc::fcntl(notify_fd, libc::F_SETFD, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn().ok()?;
    drop(write_end);

    // pkttyagent closes the notify fd once registered: wait for EOF.
    let mut pfd = libc::pollfd {
        fd: read_end.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: pfd is one valid pollfd.
    unsafe { libc::poll(&mut pfd, 1, 5000) };
    Some(TtyAgent(child))
}
