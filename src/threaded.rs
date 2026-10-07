//! A camera whose native stream is read on a dedicated thread (Linux and Windows).
//!
//! The thread opens the device itself, so thread-affine native state (COM apartments) lives
//! entirely on it, and reports the open result back before streaming.

use crate::{
    CameraDevice, CameraError, CameraFormat, Result, RgbFrame, convert::Rgb, frames::FrameSlot,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub(crate) enum StreamError {
    /// One frame was unusable; keep streaming.
    Frame(String),
    /// The device is gone or the stream failed.
    Fatal(String),
}

pub(crate) trait Stream {
    /// Wait up to `timeout` for the next frame, returning packed RGB or `None` on timeout.
    fn next_frame(&mut self, timeout: Duration) -> std::result::Result<Option<Rgb>, StreamError>;
}

/// Unblocks a `next_frame` call from another thread during close.
pub(crate) type Interrupt = Box<dyn Fn() + Send>;

pub(crate) struct ThreadedCamera {
    device: CameraDevice,
    format: CameraFormat,
    slot: Arc<FrameSlot>,
    stop: Arc<AtomicBool>,
    interrupt: Option<Interrupt>,
    worker: Option<JoinHandle<()>>,
    returned: u64,
}

const POLL: Duration = Duration::from_millis(100);

impl ThreadedCamera {
    pub(crate) fn spawn<S: Stream>(
        device: CameraDevice,
        open: impl FnOnce() -> Result<(S, CameraFormat, Option<Interrupt>)> + Send + 'static,
    ) -> Result<Self> {
        let slot = Arc::new(FrameSlot::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (opened_tx, opened_rx) = mpsc::channel();
        let (worker_slot, worker_stop) = (Arc::clone(&slot), Arc::clone(&stop));
        let worker = thread::Builder::new()
            .name("camlib-capture".into())
            .spawn(move || {
                let mut stream = match open() {
                    Ok((stream, format, interrupt)) => {
                        let _ = opened_tx.send(Ok((format, interrupt)));
                        stream
                    }
                    Err(error) => {
                        let _ = opened_tx.send(Err(error));
                        return;
                    }
                };
                while !worker_stop.load(Ordering::Acquire) {
                    match stream.next_frame(POLL) {
                        Ok(Some((width, height, rgb))) => worker_slot.publish(width, height, rgb),
                        Ok(None) => {}
                        Err(StreamError::Frame(message)) => worker_slot.report(message, false),
                        Err(StreamError::Fatal(message)) => {
                            worker_slot.report(message, true);
                            break;
                        }
                    }
                }
            })
            .map_err(|error| CameraError::Native(error.to_string()))?;
        let (format, interrupt) = match opened_rx.recv() {
            Ok(Ok(opened)) => opened,
            Ok(Err(error)) => {
                let _ = worker.join();
                return Err(error);
            }
            Err(_) => {
                let _ = worker.join();
                return Err(CameraError::Native(
                    "camera thread exited while opening".into(),
                ));
            }
        };
        Ok(Self {
            device,
            format,
            slot,
            stop,
            interrupt,
            worker: Some(worker),
            returned: 0,
        })
    }

    pub(crate) fn device(&self) -> &CameraDevice {
        &self.device
    }

    pub(crate) fn format(&self) -> CameraFormat {
        self.format
    }

    pub(crate) fn status(&self) -> Option<String> {
        self.slot.status()
    }

    pub(crate) fn frame_rgb(&mut self) -> Result<RgbFrame> {
        let frame = self.slot.latest()?;
        self.accept(&frame);
        Ok(frame)
    }

    pub(crate) fn wait_frame(&mut self, timeout: Duration) -> Result<RgbFrame> {
        let frame = self.slot.wait_newer(self.returned, timeout)?;
        self.accept(&frame);
        Ok(frame)
    }

    pub(crate) fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(interrupt) = self.interrupt.take() {
            interrupt();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn accept(&mut self, frame: &RgbFrame) {
        self.returned = frame.sequence;
        self.format.width = frame.width;
        self.format.height = frame.height;
    }
}

impl Drop for ThreadedCamera {
    fn drop(&mut self) {
        self.close();
    }
}
