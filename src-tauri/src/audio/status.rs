//! Shared status events from any platform audio backend into `SessionManager`.

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusEvent {
    SourceTapStarted = 0,
    SourceTapStopped = 1,
    SourceSilentWarning = 2,
    SourceRestored = 3,
    SourceProcessTreeChanged = 4,
    SourceAppNotFound = 5,
    MicStarted = 6,
    MicStopped = 7,
    MicDeviceChanged = 8,
    AudioPermissionDenied = 9,
    AudioPermissionGranted = 10,
    GenericError = 11,
}

impl StatusEvent {
    pub fn from_i32(value: i32) -> Option<Self> {
        Some(match value {
            0 => Self::SourceTapStarted,
            1 => Self::SourceTapStopped,
            2 => Self::SourceSilentWarning,
            3 => Self::SourceRestored,
            4 => Self::SourceProcessTreeChanged,
            5 => Self::SourceAppNotFound,
            6 => Self::MicStarted,
            7 => Self::MicStopped,
            8 => Self::MicDeviceChanged,
            9 => Self::AudioPermissionDenied,
            10 => Self::AudioPermissionGranted,
            11 => Self::GenericError,
            _ => return None,
        })
    }
}
