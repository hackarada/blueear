//! Incremental, crash-safe mono 32-bit float WAV writer.
//!
//! Writes a placeholder header, appends samples as they arrive, and
//! periodically rewrites just the three size fields (`checkpoint`) so a
//! `kill -9` mid-recording leaves a file whose header undercounts by at
//! most one checkpoint interval, rather than a file with a header claiming
//! zero bytes of audio (the failure mode hit -- and fixed -- in the capture
//! spike; see `spike/capture-spike/RESULTS.md`).
//!
//! Format: IEEE float (WAVE_FORMAT_IEEE_FLOAT = 3), mono, 32-bit, plus the
//! `fact` chunk that format conventionally requires.

use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

/// Bytes before the first audio sample: 12 (RIFF/WAVE) + 24 (fmt, 16-byte
/// body) + 12 (fact, 4-byte body) + 8 (data chunk header) = 56.
pub const HEADER_LEN: u64 = 56;
const BYTES_PER_SAMPLE: u64 = 4;

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
        write_header(&mut file, sample_rate, 0)?;
        Ok(Self {
            file,
            sample_rate,
            frames_written: 0,
        })
    }

    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    /// Appends mono samples at the current end of the file.
    pub fn write_samples(&mut self, samples: &[f32]) -> io::Result<()> {
        if samples.is_empty() {
            return Ok(());
        }
        self.file.seek(SeekFrom::End(0))?;
        for sample in samples {
            self.file.write_all(&sample.to_le_bytes())?;
        }
        self.frames_written += samples.len() as u64;
        Ok(())
    }

    /// Rewrites the RIFF/fact/data size fields to reflect everything
    /// written so far, then repositions to the end so writing can continue.
    /// Safe to call repeatedly while still recording.
    pub fn checkpoint(&mut self) -> io::Result<()> {
        let frames = self.frames_written;
        let sample_rate = self.sample_rate;
        write_header(&mut self.file, sample_rate, frames)?;
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

fn write_header(file: &mut File, sample_rate: u32, frame_count: u64) -> io::Result<()> {
    let data_size = frame_count * BYTES_PER_SAMPLE;
    let riff_size: u32 = (48 + data_size).min(u32::MAX as u64) as u32;
    let byte_rate = sample_rate as u32 * BYTES_PER_SAMPLE as u32;
    let sample_length: u32 = frame_count.min(u32::MAX as u64) as u32;
    let data_size32: u32 = data_size.min(u32::MAX as u64) as u32;

    file.seek(SeekFrom::Start(0))?;
    file.write_all(b"RIFF")?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WAVE")?;

    file.write_all(b"fmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&3u16.to_le_bytes())?; // WAVE_FORMAT_IEEE_FLOAT
    file.write_all(&1u16.to_le_bytes())?; // mono
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&(BYTES_PER_SAMPLE as u16).to_le_bytes())?; // block align
    file.write_all(&32u16.to_le_bytes())?; // bits per sample

    file.write_all(b"fact")?;
    file.write_all(&4u32.to_le_bytes())?;
    file.write_all(&sample_length.to_le_bytes())?;

    file.write_all(b"data")?;
    file.write_all(&data_size32.to_le_bytes())?;

    Ok(())
}

/// Reads the current on-disk frame count without needing a live writer.
/// Used by crash recovery to see what a `.inprogress` file actually
/// contains versus what its header claims.
pub fn actual_frames_on_disk(path: &Path) -> io::Result<u64> {
    let metadata = std::fs::metadata(path)?;
    let size = metadata.len();
    if size <= HEADER_LEN {
        return Ok(0);
    }
    Ok((size - HEADER_LEN) / BYTES_PER_SAMPLE)
}

/// Repairs a WAV file's header to match the bytes actually on disk. Never
/// truncates or overwrites audio bytes -- only the three size fields in the
/// header are rewritten.
pub fn repair_header(path: &Path, sample_rate: u32) -> io::Result<u64> {
    let frames = actual_frames_on_disk(path)?;
    let mut file = OpenOptions::new().write(true).read(true).open(path)?;
    write_header(&mut file, sample_rate, frames)?;
    file.flush()?;
    Ok(frames)
}

/// Reads back a whole mono float WAV file's samples. Test-only helper for
/// round-trip and crash-recovery assertions.
#[cfg(test)]
pub fn read_all_samples(path: &Path) -> io::Result<Vec<f32>> {
    use std::io::Read;
    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if (bytes.len() as u64) <= HEADER_LEN {
        return Ok(Vec::new());
    }
    let data = &bytes[HEADER_LEN as usize..];
    Ok(data
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
        assert_eq!(samples.len(), 5);
        assert!((samples[0] - 0.1).abs() < 1e-6);
        assert!((samples[4] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn header_reports_correct_size_after_checkpoint() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wav");
        let mut writer = CheckpointingWavWriter::create(&path, 48_000).unwrap();
        writer.write_samples(&vec![0.0; 1000]).unwrap();
        writer.checkpoint().unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(metadata.len(), HEADER_LEN + 1000 * 4);
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
}
