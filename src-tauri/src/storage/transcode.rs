//! Post-record AAC transcode.
//!
//! Capture still writes crash-safe WAV. After a clean stop, each track is
//! encoded to 96 kbps AAC in an `.m4a` container and the WAV is deleted.
//! If encoding fails, the WAV is left in place so the session stays playable.

use std::fs;
use std::io;
use std::path::Path;

#[cfg(target_os = "windows")]
use hound::{SampleFormat, WavReader};

/// 96 kbps. The Windows Media Foundation AAC encoder only accepts 96/128/160/192
/// kbps; 96 kbps is still about 7x smaller than 16-bit 48 kHz WAV.
const AAC_BITRATE_BPS: u32 = 96_000;

/// Replaces `path/to/track.wav` with `path/to/track.m4a` when encoding
/// succeeds. Returns the filename that should be stored in session metadata.
pub fn replace_wav_with_aac(wav: &Path) -> String {
    let wav_name = wav
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("audio.wav")
        .to_string();

    if !wav.is_file() {
        return wav_name;
    }

    let dest = wav.with_extension("m4a");
    if dest.exists() {
        let _ = fs::remove_file(&dest);
    }

    match transcode_wav_to_m4a(wav, &dest) {
        Ok(()) if dest.is_file() && file_nonempty(&dest) => {
            if let Err(err) = fs::remove_file(wav) {
                log::warn!(
                    "AAC written but WAV could not be deleted ({}): {err}",
                    wav.display()
                );
            }
            dest.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("audio.m4a")
                .to_string()
        }
        Ok(()) => {
            log::warn!(
                "keeping WAV after AAC transcode produced an empty file ({})",
                wav.display()
            );
            let _ = fs::remove_file(&dest);
            wav_name
        }
        Err(err) => {
            log::warn!(
                "keeping WAV after AAC transcode failed ({}): {err}",
                wav.display()
            );
            let _ = fs::remove_file(&dest);
            wav_name
        }
    }
}

fn file_nonempty(path: &Path) -> bool {
    fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false)
}

