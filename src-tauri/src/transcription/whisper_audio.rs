//! Streaming decode, resample, and silence-aware windowing for Whisper.
//!
//! Whisper's context is about 30 seconds. Decoding a whole meeting into one
//! PCM buffer risks hundreds of megabytes of RAM and hallucination loops on
//! long silences. This module streams compressed audio into mono f32 blocks,
//! resamples them to 16 kHz, and cuts them into windows at quiet points.

use std::fs::File;
use std::io::ErrorKind;
use std::path::Path;

use hound::{SampleFormat, WavReader};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::error::{AppError, AppResult};

/// Whisper.cpp expects mono f32 at this rate.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Maximum window length Whisper should see. Its native context is ~30 s.
const MAX_WINDOW_SAMPLES: usize = TARGET_SAMPLE_RATE as usize * 30;

/// Start looking for a quiet cut once the buffer reaches this length.
const LOOK_FOR_CUT_SAMPLES: usize = TARGET_SAMPLE_RATE as usize * 25;

/// Search for the quietest cut inside this trailing region.
const CUT_SEARCH_SAMPLES: usize = TARGET_SAMPLE_RATE as usize * 5;

/// RMS is measured over this many samples (20 ms at 16 kHz).
const FRAME_SAMPLES: usize = TARGET_SAMPLE_RATE as usize / 50;

/// A window whose peak frame RMS stays below this is treated as silence
/// (~-50 dBFS) and skipped before inference.
const SILENCE_RMS: f32 = 0.003_162_277_6;

/// One decoded mono block at the source sample rate.
pub struct PcmBlock {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// Streaming mono PCM from a track file (`.m4a` / `.aac` / `.mp4` or WAV).
pub struct PcmSource {
    inner: PcmSourceInner,
    sample_rate: u32,
    total_frames: Option<u64>,
}

enum PcmSourceInner {
    Aac(AacSource),
    Wav(WavSource),
}

impl PcmSource {
    pub fn open(path: &Path) -> AppResult<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "m4a" | "mp4" | "aac" => Self::open_aac(path),
            _ => Self::open_wav(path),
        }
    }

    fn open_aac(path: &Path) -> AppResult<Self> {
        let file = File::open(path).map_err(|err| {
            log::error!("whisper aac open failed: {err}");
            AppError::transcription_failed()
        })?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|err| {
                log::error!("whisper aac probe failed: {err}");
                AppError::transcription_failed()
            })?;

        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| {
                log::error!("whisper aac has no audio track");
                AppError::transcription_failed()
            })?;
        let track_id = track.id;
        let sample_rate = track.codec_params.sample_rate.unwrap_or(48_000);
        let total_frames = track.codec_params.n_frames;
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|err| {
                log::error!("whisper aac decoder failed: {err}");
                AppError::transcription_failed()
            })?;

        Ok(Self {
            inner: PcmSourceInner::Aac(AacSource {
                format,
                decoder,
                track_id,
            }),
            sample_rate,
            total_frames,
        })
    }

    fn open_wav(path: &Path) -> AppResult<Self> {
        let reader = WavReader::open(path).map_err(|err| {
            log::error!("whisper wav open failed: {err}");
            AppError::transcription_failed()
        })?;
        let spec = reader.spec();
        let channels = spec.channels.max(1) as usize;
        let total_frames = Some(reader.len() as u64 / channels as u64);
        Ok(Self {
            inner: PcmSourceInner::Wav(WavSource { reader, channels }),
            sample_rate: spec.sample_rate,
            total_frames,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Source-rate frame count when the container or WAV header reports it.
    pub fn total_frames(&self) -> Option<u64> {
        self.total_frames
    }
}

impl Iterator for PcmSource {
    type Item = AppResult<PcmBlock>;

    fn next(&mut self) -> Option<Self::Item> {
        let rate = self.sample_rate;
        let result = match &mut self.inner {
            PcmSourceInner::Aac(src) => src.next_block(),
            PcmSourceInner::Wav(src) => src.next_block(),
        };
        match result {
            Ok(None) => None,
            Ok(Some(samples)) => {
                if samples.is_empty() {
                    return self.next();
                }
                Some(Ok(PcmBlock {
                    samples,
                    sample_rate: rate,
                }))
            }
            Err(err) => Some(Err(err)),
        }
    }
}

struct AacSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
}

impl AacSource {
    fn next_block(&mut self) -> AppResult<Option<Vec<f32>>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::ResetRequired) => return Ok(None),
                Err(SymphoniaError::IoError(err)) if err.kind() == ErrorKind::UnexpectedEof => {
                    return Ok(None);
                }
                Err(err) => {
                    log::error!("whisper aac packet failed: {err}");
                    return Err(AppError::transcription_failed());
                }
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                    sample_buf.copy_interleaved_ref(decoded);
                    let channels = spec.channels.count().max(1);
                    let interleaved = sample_buf.samples();
                    let mut mono = Vec::with_capacity(interleaved.len() / channels + 1);
                    if channels == 1 {
                        mono.extend_from_slice(interleaved);
                    } else {
                        for frame in interleaved.chunks(channels) {
                            let sum: f32 = frame.iter().sum();
                            mono.push(sum / channels as f32);
                        }
                    }
                    return Ok(Some(mono));
                }
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(err) => {
                    log::error!("whisper aac decode failed: {err}");
                    return Err(AppError::transcription_failed());
                }
            }
        }
    }
}

