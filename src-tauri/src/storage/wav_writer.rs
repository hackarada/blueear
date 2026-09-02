//! Incremental, crash-safe mono 16-bit PCM WAV writer.
//!
//! Writes a placeholder header, appends samples as they arrive, and
//! periodically rewrites just the size fields (`checkpoint`) so a
//! `kill -9` mid-recording leaves a file whose header undercounts by at
//! most one checkpoint interval, rather than a file with a header claiming
//! zero bytes of audio (the failure mode hit -- and fixed -- in the capture
//! spike; see `spike/capture-spike/RESULTS.md`).
//!
//! New files: PCM (WAVE_FORMAT_PCM = 1), mono, 16-bit integer. Speech does
//! not need 32-bit float, and this halves on-disk size versus the original
//! IEEE-float writer.
//!
//! Crash recovery still understands the previous IEEE-float layout
//! (`WAVE_FORMAT_IEEE_FLOAT`, 32-bit, with a `fact` chunk) so an upgrade
//! cannot corrupt an in-progress session from an older build.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Bytes before the first audio sample in a file this writer creates:
/// 12 (RIFF/WAVE) + 24 (fmt, 16-byte body) + 8 (data chunk header) = 44.
pub const HEADER_LEN: u64 = 44;
const PCM16_BYTES_PER_SAMPLE: u64 = 2;
const FLOAT32_HEADER_LEN: u64 = 56;
const FLOAT32_BYTES_PER_SAMPLE: u64 = 4;

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Pcm16,
    Float32,
}

impl Encoding {
    fn header_len(self) -> u64 {
        match self {
            Encoding::Pcm16 => HEADER_LEN,
            Encoding::Float32 => FLOAT32_HEADER_LEN,
        }
    }

    fn bytes_per_sample(self) -> u64 {
        match self {
            Encoding::Pcm16 => PCM16_BYTES_PER_SAMPLE,
            Encoding::Float32 => FLOAT32_BYTES_PER_SAMPLE,
        }
    }
}

struct WavLayout {
    encoding: Encoding,
    data_offset: u64,
}

pub struct CheckpointingWavWriter {
    file: File,
    sample_rate: u32,
    frames_written: u64,
}

impl CheckpointingWavWriter {
    pub fn create(path: &Path, sample_rate: u32) -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        write_header(&mut file, sample_rate, 0, Encoding::Pcm16)?;
        Ok(Self {
            file,
            sample_rate,
            frames_written: 0,
        })
    }

    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    /// Appends mono samples at the current end of the file. Values outside
    /// `[-1.0, 1.0]` are clipped before quantizing to 16-bit PCM.
    pub fn write_samples(&mut self, samples: &[f32]) -> io::Result<()> {
        if samples.is_empty() {
            return Ok(());
        }
        self.file.seek(SeekFrom::End(0))?;
        for sample in samples {
            self.file.write_all(&f32_to_i16(*sample).to_le_bytes())?;
        }
        self.frames_written += samples.len() as u64;
        Ok(())
    }

    /// Rewrites the RIFF/data size fields to reflect everything
    /// written so far, then repositions to the end so writing can continue.
    /// Safe to call repeatedly while still recording.
    pub fn checkpoint(&mut self) -> io::Result<()> {
        let frames = self.frames_written;
        let sample_rate = self.sample_rate;
        write_header(&mut self.file, sample_rate, frames, Encoding::Pcm16)?;
        self.file.seek(SeekFrom::End(0))?;
        self.file.flush()
    }

    /// Final checkpoint. Semantically identical to `checkpoint`, kept as a
    /// distinct name at call sites for clarity when a recording stops.
    pub fn finalize(mut self) -> io::Result<u64> {
        self.checkpoint()?;
        Ok(self.frames_written)
    }
}

fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

#[cfg(test)]
fn i16_to_f32(sample: i16) -> f32 {
    sample as f32 / i16::MAX as f32
}

fn write_header(
    file: &mut File,
    sample_rate: u32,
    frame_count: u64,
    encoding: Encoding,
) -> io::Result<()> {
    match encoding {
        Encoding::Pcm16 => write_pcm16_header(file, sample_rate, frame_count),
        Encoding::Float32 => write_float32_header(file, sample_rate, frame_count),
    }
}

fn write_pcm16_header(file: &mut File, sample_rate: u32, frame_count: u64) -> io::Result<()> {
    let data_size = frame_count * PCM16_BYTES_PER_SAMPLE;
    let riff_size: u32 = (36 + data_size).min(u32::MAX as u64) as u32;
    let byte_rate = sample_rate.saturating_mul(PCM16_BYTES_PER_SAMPLE as u32);
    let data_size32: u32 = data_size.min(u32::MAX as u64) as u32;

    file.seek(SeekFrom::Start(0))?;
    file.write_all(b"RIFF")?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WAVE")?;

    file.write_all(b"fmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&WAVE_FORMAT_PCM.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?; // mono
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&(PCM16_BYTES_PER_SAMPLE as u16).to_le_bytes())?; // block align
    file.write_all(&16u16.to_le_bytes())?; // bits per sample

    file.write_all(b"data")?;
    file.write_all(&data_size32.to_le_bytes())?;

    Ok(())
}

