//! The normalized transcript, job, and provider-capability types every
//! transcription provider must produce, regardless of which engine actually
//! ran.
//!
//! Providers differ enormously in what they can express -- Apple Speech has no
//! notion of who was speaking, FluidAudio emits sub-word token timings and
//! separate diarization segments -- so the whole point of this module is that
//! by the time anything reaches persistence, merging, or the UI, it has been
//! flattened into one shape. See
//! `docs/superpowers/specs/2026-08-07-local-transcription-design.md`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// Bumped whenever the on-disk `transcript.json` shape changes in a way older
/// readers cannot tolerate.
pub const TRANSCRIPT_SCHEMA_VERSION: u32 = 2;

/// Bumped whenever the on-disk `transcription-job.json` shape changes.
pub const JOB_SCHEMA_VERSION: u32 = 1;

/// The three provider choices, including the default of not transcribing at
/// all. `none` is a real, first-class selection rather than an absent value:
/// the plan requires that a user who never configures transcription gets WAV
/// output and nothing else, with no engine quietly promoted in the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    None,
    AppleSpeech,
    #[serde(rename = "fluidaudio")]
    FluidAudio,
    Whisper,
}

impl ProviderId {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::None => "none",
            ProviderId::AppleSpeech => "apple_speech",
            ProviderId::FluidAudio => "fluidaudio",
            ProviderId::Whisper => "whisper",
        }
    }
}

impl Default for ProviderId {
    fn default() -> Self {
        ProviderId::None
    }
}

/// The two recorded sources worth transcribing. `mixed.wav` is deliberately
/// excluded: it is a convenience artifact for listening, and transcribing it
/// would throw away the one thing Blue Ear knows for free, which is that
/// everything on the microphone track is the local user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Track {
    #[serde(alias = "teams")]
    Meeting,
    Microphone,
}

impl Track {
    pub fn as_str(self) -> &'static str {
        match self {
            Track::Meeting => "meeting",
            Track::Microphone => "microphone",
        }
    }

    /// Meeting before microphone. Used as a merge tie-break so identical inputs
    /// always produce identical output.
    pub(crate) fn order(self) -> u8 {
        match self {
            Track::Meeting => 0,
            Track::Microphone => 1,
        }
    }
}

/// Who said a segment. A closed set, because the alternative -- free-form
/// speaker strings -- invites providers to leak participant names into a file
/// the plan says must not contain them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Speaker {
    /// The local user. Every microphone-track segment, unconditionally.
    You,
    /// Meeting-track audio that was not diarized: no claim is made about who
    /// within the meeting was talking.
    MeetingAudio,
    /// A diarized remote speaker. `index` is 1-based and stable only within
    /// this one recording; Blue Ear keeps no cross-session voiceprints.
    #[serde(rename_all = "camelCase")]
    Remote { index: u32 },
    /// Diarization ran but could not attribute this span, either because no
    /// speaker segment overlapped it or because two overlapped it equally.
    Unknown,
}

impl Speaker {
    /// Label used in text and VTT exports.
    pub fn display_name(self) -> String {
        match self {
            Speaker::You => "You".to_string(),
            Speaker::MeetingAudio => "Meeting audio".to_string(),
            Speaker::Remote { index } => format!("Speaker {index}"),
            Speaker::Unknown => "Unknown speaker".to_string(),
        }
    }

