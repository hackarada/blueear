//! Optional, post-meeting, local-only transcription.
//!
//! Nothing in this module runs unless the user has explicitly chosen a
//! provider and that provider has passed its readiness check. Recording is
//! entirely independent of it: a user who never opens the transcription
//! settings gets WAV files and no transcript, which is the designed default,
//! not a degraded state.
//!
//! The module boundaries follow the design spec at
//! `docs/superpowers/specs/2026-08-07-local-transcription-design.md`:
//!
//! - [`types`] — the normalized transcript, job, and capability shapes
//! - [`provider`] — the engine contract and the registry that owns instances
//! - [`merge`] — pure speaker alignment and deterministic track merging
//! - [`store`] — atomic persistence, preferences, recovery, exports
//! - [`service`] — job orchestration, the only writer of job state
//! - [`model_import`] — validated import of user-supplied model bundles
//! - [`native`] — the C ABI to the Swift provider adapters
//! - [`events`] — the Tauri adapter for job updates

pub mod events;
pub mod merge;
pub mod model_import;
#[cfg(target_os = "macos")]
pub mod native;
pub mod picker;
pub mod provider;
pub mod service;
pub mod store;
pub mod types;
pub mod whisper;

use std::sync::Arc;

pub use service::TranscriptionService;

/// Every provider the shipping app knows about, in the order the settings
/// screen lists them. `none` is first because it is the default and a
/// legitimate long-term choice, not a placeholder.
///
/// Constructing a provider is free: none of them touch a model, a system
/// framework, or a language asset until `probe` or `transcribe_track` is
/// called, which is what lets FluidAudio and Whisper ship inert in every build.
pub fn production_registry() -> provider::ProviderRegistry {
    let mut providers: Vec<Arc<dyn provider::TranscriptionProvider>> =
        vec![Arc::new(provider::NoneProvider)];

    #[cfg(target_os = "macos")]
    {
        providers.push(Arc::new(native::AppleSpeechProvider));
        providers.push(Arc::new(native::FluidAudioProvider));
    }

    providers.push(Arc::new(whisper::WhisperProvider));
    provider::ProviderRegistry::new(providers)
}
