//! Authentication and enrollment loops, independent of the camera so they
//! can be tested with synthetic encodings.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::encoding::{distance, min_distance, Encoding};
use crate::error::{FaceAuthError, Result};

/// Anything that yields the face encodings found in successive frames.
pub trait FaceSource {
    /// Faces in the next frame, or `None` for a frame that carries no image
    /// (the dark half of an IR camera's strobing emitter).
    fn next_faces(&mut self) -> Result<Option<Vec<Encoding>>>;
}

/// Parameters for [`authenticate`].
#[derive(Debug, Clone)]
pub struct AuthParams {
    /// Maximum distance for a frame to count as a match.
    pub threshold: f64,
    /// Consecutive matching frames required.
    pub required_matches: u32,
    pub timeout: Duration,
    /// Pause between frames.
    pub frame_interval: Duration,
    /// While not armed: how long lit frames must show no face before
    /// matches count (see [`authenticate`]).
    pub absence: Duration,
}

/// Parameters for [`enroll`].
#[derive(Debug, Clone)]
pub struct EnrollParams {
    pub captures: u32,
    /// Time allowed for each capture.
    pub capture_timeout: Duration,
    /// Captures closer than this to an earlier capture are rejected so the
    /// batch covers different poses.
    pub min_pose_distance: f64,
    pub frame_interval: Duration,
}

pub use faceauth_common::dbus::{STATUS_CAPTURED, STATUS_TOO_SIMILAR};

/// Run the matching loop until `required_matches` consecutive frames each
/// contain exactly one face within `threshold` of an enrolled encoding.
///
/// A frame with no face, several faces, or a non-matching face resets the
/// count, so a bystander in the frame cannot ride along on the owner's face.
/// Dark frames are skipped: they show nothing, so they neither count nor
/// reset.
///
/// If `armed` is false (a freshly locked screen), matches are ignored until
/// lit frames have shown no face for `params.absence`: the user left or
/// looked away since locking. `armed` is then set and stays set.
pub fn authenticate(
    source: &mut dyn FaceSource,
    enrolled: &[Encoding],
    params: &AuthParams,
    armed: &mut bool,
) -> Result<()> {
    if enrolled.is_empty() {
        return Err(FaceAuthError::NoModel);
    }
    let deadline = Instant::now() + params.timeout;
    let mut streak = 0u32;
    let mut closest: Option<f64> = None;
    let mut absent_since: Option<Instant> = None;
    let mut frames_ok = false;
    let mut last_error = None;

    while Instant::now() < deadline {
        match source.next_faces() {
            Ok(None) => {
                frames_ok = true;
                continue;
            }
            Ok(Some(faces)) if !*armed => {
                frames_ok = true;
                if faces.is_empty() {
                    let since = *absent_since.get_or_insert_with(Instant::now);
                    *armed = since.elapsed() >= params.absence;
                } else {
                    absent_since = None;
                }
            }
            Ok(Some(faces)) => {
                frames_ok = true;
                let dist = (faces.len() == 1).then(|| min_distance(&faces[0], enrolled));
                if let Some(d) = dist {
                    closest = Some(closest.map_or(d, |c| c.min(d)));
                }
                let hit = dist.is_some_and(|d| d < params.threshold);
                streak = if hit { streak + 1 } else { 0 };
                if streak >= params.required_matches {
                    return Ok(());
                }
            }
            Err(e) => last_error = Some(e),
        }
        std::thread::sleep(params.frame_interval);
    }
    match last_error {
        Some(e) if !frames_ok => Err(e),
        _ => Err(FaceAuthError::NoMatch { secs: params.timeout.as_secs(), closest }),
    }
}

