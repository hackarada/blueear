//! Pure timeline math shared by both recording tracks so Teams audio and
//! microphone audio land on one common, monotonic session timeline instead
//! of drifting apart or starting at different offsets.
//!
//! Each source tracks how many frames it has already written
//! (`frames_written`) and, on every incoming chunk, asks
//! [`silence_frames_needed`] how many frames of silence must be inserted
//! *before* that chunk so the chunk lands at the correct wall-clock offset
//! from the session start. This is intentionally simple: no drift
//! correction beyond re-deriving the target index from the host timestamp
//! on every chunk, which self-corrects for small jitter without any
//! feedback-loop tuning.

/// Given the session's start timestamp and a chunk's own host timestamp
/// (both in nanoseconds from the same monotonic clock), returns the frame
/// index at `sample_rate` where this chunk's first sample belongs.
pub fn target_frame_index(session_start_ns: u64, chunk_host_time_ns: u64, sample_rate: u32) -> u64 {
    let elapsed_ns = chunk_host_time_ns.saturating_sub(session_start_ns);
    (elapsed_ns as u128 * sample_rate as u128 / 1_000_000_000u128) as u64
}

/// How many frames of silence to insert before writing `chunk_frame_count`
/// new frames, given how many frames this source has already written.
/// Returns 0 (never negative/rewinding) if the source is already at or past
/// the target -- e.g. due to clock jitter -- so a track never rewinds.
pub fn silence_frames_needed(frames_already_written: u64, target_frame_index: u64) -> u64 {
    target_frame_index.saturating_sub(frames_already_written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_chunk_at_session_start_needs_no_silence() {
        let target = target_frame_index(1_000_000_000, 1_000_000_000, 48_000);
        assert_eq!(target, 0);
        assert_eq!(silence_frames_needed(0, target), 0);
    }

    #[test]
    fn chunk_arriving_late_needs_silence_padding() {
        // 100ms after session start at 48kHz => 4800 frames in.
        let target = target_frame_index(0, 100_000_000, 48_000);
        assert_eq!(target, 4_800);
        assert_eq!(silence_frames_needed(0, target), 4_800);
    }

    #[test]
    fn already_caught_up_source_needs_no_extra_silence() {
        let target = target_frame_index(0, 100_000_000, 48_000);
        // This source already wrote past the target due to jitter.
        assert_eq!(silence_frames_needed(5_000, target), 0);
    }
}
