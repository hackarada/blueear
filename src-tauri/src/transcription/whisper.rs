//! Shared Whisper.cpp transcription provider (whisper-rs).
//!
//! Loads only from an app-owned `whisper-v1` model bundle. Probe never loads
//! weights; inference runs only from `transcribe_track` after readiness.
//! Audio is decoded and windowed in [`whisper_audio`] so a long meeting never
//! lands as one giant PCM buffer in a single `full` call.

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(feature = "whisper")]
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

use crate::error::{AppError, AppResult};
use crate::transcription::model_import::WHISPER_BUNDLE_ID;
use crate::transcription::provider::{
    CancelToken, ProgressSink, ProviderContext, TranscriptionProvider,
};
use crate::transcription::types::{
    NotReadyReason, ProviderCapabilities, ProviderId, RawTrackResult, TrackRequest,
};
#[cfg(feature = "whisper")]
use crate::transcription::types::Word;
#[cfg(feature = "whisper")]
use crate::transcription::whisper_audio::{
    source_frames_to_target, PcmSource, StreamResampler, Windower, TARGET_SAMPLE_RATE,
};

pub struct WhisperProvider;

impl TranscriptionProvider for WhisperProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Whisper
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            id: ProviderId::Whisper,
            display_name: "Whisper".to_string(),
            minimum_macos: "14.4".to_string(),
            minimum_windows: "10.0.20348".to_string(),
            requires_model_import: true,
            supports_remote_speaker_labels: false,
            supports_word_timings: true,
            summary: "Runs an imported Whisper.cpp model on this computer. Track labels only; no remote speaker diarization.".to_string(),
        }
    }

    fn probe(&self, ctx: &ProviderContext) -> Option<NotReadyReason> {
        if !cfg!(feature = "whisper") {
            return Some(NotReadyReason::NotBuilt);
        }
        if find_model_file(&ctx.models_root).is_none() {
            return Some(NotReadyReason::ModelsMissing);
        }
        None
    }

    fn transcribe_track(
        &self,
        request: &TrackRequest,
        ctx: &ProviderContext,
        cancel: &CancelToken,
        progress: &ProgressSink<'_>,
    ) -> AppResult<RawTrackResult> {
        #[cfg(feature = "whisper")]
        {
            transcribe_with_whisper(request, ctx, cancel, progress)
        }
        #[cfg(not(feature = "whisper"))]
        {
            let _ = (request, ctx, cancel, progress);
            Err(AppError::transcription_provider_not_ready())
        }
    }
}

fn find_model_file(models_root: &Path) -> Option<PathBuf> {
    let bundle = models_root.join(WHISPER_BUNDLE_ID);
    if !bundle.is_dir() {
        // Also accept a directly promoted ggml dir layout.
        return None;
    }
    // Prefer an allowlisted installed bundle's ggml/*.bin
    let ggml_dir = bundle.join("ggml");
    if ggml_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&ggml_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("bin") {
                    return Some(path);
                }
            }
        }
    }
    // Fall back: any .bin under the bundle
    walk_bin(&bundle)
}

fn walk_bin(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = walk_bin(&path) {
                return Some(found);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("bin") {
            return Some(path);
        }
    }
    None
}

#[cfg(feature = "whisper")]
fn window_params(request: &TrackRequest) -> FullParams<'_, 'static> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_print_progress(false);
    params.set_print_special(false);
    params.set_print_realtime(false);
    params.set_token_timestamps(true);
    // Each window is independent: prior text must not seed hallucination loops.
    params.set_no_context(true);
    if let Some(lang) = request.language.as_deref() {
        params.set_language(Some(lang));
    }
    params
}