struct WavSource {
    reader: WavReader<std::io::BufReader<File>>,
    channels: usize,
}

impl WavSource {
    /// Read about 100 ms of source audio per block so the windower can cut
    /// without waiting for the whole file.
    const FRAMES_PER_BLOCK: usize = 4_800;

    fn next_block(&mut self) -> AppResult<Option<Vec<f32>>> {
        let want = Self::FRAMES_PER_BLOCK * self.channels;
        let mut interleaved = Vec::with_capacity(want);
        match self.reader.spec().sample_format {
            SampleFormat::Float => {
                for sample in self.reader.samples::<f32>().take(want) {
                    interleaved.push(sample.map_err(|err| {
                        log::error!("whisper wav read failed: {err}");
                        AppError::transcription_failed()
                    })?);
                }
            }
            SampleFormat::Int => {
                let bits = self.reader.spec().bits_per_sample;
                let scale = (1i32 << (bits - 1)) as f32;
                for sample in self.reader.samples::<i32>().take(want) {
                    let v = sample.map_err(|err| {
                        log::error!("whisper wav read failed: {err}");
                        AppError::transcription_failed()
                    })?;
                    interleaved.push(v as f32 / scale);
                }
            }
        }
        if interleaved.is_empty() {
            return Ok(None);
        }
        let mut mono = Vec::with_capacity(interleaved.len() / self.channels + 1);
        for frame in interleaved.chunks(self.channels) {
            if frame.len() < self.channels {
                break;
            }
            let sum: f32 = frame.iter().copied().sum();
            mono.push(sum / self.channels as f32);
        }
        Ok(Some(mono))
    }
}

/// Linear resampler that keeps its fractional read position across blocks so
/// output stays continuous at block boundaries.
pub struct StreamResampler<I> {
    source: I,
    input_rate: Option<u32>,
    /// Fractional index into the concatenated input stream.
    position: f64,
    /// Samples already consumed from the front of `pending`.
    consumed: u64,
    pending: Vec<f32>,
    finished: bool,
}

