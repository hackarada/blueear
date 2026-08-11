//! Platform audio backend selected at compile time.
//!
//! macOS keeps the Swift Core Audio engine; Windows uses WASAPI process
//! loopback. Session code talks only to [`PlatformAudioEngine`].

use std::sync::mpsc;

use super::meeting_app::MeetingAppId;
use super::ring::RingProducer;
use super::status::StatusEvent;

#[cfg(target_os = "macos")]
use super::ffi::NativeAudioEngine;

#[cfg(target_os = "windows")]
use super::windows::WindowsAudioEngine;

/// Concrete engine for this target OS.
pub struct PlatformAudioEngine {
    #[cfg(target_os = "macos")]
    inner: NativeAudioEngine,
    #[cfg(target_os = "windows")]
    inner: WindowsAudioEngine,
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    _unsupported: (),
}

impl PlatformAudioEngine {
    pub fn new() -> (Self, mpsc::Receiver<(StatusEvent, i32)>) {
        #[cfg(target_os = "macos")]
        {
            let (inner, rx) = NativeAudioEngine::new();
            (Self { inner }, rx)
        }
        #[cfg(target_os = "windows")]
        {
            let (inner, rx) = WindowsAudioEngine::new();
            (Self { inner }, rx)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let (_tx, rx) = mpsc::channel();
            (Self { _unsupported: () }, rx)
        }
    }

    pub fn os_supported(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.inner.macos_version_supported()
        }
        #[cfg(target_os = "windows")]
        {
            self.inner.os_supported()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            false
        }
    }

    pub fn is_meeting_app_running(&self, app: MeetingAppId) -> bool {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.inner.is_meeting_app_running(app)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = app;
            false
        }
    }

    pub fn is_meeting_app_installed(&self, app: MeetingAppId) -> bool {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.inner.is_meeting_app_installed(app)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = app;
            false
        }
    }

    pub fn probe_audio_permission(&self) -> bool {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.inner.probe_audio_permission()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            false
        }
    }

    pub fn microphone_input_available(&self) -> bool {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.inner.microphone_input_available()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            false
        }
    }

    pub fn start_meeting_capture(
        &self,
        app: MeetingAppId,
        producer: RingProducer,
    ) -> Result<(), i32> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.inner.start_meeting_capture(app, producer)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (app, producer);
            Err(-1)
        }
    }

    pub fn stop_meeting_capture(&self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.inner.stop_meeting_capture();
        }
    }

    pub fn start_microphone_capture(&self, producer: RingProducer) -> Result<(), i32> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.inner.start_microphone_capture(producer)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = producer;
            Err(-1)
        }
    }

    pub fn stop_microphone_capture(&self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.inner.stop_microphone_capture();
        }
    }
}