/// Capture `params.captures` distinct single-face encodings.
///
/// `progress(done, total, status)` is called after each accepted capture
/// (`STATUS_CAPTURED`) and each rejected near-duplicate (`STATUS_TOO_SIMILAR`).
/// Checks `cancel` before every frame.
pub fn enroll(
    source: &mut dyn FaceSource,
    params: &EnrollParams,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u32, u32, &str),
) -> Result<Vec<Encoding>> {
    let mut captured: Vec<Encoding> = Vec::new();
    let mut deadline = Instant::now() + params.capture_timeout;
    let mut frames_ok = false;
    let mut last_error = None;

    while (captured.len() as u32) < params.captures {
        if cancel.load(Ordering::Relaxed) {
            return Err(FaceAuthError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(match last_error {
                Some(e) if !frames_ok => e,
                _ => FaceAuthError::NoFace(params.capture_timeout.as_secs()),
            });
        }
        let faces = match source.next_faces() {
            Ok(Some(f)) if f.len() == 1 => f,
            other => {
                match other {
                    Ok(_) => frames_ok = true,
                    Err(e) => last_error = Some(e),
                }
                std::thread::sleep(params.frame_interval);
                continue;
            }
        };
        frames_ok = true;
        let done = captured.len() as u32;
        if captured.iter().any(|prev| distance(prev, &faces[0]) < params.min_pose_distance) {
            progress(done, params.captures, STATUS_TOO_SIMILAR);
        } else {
            captured.push(faces[0]);
            progress(done + 1, params.captures, STATUS_CAPTURED);
            deadline = Instant::now() + params.capture_timeout;
        }
        std::thread::sleep(params.frame_interval);
    }
    Ok(captured)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Replays a fixed list of frames, then repeats the last one.
    struct Script(VecDeque<Result<Option<Vec<Encoding>>>>);

    impl FaceSource for Script {
        fn next_faces(&mut self) -> Result<Option<Vec<Encoding>>> {
            if self.0.len() > 1 {
                self.0.pop_front().unwrap()
            } else {
                match self.0.front() {
                    Some(Ok(f)) => Ok(f.clone()),
                    _ => Err(FaceAuthError::Camera("no frames".into())),
                }
            }
        }
    }

    fn enc(v: f32) -> Encoding {
        [v; 128]
    }

    fn script(frames: Vec<Vec<Encoding>>) -> Script {
        Script(frames.into_iter().map(|f| Ok(Some(f))).collect())
    }

    fn auth_params(required: u32) -> AuthParams {
        AuthParams {
            threshold: 0.45,
            required_matches: required,
            timeout: Duration::from_millis(200),
            frame_interval: Duration::ZERO,
            absence: Duration::from_millis(30),
        }
    }

    /// Authenticate with matching armed from the start.
    fn auth(src: &mut dyn FaceSource, enrolled: &[Encoding], params: &AuthParams) -> Result<()> {
        authenticate(src, enrolled, params, &mut true)
    }

    const OWNER: f32 = 0.10;
    const STRANGER: f32 = 0.50; // distance sqrt(128)*0.4 ~ 4.5 from OWNER

    #[test]
    fn consecutive_matches_succeed() {
        let mut src = script(vec![vec![enc(OWNER)]; 3]);
        assert!(auth(&mut src, &[enc(OWNER)], &auth_params(3)).is_ok());
    }

    #[test]
    fn stranger_never_matches() {
        let mut src = script(vec![vec![enc(STRANGER)]]);
        let err = auth(&mut src, &[enc(OWNER)], &auth_params(1)).unwrap_err();
        assert!(matches!(err, FaceAuthError::NoMatch { closest: Some(d), .. } if d > 1.0));
    }

    #[test]
    fn interrupted_streak_resets() {
        // match, stranger, match, then empty frames forever: never 2 in a row.
        let mut src = script(vec![vec![enc(OWNER)], vec![enc(STRANGER)], vec![enc(OWNER)], vec![]]);
        assert!(auth(&mut src, &[enc(OWNER)], &auth_params(2)).is_err());
    }

    #[test]
    fn dark_frames_do_not_break_the_streak() {
        // Strobing IR emitter: every other frame is dark.
        let frames = [Some(vec![enc(OWNER)]), None, Some(vec![enc(OWNER)]), None, Some(vec![enc(OWNER)]), None];
        let mut src = Script(frames.into_iter().map(Ok).collect());
        assert!(auth(&mut src, &[enc(OWNER)], &auth_params(3)).is_ok());
    }

    #[test]
    fn dark_frames_do_not_count_as_matches() {
        let frames = [Some(vec![enc(OWNER)]), None, Some(vec![]), None];
        let mut src = Script(frames.into_iter().map(Ok).collect());
        assert!(auth(&mut src, &[enc(OWNER)], &auth_params(2)).is_err());
    }

    /// Params for lock re-arming tests: 5 ms per frame, 30 ms absence.
    fn arming_params() -> AuthParams {
        AuthParams { frame_interval: Duration::from_millis(5), ..auth_params(2) }
    }

    fn frames(list: Vec<Option<Vec<Encoding>>>) -> Script {
        Script(list.into_iter().map(Ok).collect())
    }

    #[test]
    fn unarmed_owner_who_never_left_is_not_accepted() {
        let mut armed = false;
        let mut src = frames(vec![Some(vec![enc(OWNER)])]);
        assert!(authenticate(&mut src, &[enc(OWNER)], &arming_params(), &mut armed).is_err());
        assert!(!armed);
    }

    #[test]
    fn absence_arms_then_owner_matches() {
        let mut armed = false;
        let mut list = vec![Some(vec![enc(OWNER)]), Some(vec![enc(OWNER)])];
        list.extend(std::iter::repeat_n(Some(vec![]), 10));
        list.push(Some(vec![enc(OWNER)]));
        let mut src = frames(list);
        assert!(authenticate(&mut src, &[enc(OWNER)], &arming_params(), &mut armed).is_ok());
        assert!(armed);
    }

    #[test]
    fn short_absence_or_darkness_does_not_arm() {
        for list in [
            vec![Some(vec![]), Some(vec![]), Some(vec![enc(OWNER)])],
            vec![Some(vec![enc(OWNER)]), None],
        ] {
            let mut armed = false;
            let mut src = frames(list);
            assert!(authenticate(&mut src, &[enc(OWNER)], &arming_params(), &mut armed).is_err());
            assert!(!armed);
        }
    }

    #[test]
    fn owner_with_bystander_does_not_match() {
        let mut src = script(vec![vec![enc(OWNER), enc(STRANGER)]]);
        assert!(auth(&mut src, &[enc(OWNER)], &auth_params(1)).is_err());
    }

    #[test]
    fn empty_model_is_no_model() {
        let mut src = script(vec![vec![enc(OWNER)]]);
        assert!(matches!(auth(&mut src, &[], &auth_params(1)), Err(FaceAuthError::NoModel)));
    }

    #[test]
    fn camera_failure_is_reported() {
        let mut src = Script(VecDeque::from([Err(FaceAuthError::Camera("gone".into()))]));
        let err = auth(&mut src, &[enc(OWNER)], &auth_params(1)).unwrap_err();
        assert!(matches!(err, FaceAuthError::Camera(_)));
    }

    fn enroll_params() -> EnrollParams {
        EnrollParams {
            captures: 3,
            capture_timeout: Duration::from_millis(200),
            min_pose_distance: 0.3,
            frame_interval: Duration::ZERO,
        }
    }

    #[test]
    fn enroll_collects_distinct_poses_and_reports_progress() {
        let frames = vec![vec![enc(0.1)], vec![enc(0.1)], vec![], vec![enc(0.2)], vec![enc(0.3)]];
        let mut events = Vec::new();
        let out = enroll(&mut script(frames), &enroll_params(), &AtomicBool::new(false), &mut |d, t, s| {
            events.push((d, t, s.to_string()))
        })
        .unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(
            events,
            vec![
                (1, 3, STATUS_CAPTURED.to_string()),
                (1, 3, STATUS_TOO_SIMILAR.to_string()),
                (2, 3, STATUS_CAPTURED.to_string()),
                (3, 3, STATUS_CAPTURED.to_string()),
            ]
        );
    }

    #[test]
    fn enroll_times_out_without_a_single_face() {
        let err = enroll(&mut script(vec![vec![]]), &enroll_params(), &AtomicBool::new(false), &mut |_, _, _| {})
            .unwrap_err();
        assert!(matches!(err, FaceAuthError::NoFace(_)));
    }

    #[test]
    fn enroll_reports_persistent_camera_failure() {
        let mut src = Script(VecDeque::from([Err(FaceAuthError::Camera("gone".into()))]));
        let err = enroll(&mut src, &enroll_params(), &AtomicBool::new(false), &mut |_, _, _| {}).unwrap_err();
        assert!(matches!(err, FaceAuthError::Camera(_)));
    }

    #[test]
    fn enroll_honours_cancel() {
        let err = enroll(&mut script(vec![vec![enc(0.1)]]), &enroll_params(), &AtomicBool::new(true), &mut |_, _, _| {})
            .unwrap_err();
        assert!(matches!(err, FaceAuthError::Cancelled));
    }
}
