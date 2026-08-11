//! The Tauri side of job reporting.
//!
//! `service.rs` deliberately knows nothing about Tauri so its contract tests
//! can drive a whole job lifecycle with no app running. This is the one small
//! adapter that turns those updates into a frontend event.

use tauri::{AppHandle, Emitter};

use crate::transcription::service::JobObserver;
use crate::transcription::types::Job;

/// Carries the full job record, which is the same shape `get_transcription_job`
/// returns. The frontend therefore has one way to read job state whether it
/// arrives by poll or by push.
pub const JOB_EVENT: &str = "transcription-job";

pub struct TauriJobObserver {
    app_handle: AppHandle,
}

impl TauriJobObserver {
    pub fn new(app_handle: AppHandle) -> Self {
        Self { app_handle }
    }
}

impl JobObserver for TauriJobObserver {
    fn job_changed(&self, job: &Job) {
        let _ = self.app_handle.emit(JOB_EVENT, job);
    }
}