    /// Secondary merge tie-break, after start time and track.
    pub(crate) fn order(self) -> (u8, u32) {
        match self {
            Speaker::You => (0, 0),
            Speaker::MeetingAudio => (1, 0),
            Speaker::Remote { index } => (2, index),
            Speaker::Unknown => (3, 0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Word {
    pub text: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// Providers that do not report per-word confidence leave this `None`
    /// rather than inventing a value.
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub track: Track,
    pub speaker: Speaker,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub text: String,
    pub words: Vec<Word>,
}

/// One track's result. Kept separate from the merged timeline so a failed
/// track can be retried without discarding a successful one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackTranscript {
    pub track: Track,
    pub provider: ProviderId,
    pub model_id: Option<String>,
    pub language: Option<String>,
    pub diarized: bool,
    pub speaker_count: u32,
    pub segments: Vec<Segment>,
}

/// The canonical persisted transcript. Text and VTT exports are derived from
/// this; they are never the source of truth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub schema_version: u32,
    pub session_id: String,
    pub provider: ProviderId,
    pub created_at: DateTime<Utc>,
    pub duration_seconds: f64,
    pub tracks: Vec<TrackTranscript>,
    /// Every track's segments interleaved into one timeline. Produced by
    /// `merge::merge_tracks`.
    pub segments: Vec<Segment>,
}

// MARK: - Jobs

/// Job lifecycle. `completed`, `cancelled`, and `failed` are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobStatus {
    Queued,
    Preparing,
    Transcribing,
    Merging,
    Completed,
    Cancelled,
    Failed,
}

impl JobStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobStatus::Completed | JobStatus::Cancelled | JobStatus::Failed
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackJob {
    pub track: Track,
    pub status: JobStatus,
    /// 0.0 to 1.0.
    pub progress: f32,
    pub error: Option<AppError>,
}

impl TrackJob {
    pub fn queued(track: Track) -> Self {
        Self {
            track,
            status: JobStatus::Queued,
            progress: 0.0,
            error: None,
        }
    }
}

/// The persisted, restart-surviving record of one transcription attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub schema_version: u32,
    pub session_id: String,
    pub provider: ProviderId,
    pub status: JobStatus,
    /// Mean of the per-track fractions, so the UI has one number to show.
    pub progress: f32,
    pub tracks: Vec<TrackJob>,
    /// Incremented by every retry, so logs and the UI can distinguish "still
    /// failing" from "failed once".
    pub attempt: u32,
    pub diarize: bool,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub error: Option<AppError>,
}

impl Job {
    pub fn new(session_id: &str, provider: ProviderId, tracks: &[Track], diarize: bool) -> Self {
        let now = Utc::now();
        Self {
            schema_version: JOB_SCHEMA_VERSION,
            session_id: session_id.to_string(),
            provider,
            status: JobStatus::Queued,
            progress: 0.0,
            tracks: tracks.iter().copied().map(TrackJob::queued).collect(),
            attempt: 1,
            diarize,
            started_at: now,
            updated_at: now,
            error: None,
        }
    }

    pub fn track_mut(&mut self, track: Track) -> Option<&mut TrackJob> {
        self.tracks.iter_mut().find(|t| t.track == track)
    }

    /// Recomputes the aggregate fraction and bumps `updated_at`. Called after
    /// every per-track change so the emitted record is always self-consistent.
    pub fn refresh(&mut self) {
        self.progress = if self.tracks.is_empty() {
            0.0
        } else {
            self.tracks.iter().map(|t| t.progress).sum::<f32>() / self.tracks.len() as f32
        };
        self.updated_at = Utc::now();
    }

    /// Tracks that still need work. Retry uses this so an already-transcribed
    /// track is never redone.
    pub fn pending_tracks(&self) -> Vec<Track> {
        self.tracks
            .iter()
            .filter(|t| t.status != JobStatus::Completed)
            .map(|t| t.track)
            .collect()
    }
}

// MARK: - Capabilities and readiness

/// Why a provider cannot be used right now. The UI turns these into
/// instructions, which is why "not ready" is never reported as a bare boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotReadyReason {
    /// Provider is `none`: transcription is intentionally off.
    NotConfigured,
    /// This OS release predates the provider's requirement.
    OsTooOld,
    /// Compiled against an SDK that does not contain the provider's API. See
    /// the `#if compiler(>=6.2)` gate on the Apple Speech adapter.
    NotBuilt,
    /// FluidAudio is selected but no valid model bundle has been imported.
    ModelsMissing,
    /// Apple Speech is available but the system has no downloaded assets for
    /// any locale it could use.
    LanguageAssetsMissing,
    /// The native adapter reported an unexpected failure during probing.
    ProbeFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    pub id: ProviderId,
    pub display_name: String,
    /// Minimum macOS version when this provider ships on macOS. Empty when the
    /// provider is not available on macOS.
    pub minimum_macos: String,
    /// Minimum Windows version (build label) when this provider ships on
    /// Windows. Empty when the provider is not available on Windows.
    #[serde(default)]
    pub minimum_windows: String,
    /// Whether the user must manually import a model bundle before this
    /// provider can run.
    pub requires_model_import: bool,
    pub supports_remote_speaker_labels: bool,
    pub supports_word_timings: bool,
    /// One short sentence for the settings comparison card.
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    #[serde(flatten)]
    pub capabilities: ProviderCapabilities,
    pub ready: bool,
    pub not_ready_reason: Option<NotReadyReason>,
}