impl<I> StreamResampler<I>
where
    I: Iterator<Item = AppResult<PcmBlock>>,
{
    pub fn new(source: I) -> Self {
        Self {
            source,
            input_rate: None,
            position: 0.0,
            consumed: 0,
            pending: Vec::new(),
            finished: false,
        }
    }

    fn pull_more(&mut self) -> AppResult<bool> {
        if self.finished {
            return Ok(false);
        }
        match self.source.next() {
            Some(Ok(block)) => {
                if let Some(rate) = self.input_rate {
                    if rate != block.sample_rate {
                        log::error!(
                            "whisper sample rate changed mid-stream ({rate} -> {})",
                            block.sample_rate
                        );
                        return Err(AppError::transcription_failed());
                    }
                } else {
                    self.input_rate = Some(block.sample_rate);
                }
                self.pending.extend_from_slice(&block.samples);
                Ok(true)
            }
            Some(Err(err)) => Err(err),
            None => {
                self.finished = true;
                Ok(false)
            }
        }
    }

    fn drop_consumed(&mut self) {
        let drop = self.consumed.min(self.pending.len() as u64) as usize;
        if drop > 0 {
            self.pending.drain(..drop);
            self.position -= drop as f64;
            self.consumed -= drop as u64;
        }
    }
}

impl<I> Iterator for StreamResampler<I>
where
    I: Iterator<Item = AppResult<PcmBlock>>,
{
    type Item = AppResult<Vec<f32>>;

    fn next(&mut self) -> Option<Self::Item> {
        // Produce about 100 ms of 16 kHz audio per call.
        const OUT_CHUNK: usize = TARGET_SAMPLE_RATE as usize / 10;

        loop {
            if self.input_rate.is_none() {
                match self.pull_more() {
                    Ok(true) => {}
                    Ok(false) => return None,
                    Err(err) => return Some(Err(err)),
                }
                continue;
            }

            let rate = self.input_rate.unwrap();
            if rate == TARGET_SAMPLE_RATE {
                if self.pending.is_empty() {
                    match self.pull_more() {
                        Ok(true) => continue,
                        Ok(false) => return None,
                        Err(err) => return Some(Err(err)),
                    }
                }
                let take = self.pending.len().min(OUT_CHUNK);
                let out: Vec<f32> = self.pending.drain(..take).collect();
                return Some(Ok(out));
            }

            let ratio = TARGET_SAMPLE_RATE as f64 / rate as f64;
            // Need one sample past the last interpolated index.
            let needed_end = self.position + OUT_CHUNK as f64 / ratio + 1.0;
            while (self.pending.len() as f64) < needed_end && !self.finished {
                match self.pull_more() {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(err) => return Some(Err(err)),
                }
            }

            if self.pending.is_empty() {
                return None;
            }

            let available = self.pending.len() as f64;
            let max_out = if self.finished {
                // Last sample is held for the fractional tail.
                (((available - 1.0 - self.position).max(0.0)) * ratio).floor() as usize + 1
            } else {
                (((available - 1.0 - self.position).max(0.0)) * ratio).floor() as usize
            };
            if max_out == 0 {
                if self.finished {
                    return None;
                }
                match self.pull_more() {
                    Ok(true) => continue,
                    Ok(false) => return None,
                    Err(err) => return Some(Err(err)),
                }
            }

            let out_len = max_out.min(OUT_CHUNK);
            let mut out = Vec::with_capacity(out_len);
            for _ in 0..out_len {
                let src = self.position;
                let i0 = src.floor() as usize;
                let i1 = (i0 + 1).min(self.pending.len().saturating_sub(1));
                let frac = (src - i0 as f64) as f32;
                let a = self.pending.get(i0).copied().unwrap_or(0.0);
                let b = self.pending.get(i1).copied().unwrap_or(a);
                out.push(a + (b - a) * frac);
                self.position += 1.0 / ratio;
            }
            self.consumed = self.position.floor() as u64;
            self.drop_consumed();
            return Some(Ok(out));
        }
    }
}

/// One window of 16 kHz mono PCM ready for Whisper.
#[derive(Debug, Clone)]
pub struct Window {
    pub start_sample: u64,
    pub samples: Vec<f32>,
    pub silent: bool,
}