#[cfg(feature = "whisper")]
fn collect_words(
    state: &WhisperState,
    offset_seconds: f64,
    words: &mut Vec<Word>,
) -> AppResult<()> {
    let n = state
        .full_n_segments()
        .map_err(|_| AppError::transcription_failed())?;
    for i in 0..n {
        let text = state
            .full_get_segment_text(i)
            .map_err(|_| AppError::transcription_failed())?
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        let start = state
            .full_get_segment_t0(i)
            .map_err(|_| AppError::transcription_failed())? as f64
            / 100.0
            + offset_seconds;
        let end = state
            .full_get_segment_t1(i)
            .map_err(|_| AppError::transcription_failed())? as f64
            / 100.0
            + offset_seconds;
        let token_count = state.full_n_tokens(i).unwrap_or(0);
        if token_count > 0 {
            for t in 0..token_count {
                if let Ok(token_text) = state.full_get_token_text(i, t) {
                    let cleaned = token_text.trim();
                    if cleaned.is_empty() || cleaned.starts_with('[') {
                        continue;
                    }
                    if let Ok(data) = state.full_get_token_data(i, t) {
                        words.push(Word {
                            text: cleaned.to_string(),
                            start_seconds: data.t0 as f64 / 100.0 + offset_seconds,
                            end_seconds: data.t1 as f64 / 100.0 + offset_seconds,
                            confidence: None,
                        });
                    }
                }
            }
        } else {
            words.push(Word {
                text,
                start_seconds: start,
                end_seconds: end,
                confidence: None,
            });
        }
    }
    Ok(())
}

#[cfg(feature = "whisper")]
fn transcribe_with_whisper(
    request: &TrackRequest,
    ctx: &ProviderContext,
    cancel: &CancelToken,
    progress: &ProgressSink<'_>,
) -> AppResult<RawTrackResult> {
    cancel.check()?;
    progress(0.05);

    let model_path = find_model_file(&ctx.models_root).ok_or_else(|| {
        log::error!("whisper model missing at transcribe time");
        AppError::transcription_model_missing()
    })?;

    let model_id = model_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());

    let ctx_whisper = WhisperContext::new_with_params(
        model_path
            .to_str()
            .ok_or_else(AppError::transcription_failed)?,
        WhisperContextParameters::default(),
    )
    .map_err(|err| {
        log::error!("whisper context failed: {err}");
        AppError::transcription_failed()
    })?;

    cancel.check()?;
    progress(0.15);

    let source = PcmSource::open(&request.audio_path)?;
    let estimated_total = source
        .total_frames()
        .map(|frames| source_frames_to_target(frames, source.sample_rate()));
    let resampler = StreamResampler::new(source);

    let mut state = ctx_whisper.create_state().map_err(|err| {
        log::error!("whisper state failed: {err}");
        AppError::transcription_failed()
    })?;

    let mut words = Vec::new();
    let mut saw_audio = false;

    for window in Windower::new(resampler) {
        cancel.check()?;
        let window = window?;
        saw_audio = true;

        if !window.silent {
            let params = window_params(request);
            state.full(params, &window.samples).map_err(|err| {
                log::error!("whisper full failed: {err}");
                AppError::transcription_failed()
            })?;
            let offset = window.start_sample as f64 / f64::from(TARGET_SAMPLE_RATE);
            collect_words(&state, offset, &mut words)?;
        }

        let fraction = match estimated_total {
            Some(total) => window.end_fraction(total),
            None => 0.5,
        };
        progress(0.15 + 0.8 * fraction);
    }

    if !saw_audio {
        log::error!("whisper audio produced no samples");
        return Err(AppError::transcription_failed());
    }

    progress(1.0);
    Ok(RawTrackResult {
        words,
        speaker_spans: Vec::new(),
        model_id,
        language: request.language.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::provider::ProviderContext;

    #[test]
    fn probe_reports_models_missing_without_loading_weights() {
        let provider = WhisperProvider;
        let ctx = ProviderContext {
            models_root: PathBuf::from("/nonexistent/blueear-models"),
        };
        let expected = if cfg!(feature = "whisper") {
            NotReadyReason::ModelsMissing
        } else {
            NotReadyReason::NotBuilt
        };
        assert_eq!(provider.probe(&ctx), Some(expected));
    }
}