/// On-disk layout of sessions written by the previous 32-bit float writer.
/// Kept only so crash recovery can repair those files without shifting or
/// reinterpreting the audio bytes.
fn write_float32_header(file: &mut File, sample_rate: u32, frame_count: u64) -> io::Result<()> {
    let data_size = frame_count * FLOAT32_BYTES_PER_SAMPLE;
    let riff_size: u32 = (48 + data_size).min(u32::MAX as u64) as u32;
    let byte_rate = sample_rate.saturating_mul(FLOAT32_BYTES_PER_SAMPLE as u32);
    let sample_length: u32 = frame_count.min(u32::MAX as u64) as u32;
    let data_size32: u32 = data_size.min(u32::MAX as u64) as u32;

    file.seek(SeekFrom::Start(0))?;
    file.write_all(b"RIFF")?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WAVE")?;

    file.write_all(b"fmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&WAVE_FORMAT_IEEE_FLOAT.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?; // mono
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&(FLOAT32_BYTES_PER_SAMPLE as u16).to_le_bytes())?; // block align
    file.write_all(&32u16.to_le_bytes())?; // bits per sample

    file.write_all(b"fact")?;
    file.write_all(&4u32.to_le_bytes())?;
    file.write_all(&sample_length.to_le_bytes())?;

    file.write_all(b"data")?;
    file.write_all(&data_size32.to_le_bytes())?;

    Ok(())
}

fn inspect_layout(path: &Path) -> io::Result<WavLayout> {
    let mut file = File::open(path)?;
    let mut riff = [0u8; 12];
    if file.read_exact(&mut riff).is_err() || &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return Ok(WavLayout {
            encoding: Encoding::Pcm16,
            data_offset: Encoding::Pcm16.header_len(),
        });
    }

    let mut encoding = Encoding::Pcm16;
    let mut data_offset: Option<u64> = None;

    loop {
        let mut chunk_hdr = [0u8; 8];
        if file.read_exact(&mut chunk_hdr).is_err() {
            break;
        }
        let size = u32::from_le_bytes([chunk_hdr[4], chunk_hdr[5], chunk_hdr[6], chunk_hdr[7]]) as u64;
        let chunk_data_start = file.stream_position()?;
        if &chunk_hdr[0..4] == b"fmt " {
            let mut tag = [0u8; 2];
            if file.read_exact(&mut tag).is_ok() {
                encoding = match u16::from_le_bytes(tag) {
                    WAVE_FORMAT_IEEE_FLOAT => Encoding::Float32,
                    _ => Encoding::Pcm16,
                };
            }
        } else if &chunk_hdr[0..4] == b"data" {
            data_offset = Some(chunk_data_start);
            break;
        }
        let padded = size + (size % 2);
        if file.seek(SeekFrom::Start(chunk_data_start + padded)).is_err() {
            break;
        }
    }

    Ok(WavLayout {
        encoding,
        data_offset: data_offset.unwrap_or_else(|| encoding.header_len()),
    })
}

fn frames_from_len(file_len: u64, layout: &WavLayout) -> u64 {
    if file_len <= layout.data_offset {
        0
    } else {
        (file_len - layout.data_offset) / layout.encoding.bytes_per_sample()
    }
}

/// Reads the current on-disk frame count without needing a live writer.
/// Used by crash recovery to see what a `.inprogress` file actually
/// contains versus what its header claims. Understands both the current
/// 16-bit PCM layout and the previous 32-bit float layout.
pub fn actual_frames_on_disk(path: &Path) -> io::Result<u64> {
    let layout = inspect_layout(path)?;
    let size = std::fs::metadata(path)?.len();
    Ok(frames_from_len(size, &layout))
}

/// Repairs a WAV file's header to match the bytes actually on disk. Never
/// truncates or overwrites audio bytes -- only the size fields in the
/// header are rewritten, in the same encoding the file already uses.
pub fn repair_header(path: &Path, sample_rate: u32) -> io::Result<u64> {
    let layout = inspect_layout(path)?;
    let frames = actual_frames_on_disk(path)?;
    let mut file = OpenOptions::new().write(true).read(true).open(path)?;
    write_header(&mut file, sample_rate, frames, layout.encoding)?;
    file.flush()?;
    Ok(frames)
}