impl Window {
    pub fn end_sample(&self) -> u64 {
        self.start_sample + self.samples.len() as u64
    }

    pub fn end_fraction(&self, total_samples: u64) -> f32 {
        if total_samples == 0 {
            return 1.0;
        }
        (self.end_sample() as f32 / total_samples as f32).clamp(0.0, 1.0)
    }
}

/// Buffers 16 kHz samples and emits windows of at most 30 s, cutting at the
/// quietest 20 ms frame in the trailing 5 s once the buffer reaches 25 s.
pub struct Windower<I> {
    source: I,
    buffer: Vec<f32>,
    next_start: u64,
    finished: bool,
}

impl<I> Windower<I>
where
    I: Iterator<Item = AppResult<Vec<f32>>>,
{
    pub fn new(source: I) -> Self {
        Self {
            source,
            buffer: Vec::new(),
            next_start: 0,
            finished: false,
        }
    }

    fn is_silent(samples: &[f32]) -> bool {
        if samples.is_empty() {
            return true;
        }
        let mut peak = 0.0f32;
        for frame in samples.chunks(FRAME_SAMPLES) {
            let sum_sq: f32 = frame.iter().map(|s| s * s).sum();
            let rms = (sum_sq / frame.len() as f32).sqrt();
            peak = peak.max(rms);
        }
        peak < SILENCE_RMS
    }

    /// Cut index inside `buffer` (relative to buffer start), preferring the
    /// quietest frame in the trailing search region once we have enough audio.
    fn find_cut(buffer: &[f32]) -> usize {
        if buffer.len() < LOOK_FOR_CUT_SAMPLES {
            return buffer.len();
        }
        let search_start = buffer.len().saturating_sub(CUT_SEARCH_SAMPLES);
        // Never cut before we have at least (max - search) of kept audio, so
        // windows stay near 25–30 s rather than collapsing early.
        let min_keep = LOOK_FOR_CUT_SAMPLES.saturating_sub(CUT_SEARCH_SAMPLES);
        let mut best_idx = buffer.len().min(MAX_WINDOW_SAMPLES);
        let mut best_rms = f32::MAX;
        let end = buffer.len().min(MAX_WINDOW_SAMPLES);
        let mut i = search_start.max(min_keep);
        while i + FRAME_SAMPLES <= end {
            let frame = &buffer[i..i + FRAME_SAMPLES];
            let sum_sq: f32 = frame.iter().map(|s| s * s).sum();
            let rms = (sum_sq / FRAME_SAMPLES as f32).sqrt();
            if rms < best_rms {
                best_rms = rms;
                best_idx = i + FRAME_SAMPLES / 2;
            }
            i += FRAME_SAMPLES;
        }
        best_idx.clamp(1, end)
    }

    fn emit_front(&mut self, cut: usize) -> Window {
        let samples: Vec<f32> = self.buffer.drain(..cut).collect();
        let start_sample = self.next_start;
        self.next_start += samples.len() as u64;
        let silent = Self::is_silent(&samples);
        Window {
            start_sample,
            samples,
            silent,
        }
    }
}

