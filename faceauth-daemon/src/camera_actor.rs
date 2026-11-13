//! A single worker thread owns the face encoder and the camera, so camera
//! jobs run one at a time. The dlib models are loaded on demand and
//! unloaded after a few idle minutes (reloading takes well under a second).

use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use faceauth_core::capture::LiveSource;
use faceauth_core::matcher::{self, AuthParams, EnrollParams};
use faceauth_core::{Encoding, FaceAuthError, FaceEncoder, Result};

/// Idle time after which the face models are unloaded.
const MODEL_IDLE: Duration = Duration::from_secs(5 * 60);

/// Progress event: (done, total, status).
pub type Progress = (u32, u32, String);

pub enum Job {
    Authenticate {
        camera_index: u32,
        enrolled: Vec<Encoding>,
        params: AuthParams,
        /// See `matcher::authenticate`; returned updated with the result.
        armed: bool,
        reply: async_channel::Sender<(Result<()>, bool)>,
    },
    Enroll {
        camera_index: u32,
        params: EnrollParams,
        cancel: Arc<AtomicBool>,
        progress: async_channel::Sender<Progress>,
        reply: async_channel::Sender<Result<Vec<Encoding>>>,
    },
}

/// Start the worker thread and return its job queue.
pub fn start() -> mpsc::Sender<Job> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("camera".into())
        .spawn(move || run(rx))
        .expect("failed to spawn camera thread");
    tx
}

fn run(rx: mpsc::Receiver<Job>) {
    // Loaded on first use; a failed load is retried on the next job.
    let mut encoder: Option<FaceEncoder> = None;
    loop {
        let next = if encoder.is_some() { rx.recv_timeout(MODEL_IDLE) } else { rx.recv().map_err(Into::into) };
        let job = match next {
            Ok(job) => job,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                encoder = None;
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if encoder.is_none() {
            match FaceEncoder::new() {
                Ok(e) => encoder = Some(e),
                Err(e) => {
                    eprintln!("faceauth-daemon: failed to load face models: {e}");
                    fail(job, FaceAuthError::Dlib(format!("face models unavailable: {e}")));
                    continue;
                }
            }
        }
        let encoder = encoder.as_ref().expect("encoder initialized above");
        match job {
            Job::Authenticate { camera_index, enrolled, params, mut armed, reply } => {
                let result = LiveSource::open(camera_index, encoder)
                    .and_then(|mut src| matcher::authenticate(&mut src, &enrolled, &params, &mut armed));
                let _ = reply.send_blocking((result, armed));
            }
            Job::Enroll { camera_index, params, cancel, progress, reply } => {
                let result = LiveSource::open(camera_index, encoder).and_then(|mut src| {
                    matcher::enroll(&mut src, &params, &cancel, &mut |d, t, s| {
                        let _ = progress.try_send((d, t, s.to_string()));
                    })
                });
                let _ = reply.send_blocking(result);
            }
        }
    }
}

fn fail(job: Job, err: FaceAuthError) {
    match job {
        Job::Authenticate { armed, reply, .. } => drop(reply.send_blocking((Err(err), armed))),
        Job::Enroll { reply, .. } => drop(reply.send_blocking(Err(err))),
    }
}
