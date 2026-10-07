//! The newest-frame slot every backend publishes into and `OpenedCamera` reads from.

use crate::{CameraError, Result, RgbFrame};
use std::{
    sync::{Condvar, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Shared {
    latest: Option<RgbFrame>,
    sequence: u64,
    status: Option<String>,
    failure: Option<String>,
}

#[derive(Default)]
pub(crate) struct FrameSlot {
    shared: Mutex<Shared>,
    changed: Condvar,
}

impl FrameSlot {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replace the newest frame. `data` is packed RGB of `width * height * 3` bytes.
    pub(crate) fn publish(&self, width: u32, height: u32, data: Vec<u8>) {
        let mut shared = self.lock();
        shared.sequence += 1;
        shared.latest = Some(RgbFrame {
            width,
            height,
            sequence: shared.sequence,
            data,
        });
        drop(shared);
        self.changed.notify_all();
    }

    /// Record a status message. A failure ends every later wait with `Disconnected`.
    pub(crate) fn report(&self, message: String, failure: bool) {
        let mut shared = self.lock();
        if failure {
            shared.failure = Some(message.clone());
        }
        shared.status = Some(message);
        drop(shared);
        self.changed.notify_all();
    }

    pub(crate) fn status(&self) -> Option<String> {
        self.lock().status.clone()
    }

    pub(crate) fn latest(&self) -> Result<RgbFrame> {
        let shared = self.lock();
        if let Some(failure) = &shared.failure {
            return Err(CameraError::Disconnected(failure.clone()));
        }
        shared.latest.clone().ok_or(CameraError::NoFrame)
    }

    /// Wait for a frame whose sequence is greater than `after`.
    pub(crate) fn wait_newer(&self, after: u64, timeout: Duration) -> Result<RgbFrame> {
        let deadline = Instant::now() + timeout;
        let mut shared = self.lock();
        loop {
            if let Some(failure) = &shared.failure {
                return Err(CameraError::Disconnected(failure.clone()));
            }
            if let Some(frame) = &shared.latest
                && frame.sequence > after
            {
                return Ok(frame.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(CameraError::Timeout);
            }
            shared = self
                .changed
                .wait_timeout(shared, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, thread};

    #[test]
    fn waits_return_only_newer_frames_and_failures_end_them() {
        let slot = Arc::new(FrameSlot::default());
        assert!(matches!(slot.latest(), Err(CameraError::NoFrame)));
        assert!(matches!(
            slot.wait_newer(0, Duration::from_millis(5)),
            Err(CameraError::Timeout)
        ));

        let publisher = Arc::clone(&slot);
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            publisher.publish(1, 1, vec![1, 2, 3]);
        });
        let frame = slot.wait_newer(0, Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        assert_eq!((frame.sequence, frame.data), (1, vec![1, 2, 3]));
        assert!(matches!(
            slot.wait_newer(1, Duration::from_millis(5)),
            Err(CameraError::Timeout)
        ));

        slot.report("unplugged".into(), true);
        assert!(matches!(
            slot.wait_newer(1, Duration::from_secs(1)),
            Err(CameraError::Disconnected(_))
        ));
        assert_eq!(slot.status().as_deref(), Some("unplugged"));
    }
}