// MARK: - Provider request and raw result

/// What a provider is asked to do for one track. `audio_path` is always
/// resolved server-side from a session ID; no frontend-supplied path reaches
/// this struct.
#[derive(Debug, Clone)]
pub struct TrackRequest {
    pub track: Track,
    pub audio_path: std::path::PathBuf,
    /// Only ever true for the Meeting track, and only when the selected provider
    /// supports it.
    pub diarize: bool,
    pub language: Option<String>,
}

/// A diarization span, before words are attached to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerSpan {
    /// Provider-local speaker key. Mapped to a 1-based `Speaker::Remote` index
    /// in order of first appearance, so the UI never sees an engine's internal
    /// cluster IDs.
    pub speaker_key: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
}

/// What a provider actually returns: words, and optionally the diarization it
/// managed to produce. Turning this into segments with speakers is Rust's job,
/// in `merge.rs`, where it can be tested without a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawTrackResult {
    pub words: Vec<Word>,
    #[serde(default)]
    pub speaker_spans: Vec<SpeakerSpan>,
    pub model_id: Option<String>,
    pub language: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ids_serialize_to_the_documented_wire_names() {
        for (id, expected) in [
            (ProviderId::None, "none"),
            (ProviderId::AppleSpeech, "apple_speech"),
            (ProviderId::FluidAudio, "fluidaudio"),
            (ProviderId::Whisper, "whisper"),
        ] {
            let json = serde_json::to_value(id).unwrap();
            assert_eq!(json, serde_json::json!(expected));
            assert_eq!(id.as_str(), expected);
            let round_tripped: ProviderId = serde_json::from_value(json).unwrap();
            assert_eq!(round_tripped, id);
        }
    }

    /// Same class of bug as `SessionState`'s camelCase regression test: an
    /// enum-level `rename_all` does not reach fields inside struct variants,
    /// so `Speaker::Remote`'s payload needs its own attribute.
    #[test]
    fn speaker_remote_variant_serializes_as_a_tagged_camel_case_object() {
        let json = serde_json::to_value(Speaker::Remote { index: 2 }).unwrap();
        assert_eq!(json["kind"], "remote");
        assert_eq!(json["index"], 2);
    }

    #[test]
    fn speaker_display_names_are_session_local_and_generic() {
        assert_eq!(Speaker::You.display_name(), "You");
        assert_eq!(Speaker::MeetingAudio.display_name(), "Meeting audio");
        assert_eq!(Speaker::Remote { index: 3 }.display_name(), "Speaker 3");
        assert_eq!(Speaker::Unknown.display_name(), "Unknown speaker");
    }

    #[test]
    fn job_progress_is_the_mean_of_its_tracks() {
        let mut job = Job::new(
            "session-a",
            ProviderId::FluidAudio,
            &[Track::Meeting, Track::Microphone],
            true,
        );
        job.track_mut(Track::Meeting).unwrap().progress = 1.0;
        job.track_mut(Track::Microphone).unwrap().progress = 0.5;
        job.refresh();
        assert!((job.progress - 0.75).abs() < 1e-6);
    }

    #[test]
    fn pending_tracks_excludes_already_completed_tracks() {
        let mut job = Job::new(
            "session-b",
            ProviderId::FluidAudio,
            &[Track::Meeting, Track::Microphone],
            false,
        );
        let teams = job.track_mut(Track::Meeting).unwrap();
        teams.status = JobStatus::Completed;
        teams.progress = 1.0;
        assert_eq!(job.pending_tracks(), vec![Track::Microphone]);
    }

    #[test]
    fn track_and_speaker_orderings_are_total_and_stable() {
        assert!(Track::Meeting.order() < Track::Microphone.order());
        let mut speakers = [
            Speaker::Unknown,
            Speaker::Remote { index: 2 },
            Speaker::You,
            Speaker::Remote { index: 1 },
            Speaker::MeetingAudio,
        ];
        speakers.sort_by_key(|s| s.order());
        assert_eq!(
            speakers,
            [
                Speaker::You,
                Speaker::MeetingAudio,
                Speaker::Remote { index: 1 },
                Speaker::Remote { index: 2 },
                Speaker::Unknown,
            ]
        );
    }
}
