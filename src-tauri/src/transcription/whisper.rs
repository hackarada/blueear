//! Shared Whisper.cpp transcription provider (whisper-rs).
//!
//! Loads only from an app-owned `whisper-v1` model bundle. Probe never loads
//! weights; inference runs only from `transcribe_track` after readiness.

use std::fs;
use std::path::{Path, PathBuf};

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
fn transcribe_with_whisper(
    request: &TrackRequest,
    ctx: &ProviderContext,
    cancel: &CancelToken,
    progress: &ProgressSink<'_>,
) -> AppResult<RawTrackResult> {
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

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
        model_path.to_str().ok_or_else(AppError::transcription_failed)?,
        WhisperContextParameters::default(),
    )
    .map_err(|err| {
        log::error!("whisper context failed: {err}");
        AppError::transcription_failed()
    })?;

    cancel.check()?;
    progress(0.15);

    let samples = load_wav_mono_16k(&request.audio_path)?;
    cancel.check()?;
    progress(0.25);

    let mut state = ctx_whisper.create_state().map_err(|err| {
        log::error!("whisper state failed: {err}");
        AppError::transcription_failed()
    })?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_print_progress(false);
    params.set_print_special(false);
    params.set_print_realtime(false);
    params.set_token_timestamps(true);
    if let Some(lang) = request.language.as_deref() {
        params.set_language(Some(lang));
    }

    state.full(params, &samples).map_err(|err| {
        log::error!("whisper full failed: {err}");
        AppError::transcription_failed()
    })?;

    cancel.check()?;
    progress(0.85);

    let n = state.full_n_segments().map_err(|_| AppError::transcription_failed())?;
    let mut words = Vec::new();
    for i in 0..n {
        cancel.check()?;
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
            / 100.0;
        let end = state
            .full_get_segment_t1(i)
            .map_err(|_| AppError::transcription_failed())? as f64
            / 100.0;
        // Prefer token-level timings when available.
        let token_count = state
            .full_n_tokens(i)
            .unwrap_or(0);
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
                            start_seconds: data.t0 as f64 / 100.0,
                            end_seconds: data.t1 as f64 / 100.0,
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
        progress(0.85 + 0.1 * ((i + 1) as f32 / n.max(1) as f32));
    }

    progress(1.0);
    Ok(RawTrackResult {
        words,
        speaker_spans: Vec::new(),
        model_id,
        language: request.language.clone(),
    })
}

/// Whisper.cpp expects mono f32 PCM at 16 kHz.
#[cfg(feature = "whisper")]
fn load_wav_mono_16k(path: &Path) -> AppResult<Vec<f32>> {
    let mut reader = hound::WavReader::open(path).map_err(|err| {
        log::error!("whisper wav open failed: {err}");
        AppError::transcription_failed()
    })?;
    let spec = reader.spec();
    let samples: Result<Vec<f32>, _> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect(),
        hound::SampleFormat::Int => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / (1i32 << (spec.bits_per_sample - 1)) as f32))
            .collect(),
    };
    let interleaved = samples.map_err(|err| {
        log::error!("whisper wav read failed: {err}");
        AppError::transcription_failed()
    })?;

    let channels = spec.channels.max(1) as usize;
    let mut mono = Vec::with_capacity(interleaved.len() / channels + 1);
    for frame in interleaved.chunks(channels) {
        let sum: f32 = frame.iter().copied().sum();
        mono.push(sum / channels as f32);
    }

    if spec.sample_rate == 16_000 {
        return Ok(mono);
    }

    // Linear resample to 16 kHz.
    let ratio = 16_000.0 / spec.sample_rate as f64;
    let out_len = ((mono.len() as f64) * ratio).round().max(1.0) as usize;
    let mut out = vec![0.0f32; out_len];
    for (i, sample) in out.iter_mut().enumerate() {
        let src = i as f64 / ratio;
        let i0 = src.floor() as usize;
        let i1 = (i0 + 1).min(mono.len().saturating_sub(1));
        let frac = (src - i0 as f64) as f32;
        let a = mono.get(i0).copied().unwrap_or(0.0);
        let b = mono.get(i1).copied().unwrap_or(a);
        *sample = a + (b - a) * frac;
    }
    Ok(out)
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
        assert_eq!(provider.probe(&ctx), Some(NotReadyReason::ModelsMissing));
    }
}