impl<I> Iterator for Windower<I>
where
    I: Iterator<Item = AppResult<Vec<f32>>>,
{
    type Item = AppResult<Window>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.finished && self.buffer.is_empty() {
                return None;
            }

            while self.buffer.len() < LOOK_FOR_CUT_SAMPLES && !self.finished {
                match self.source.next() {
                    Some(Ok(chunk)) => self.buffer.extend_from_slice(&chunk),
                    Some(Err(err)) => return Some(Err(err)),
                    None => {
                        self.finished = true;
                        break;
                    }
                }
            }

            if self.buffer.is_empty() {
                return None;
            }

            if !self.finished && self.buffer.len() < LOOK_FOR_CUT_SAMPLES {
                // Need more input; pull once more if possible.
                match self.source.next() {
                    Some(Ok(chunk)) => {
                        self.buffer.extend_from_slice(&chunk);
                        continue;
                    }
                    Some(Err(err)) => return Some(Err(err)),
                    None => {
                        self.finished = true;
                    }
                }
            }

            if self.finished {
                // Flush remaining audio, still capped at max window length.
                let cut = self.buffer.len().min(MAX_WINDOW_SAMPLES);
                return Some(Ok(self.emit_front(cut)));
            }

            // Keep pulling until we can cut, or hit max and must cut.
            while self.buffer.len() < MAX_WINDOW_SAMPLES && !self.finished {
                match self.source.next() {
                    Some(Ok(chunk)) => self.buffer.extend_from_slice(&chunk),
                    Some(Err(err)) => return Some(Err(err)),
                    None => {
                        self.finished = true;
                        break;
                    }
                }
                if self.buffer.len() >= LOOK_FOR_CUT_SAMPLES {
                    break;
                }
            }

            if self.finished {
                let cut = self.buffer.len().min(MAX_WINDOW_SAMPLES);
                if cut == 0 {
                    return None;
                }
                return Some(Ok(self.emit_front(cut)));
            }

            let cut = Self::find_cut(&self.buffer);
            return Some(Ok(self.emit_front(cut)));
        }
    }
}

/// Convert a source-rate frame count to an estimated 16 kHz sample count.
pub fn source_frames_to_target(frames: u64, sample_rate: u32) -> u64 {
    if sample_rate == TARGET_SAMPLE_RATE {
        frames
    } else {
        ((frames as f64) * TARGET_SAMPLE_RATE as f64 / f64::from(sample_rate)).round() as u64
    }
}