pub fn transcode_wav_to_m4a(wav: &Path, m4a: &Path) -> io::Result<()> {
    if m4a.exists() {
        fs::remove_file(m4a)?;
    }
    transcode_platform(wav, m4a)?;
    if !file_nonempty(m4a) {
        let _ = fs::remove_file(m4a);
        return Err(io::Error::other("AAC encoder wrote an empty file"));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn transcode_platform(wav: &Path, m4a: &Path) -> io::Result<()> {
    let output = std::process::Command::new("afconvert")
        .args(["-f", "m4af", "-d", "aac", "-b", &AAC_BITRATE_BPS.to_string()])
        .arg(wav)
        .arg(m4a)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "afconvert failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

#[cfg(target_os = "windows")]
fn transcode_platform(wav: &Path, m4a: &Path) -> io::Result<()> {
    windows_aac::encode(wav, m4a).map_err(|err| io::Error::other(err.to_string()))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn transcode_platform(_wav: &Path, _m4a: &Path) -> io::Result<()> {
    Err(io::Error::other("AAC transcode is not supported on this OS"))
}

#[cfg(target_os = "windows")]
fn read_pcm16_mono(path: &Path) -> io::Result<(u32, Vec<i16>)> {
    let mut reader = WavReader::open(path).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let pcm: Vec<i16> = match spec.sample_format {
        SampleFormat::Int => {
            let shift = spec.bits_per_sample.saturating_sub(16);
            reader
                .samples::<i32>()
                .filter_map(Result::ok)
                .map(|sample| {
                    let scaled = if shift == 0 { sample } else { sample >> shift };
                    scaled.clamp(i16::MIN as i32, i16::MAX as i32) as i16
                })
                .collect()
        }
        SampleFormat::Float => reader
            .samples::<f32>()
            .filter_map(Result::ok)
            .map(|sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16)
            .collect(),
    };
    let mono = if channels <= 1 {
        pcm
    } else {
        pcm.chunks_exact(channels)
            .map(|frame| {
                let sum: i32 = frame.iter().map(|s| i32::from(*s)).sum();
                (sum / channels as i32) as i16
            })
            .collect()
    };
    Ok((spec.sample_rate, mono))
}

#[cfg(target_os = "windows")]
mod windows_aac {
    use std::path::Path;

    use windows::core::HSTRING;
    use windows::Win32::Media::MediaFoundation::{
        IMFAttributes, IMFMediaBuffer, IMFSample, IMFSinkWriter, MFAudioFormat_AAC,
        MFAudioFormat_PCM,
        MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
        MFCreateSinkWriterFromURL, MFMediaType_Audio, MFShutdown, MFStartup, MFSTARTUP_NOSOCKET,
        MFTranscodeContainerType_MPEG4, MF_MT_AAC_PAYLOAD_TYPE, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
        MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS,
        MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_TRANSCODE_CONTAINERTYPE,
        MF_VERSION,
    };
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    use super::{read_pcm16_mono, AAC_BITRATE_BPS};

    pub fn encode(wav: &Path, m4a: &Path) -> windows::core::Result<()> {
        let (sample_rate, samples) = match read_pcm16_mono(wav) {
            Ok(pair) => pair,
            Err(err) => {
                log::error!("wav read for AAC failed: {err}");
                return Err(windows::core::Error::from(
                    windows::Win32::Foundation::E_FAIL,
                ));
            }
        };
        if samples.is_empty() {
            return Err(windows::core::Error::from(
                windows::Win32::Foundation::E_INVALIDARG,
            ));
        }
        // SAFETY: COM/MF objects created here are released by RAII; PCM is a
        // valid i16 slice for the duration of the call.
        unsafe { encode_pcm(m4a, sample_rate, &samples) }
    }

    unsafe fn encode_pcm(m4a: &Path, sample_rate: u32, samples: &[i16]) -> windows::core::Result<()> {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;

        let result = encode_pcm_inner(m4a, sample_rate, samples);
        let _ = MFShutdown();
        result
    }

    unsafe fn encode_pcm_inner(
        m4a: &Path,
        sample_rate: u32,
        samples: &[i16],
    ) -> windows::core::Result<()> {
        let url = HSTRING::from(m4a.as_os_str());
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 1)?;
        let attributes = attributes.ok_or_else(|| {
            windows::core::Error::from(windows::Win32::Foundation::E_FAIL)
        })?;
        attributes.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
        let writer: IMFSinkWriter = MFCreateSinkWriterFromURL(&url, None, &attributes)?;

        let output = MFCreateMediaType()?;
        output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        output.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        output.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        output.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate)?;
        output.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 1)?;
        output.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BITRATE_BPS / 8)?;
        output.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
        let stream_index = writer.AddStream(&output)?;

        let input = MFCreateMediaType()?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate)?;
        input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 1)?;
        input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 2)?;
        input.SetUINT32(
            &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
            sample_rate.saturating_mul(2),
        )?;
        writer.SetInputMediaType(stream_index, &input, None)?;
        writer.BeginWriting()?;

        let mut time_hns: i64 = 0;
        let chunk_frames = sample_rate.max(1) as usize;
        for chunk in samples.chunks(chunk_frames) {
            let byte_len = chunk.len() * 2;
            let buffer: IMFMediaBuffer = MFCreateMemoryBuffer(byte_len as u32)?;
            let mut dest = std::ptr::null_mut();
            buffer.Lock(&mut dest, None, None)?;
            if dest.is_null() {
                let _ = buffer.Unlock();
                return Err(windows::core::Error::from(
                    windows::Win32::Foundation::E_FAIL,
                ));
            }
            std::ptr::copy_nonoverlapping(chunk.as_ptr() as *const u8, dest, byte_len);
            buffer.Unlock()?;
            buffer.SetCurrentLength(byte_len as u32)?;

            let sample: IMFSample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(time_hns)?;
            let duration = (chunk.len() as i64) * 10_000_000 / i64::from(sample_rate);
            sample.SetSampleDuration(duration)?;
            writer.WriteSample(stream_index, &sample)?;
            time_hns += duration;
        }

        writer.Finalize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::wav_writer::CheckpointingWavWriter;
    use std::f32::consts::PI;
    use tempfile::tempdir;

    fn sine_wav(path: &Path, seconds: u32) {
        let rate = 48_000u32;
        let mut writer = CheckpointingWavWriter::create(path, rate).unwrap();
        let samples: Vec<f32> = (0..rate * seconds)
            .map(|i| ((i as f32) * 440.0 * 2.0 * PI / rate as f32).sin() * 0.25)
            .collect();
        writer.write_samples(&samples).unwrap();
        writer.finalize().unwrap();
    }

    #[test]
    fn replace_wav_with_aac_encodes_or_keeps_wav() {
        let dir = tempdir().unwrap();
        let wav = dir.path().join("meeting.wav");
        sine_wav(&wav, 1);

        let name = replace_wav_with_aac(&wav);
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            assert_eq!(name, "meeting.m4a");
            assert!(dir.path().join("meeting.m4a").is_file());
            assert!(!wav.exists());
            let aac_len = fs::metadata(dir.path().join("meeting.m4a")).unwrap().len();
            // 1s of 16-bit 48 kHz mono is ~96 KB; 96 kbps AAC should be far smaller.
            assert!(aac_len > 0);
            assert!(aac_len < 96_000);
        } else {
            assert_eq!(name, "meeting.wav");
            assert!(wav.exists());
        }
    }

    #[test]
    fn missing_wav_returns_original_name() {
        let dir = tempdir().unwrap();
        let wav = dir.path().join("meeting.wav");
        assert_eq!(replace_wav_with_aac(&wav), "meeting.wav");
    }
}