/// Reads back a whole mono WAV file's samples as `f32` in `[-1.0, 1.0]`.
/// Test-only helper for round-trip and crash-recovery assertions; understands
/// both PCM16 and legacy IEEE-float files.
#[cfg(test)]
pub fn read_all_samples(path: &Path) -> io::Result<Vec<f32>> {
    let layout = inspect_layout(path)?;
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(layout.data_offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    match layout.encoding {
        Encoding::Pcm16 => Ok(bytes
            .chunks_exact(2)
            .map(|chunk| i16_to_f32(i16::from_le_bytes([chunk[0], chunk[1]])))
            .collect()),
        Encoding::Float32 => Ok(bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const QUANT_EPS: f32 = 2.0 / i16::MAX as f32;

    fn assert_close(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected) {
            assert!(
                (a - e).abs() < QUANT_EPS,
                "sample {a} drifted from {e} by more than PCM16 quantization"
            );
        }
    }

    fn write_legacy_float32_wav(path: &Path, sample_rate: u32, samples: &[f32]) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        write_header(&mut file, sample_rate, 0, Encoding::Float32)?;
        file.seek(SeekFrom::End(0))?;
        for sample in samples {
            file.write_all(&sample.to_le_bytes())?;
        }
        Ok(())
    }

    #[test]
    fn write_checkpoint_and_read_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wav");
        let mut writer = CheckpointingWavWriter::create(&path, 48_000).unwrap();
        writer.write_samples(&[0.1, 0.2, 0.3]).unwrap();
        writer.checkpoint().unwrap();
        writer.write_samples(&[0.4, 0.5]).unwrap();
        let frames = writer.finalize().unwrap();
        assert_eq!(frames, 5);

        let samples = read_all_samples(&path).unwrap();
        assert_close(&samples, &[0.1, 0.2, 0.3, 0.4, 0.5]);
    }

    #[test]
    fn header_reports_correct_size_after_checkpoint() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wav");
        let mut writer = CheckpointingWavWriter::create(&path, 48_000).unwrap();
        writer.write_samples(&vec![0.0; 1000]).unwrap();
        writer.checkpoint().unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(metadata.len(), HEADER_LEN + 1000 * PCM16_BYTES_PER_SAMPLE);

        let mut header = [0u8; 44];
        let mut file = File::open(&path).unwrap();
        file.read_exact(&mut header).unwrap();
        let format_tag = u16::from_le_bytes([header[20], header[21]]);
        let bits = u16::from_le_bytes([header[34], header[35]]);
        assert_eq!(format_tag, WAVE_FORMAT_PCM);
        assert_eq!(bits, 16);
    }

    #[test]
    fn simulated_crash_before_finalize_is_still_readable_after_repair() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("crashed.wav");
        {
            // Deliberately never call checkpoint/finalize before dropping,
            // simulating a forced-exit style crash.
            let mut writer = CheckpointingWavWriter::create(&path, 48_000).unwrap();
            writer.write_samples(&vec![0.25; 480]).unwrap();
        }
        let repaired_frames = repair_header(&path, 48_000).unwrap();
        assert_eq!(repaired_frames, 480);
        let samples = read_all_samples(&path).unwrap();
        assert_eq!(samples.len(), 480);
        assert_close(&samples, &vec![0.25; 480]);
    }

    #[test]
    fn actual_frames_on_disk_matches_written_frames() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wav");
        let mut writer = CheckpointingWavWriter::create(&path, 48_000).unwrap();
        writer.write_samples(&vec![0.0; 2000]).unwrap();
        drop(writer);
        assert_eq!(actual_frames_on_disk(&path).unwrap(), 2000);
    }

    #[test]
    fn repair_header_preserves_legacy_float32_bytes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("legacy.wav");
        write_legacy_float32_wav(&path, 48_000, &[0.5; 240]).unwrap();

        let repaired_frames = repair_header(&path, 48_000).unwrap();
        assert_eq!(repaired_frames, 240);

        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(
            metadata.len(),
            FLOAT32_HEADER_LEN + 240 * FLOAT32_BYTES_PER_SAMPLE
        );

        let mut header = [0u8; 22];
        let mut file = File::open(&path).unwrap();
        file.read_exact(&mut header).unwrap();
        let format_tag = u16::from_le_bytes([header[20], header[21]]);
        assert_eq!(format_tag, WAVE_FORMAT_IEEE_FLOAT);

        let samples = read_all_samples(&path).unwrap();
        assert_eq!(samples.len(), 240);
        assert!(samples.iter().all(|&s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn hound_reads_new_pcm16_files() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hound.wav");
        let mut writer = CheckpointingWavWriter::create(&path, 48_000).unwrap();
        writer.write_samples(&[0.0, 0.5, -0.5]).unwrap();
        writer.finalize().unwrap();

        let mut reader = hound::WavReader::open(&path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
        let samples: Vec<i32> = reader.samples::<i32>().map(|s| s.unwrap()).collect();
        assert_eq!(samples.len(), 3);
    }
}
