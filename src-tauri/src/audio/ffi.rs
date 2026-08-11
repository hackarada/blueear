//! The narrow C ABI boundary to the linked `BlueEarAudio` Swift package
//! (see `src-tauri/native/BlueEarAudio`). This is the only file in the Rust
//! codebase that knows about the native symbol names; everything else talks
//! to the safe `NativeAudioEngine` wrapper below.
//!
//! SECURITY-REVIEW: this module is the boundary where native macOS device
//! access (Core Audio process taps + microphone) is exposed to the rest of
//! the app. All raw-pointer handling here is scoped to translating fixed
//! POD audio frames; no arbitrary memory, strings, or objects cross this
//! boundary. Meeting-app identity crosses as `MeetingAppId` Int32 only.

use std::ffi::c_void;
use std::os::raw::c_float;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use super::meeting_app::MeetingAppId;
use super::ring::RingProducer;
use super::status::StatusEvent;

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSource {
    Meeting = 0,
    Microphone = 1,
}

type AudioCallbackC =
    extern "C" fn(*mut c_void, i32, *const c_float, u32, u32, f64, u64);
type StatusCallbackC = extern "C" fn(*mut c_void, i32, i32);

extern "C" {
    fn blueear_audio_init(
        audio_callback: AudioCallbackC,
        status_callback: StatusCallbackC,
        user_data: *mut c_void,
    );
    fn blueear_is_meeting_app_running(app: i32) -> i32;
    fn blueear_is_meeting_app_installed(app: i32) -> i32;
    fn blueear_macos_version_supported() -> i32;
    fn blueear_probe_audio_permission() -> i32;
    fn blueear_microphone_input_available() -> i32;
    fn blueear_start_meeting_capture(app: i32) -> i32;
    fn blueear_stop_meeting_capture();
    fn blueear_start_microphone_capture() -> i32;
    fn blueear_stop_microphone_capture();
    fn blueear_shutdown();
}

struct EngineInner {
    meeting_tx: Mutex<Option<RingProducer>>,
    mic_tx: Mutex<Option<RingProducer>>,
    status_tx: mpsc::Sender<(StatusEvent, i32)>,
}

extern "C" fn trampoline_audio(
    user_data: *mut c_void,
    source: i32,
    samples: *const c_float,
    frame_count: u32,
    channel_count: u32,
    sample_rate: f64,
    host_time_ns: u64,
) {
    if user_data.is_null() || samples.is_null() || frame_count == 0 || channel_count == 0 {
        return;
    }
    // SAFETY: `user_data` is the raw pointer this module handed to Swift in
    // `NativeAudioEngine::new`, kept alive for the process's lifetime via
    // `Arc::into_raw` until `NativeAudioEngine::drop` reclaims it. `samples`
    // points to `frame_count * channel_count` valid f32s for the duration
    // of this call only (native side owns the buffer).
    let inner = unsafe { &*(user_data as *const EngineInner) };
    let len = (frame_count as usize) * (channel_count as usize);
    let slice = unsafe { std::slice::from_raw_parts(samples, len) };

    let target = match source {
        s if s == AudioSource::Meeting as i32 => &inner.meeting_tx,
        s if s == AudioSource::Microphone as i32 => &inner.mic_tx,
        _ => return,
    };
    if let Ok(mut guard) = target.lock() {
        if let Some(producer) = guard.as_mut() {
            producer.push(slice, frame_count, channel_count, sample_rate, host_time_ns);
        }
    }
}

extern "C" fn trampoline_status(user_data: *mut c_void, event: i32, detail: i32) {
    if user_data.is_null() {
        return;
    }
    let inner = unsafe { &*(user_data as *const EngineInner) };
    if let Some(event) = StatusEvent::from_i32(event) {
        let _ = inner.status_tx.send((event, detail));
    }
}

/// Safe wrapper around the native capture engine. Constructed once for the
/// app's lifetime; individual recordings call `start_meeting_capture` /
/// `start_microphone_capture` with a fresh ring producer each time.
pub struct NativeAudioEngine {
    inner: *const EngineInner,
}

// SAFETY: `EngineInner` only exposes synchronized access (Mutex-guarded
// producers, an mpsc sender) and the raw pointer is never mutated after
// construction, so sharing `NativeAudioEngine` across threads is sound.
unsafe impl Send for NativeAudioEngine {}
unsafe impl Sync for NativeAudioEngine {}

impl NativeAudioEngine {
    pub fn new() -> (Self, mpsc::Receiver<(StatusEvent, i32)>) {
        let (status_tx, status_rx) = mpsc::channel();
        let inner = Arc::new(EngineInner {
            meeting_tx: Mutex::new(None),
            mic_tx: Mutex::new(None),
            status_tx,
        });
        let raw = Arc::into_raw(inner);
        unsafe {
            blueear_audio_init(trampoline_audio, trampoline_status, raw as *mut c_void);
        }
        (Self { inner: raw }, status_rx)
    }

    fn inner(&self) -> &EngineInner {
        // SAFETY: valid until `drop`, which only runs once and only after
        // all other references to `self` are gone.
        unsafe { &*self.inner }
    }

    pub fn macos_version_supported(&self) -> bool {
        unsafe { blueear_macos_version_supported() != 0 }
    }

    pub fn is_meeting_app_running(&self, app: MeetingAppId) -> bool {
        unsafe { blueear_is_meeting_app_running(app.as_i32()) != 0 }
    }

    pub fn is_meeting_app_installed(&self, app: MeetingAppId) -> bool {
        unsafe { blueear_is_meeting_app_installed(app.as_i32()) != 0 }
    }

    pub fn probe_audio_permission(&self) -> bool {
        unsafe { blueear_probe_audio_permission() != 0 }
    }

    pub fn microphone_input_available(&self) -> bool {
        unsafe { blueear_microphone_input_available() != 0 }
    }

    pub fn start_meeting_capture(
        &self,
        app: MeetingAppId,
        producer: RingProducer,
    ) -> Result<(), i32> {
        *self.inner().meeting_tx.lock().expect("meeting_tx poisoned") = Some(producer);
        let code = unsafe { blueear_start_meeting_capture(app.as_i32()) };
        if code != 0 {
            *self.inner().meeting_tx.lock().expect("meeting_tx poisoned") = None;
            return Err(code);
        }
        Ok(())
    }

    pub fn stop_meeting_capture(&self) {
        unsafe { blueear_stop_meeting_capture() };
        *self.inner().meeting_tx.lock().expect("meeting_tx poisoned") = None;
    }

    pub fn start_microphone_capture(&self, producer: RingProducer) -> Result<(), i32> {
        *self.inner().mic_tx.lock().expect("mic_tx poisoned") = Some(producer);
        let code = unsafe { blueear_start_microphone_capture() };
        if code != 0 {
            *self.inner().mic_tx.lock().expect("mic_tx poisoned") = None;
            return Err(code);
        }
        Ok(())
    }

    pub fn stop_microphone_capture(&self) {
        unsafe { blueear_stop_microphone_capture() };
        *self.inner().mic_tx.lock().expect("mic_tx poisoned") = None;
    }
}

impl Drop for NativeAudioEngine {
    fn drop(&mut self) {
        unsafe {
            blueear_shutdown();
            // SAFETY: reclaims exactly the strong reference leaked in
            // `new` via `Arc::into_raw`; runs exactly once since `Drop`
            // only fires once for this value.
            drop(Arc::from_raw(self.inner));
        }
    }
}