/// Drains a file through decode and resample into one 16 kHz mono buffer.
#[cfg(test)]
fn load_mono_16k(path: &Path) -> AppResult<Vec<f32>> {
    let source = PcmSource::open(path)?;
    let mut out = Vec::new();
    for block in StreamResampler::new(source) {
        out.extend_from_slice(&block?);
    }
    if out.is_empty() {
        log::error!("whisper audio produced no samples");
        return Err(AppError::transcription_failed());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn tone(rate: u32, seconds: f32, freq: f32, amp: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .map(|i| ((i as f32) * freq * 2.0 * PI / rate as f32).sin() * amp)
            .collect()
    }

    fn silence(rate: u32, seconds: f32) -> Vec<f32> {
        vec![0.0; (rate as f32 * seconds) as usize]
    }

    /// Whole-buffer linear resample, matching the old one-shot algorithm.
    fn resample_all(mono: &[f32], sample_rate: u32) -> Vec<f32> {
        if sample_rate == TARGET_SAMPLE_RATE || mono.is_empty() {
            return mono.to_vec();
        }
        let ratio = TARGET_SAMPLE_RATE as f64 / sample_rate as f64;
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
        out
    }

    struct VecSource {
        blocks: Vec<PcmBlock>,
        index: usize,
    }

    impl Iterator for VecSource {
        type Item = AppResult<PcmBlock>;

        fn next(&mut self) -> Option<Self::Item> {
            if self.index >= self.blocks.len() {
                return None;
            }
            let block = self.blocks[self.index].clone();
            self.index += 1;
            Some(Ok(block))
        }
    }

    impl Clone for PcmBlock {
        fn clone(&self) -> Self {
            Self {
                samples: self.samples.clone(),
                sample_rate: self.sample_rate,
            }
        }
    }

    #[test]
    fn stream_resampler_matches_whole_buffer() {
        let rate = 48_000u32;
        let input = tone(rate, 1.0, 440.0, 0.25);
        let expected = resample_all(&input, rate);

        let chunk = 2400;
        let mut blocks = Vec::new();
        for piece in input.chunks(chunk) {
            blocks.push(PcmBlock {
                samples: piece.to_vec(),
                sample_rate: rate,
            });
        }
        let mut got = Vec::new();
        for block in StreamResampler::new(VecSource { blocks, index: 0 }) {
            got.extend_from_slice(&block.unwrap());
        }

        assert_eq!(got.len(), expected.len());
        let max_err = got
            .iter()
            .zip(expected.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_err < 1e-4,
            "stream vs whole-buffer max error {max_err}"
        );
    }

    #[test]
    fn windower_cuts_inside_silence_and_caps_length() {
        let rate = TARGET_SAMPLE_RATE;
        // 20 s tone, 2 s silence, 20 s tone — forces a cut in the quiet gap
        // once the buffer reaches the look-for-cut threshold.
        let mut pcm = tone(rate, 20.0, 440.0, 0.3);
        pcm.extend(silence(rate, 2.0));
        pcm.extend(tone(rate, 20.0, 440.0, 0.3));

        let chunks: Vec<AppResult<Vec<f32>>> = pcm
            .chunks(rate as usize / 2)
            .map(|c| Ok(c.to_vec()))
            .collect();
        let windows: Vec<Window> = Windower::new(chunks.into_iter())
            .map(|w| w.unwrap())
            .collect();

        assert!(windows.len() >= 2);
        for w in &windows {
            assert!(
                w.samples.len() <= MAX_WINDOW_SAMPLES,
                "window len {}",
                w.samples.len()
            );
        }

        // Contiguous coverage.
        let mut cursor = 0u64;
        for w in &windows {
            assert_eq!(w.start_sample, cursor);
            cursor += w.samples.len() as u64;
        }
        assert_eq!(cursor, pcm.len() as u64);

        // First cut should land in the silence region (20–22 s).
        let first_end = windows[0].end_sample();
        let silence_start = (rate as u64) * 20;
        let silence_end = (rate as u64) * 22;
        assert!(
            first_end >= silence_start && first_end <= silence_end,
            "first cut at sample {first_end}, expected in [{silence_start}, {silence_end}]"
        );
    }

    #[test]
    fn pure_silence_windows_are_marked_silent() {
        let rate = TARGET_SAMPLE_RATE;
        let pcm = silence(rate, 35.0);
        let chunks: Vec<AppResult<Vec<f32>>> = pcm
            .chunks(rate as usize)
            .map(|c| Ok(c.to_vec()))
            .collect();
        let windows: Vec<Window> = Windower::new(chunks.into_iter())
            .map(|w| w.unwrap())
            .collect();
        assert!(!windows.is_empty());
        assert!(windows.iter().all(|w| w.silent));
        let total: usize = windows.iter().map(|w| w.samples.len()).sum();
        assert_eq!(total, pcm.len());
    }

    #[test]
    fn garbage_m4a_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.m4a");
        std::fs::write(&path, b"not a real m4a file at all").unwrap();
        let err = PcmSource::open(&path).err().or_else(|| {
            let source = PcmSource::open(&path).ok()?;
            StreamResampler::new(source).find_map(|r| r.err())
        });
        assert!(err.is_some(), "expected transcription failure on garbage m4a");
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn transcoded_aac_streams_as_mono_16k() {
        use crate::storage::transcode;
        use crate::storage::wav_writer::CheckpointingWavWriter;

        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("meeting.wav");
        let rate = 48_000u32;
        let mut writer = CheckpointingWavWriter::create(&wav, rate).unwrap();
        let samples = tone(rate, 1.0, 440.0, 0.25);
        writer.write_samples(&samples).unwrap();
        writer.finalize().unwrap();

        let name = transcode::replace_wav_with_aac(&wav);
        assert_eq!(name, "meeting.m4a");
        let pcm = load_mono_16k(&dir.path().join("meeting.m4a")).unwrap();
        assert!(pcm.len() > 12_000 && pcm.len() < 20_000);
        assert!(pcm.iter().any(|s| s.abs() > 0.01));
    }
}
