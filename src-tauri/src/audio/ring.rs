//! Bounded, allocation-free single-producer/single-consumer ring buffer used
//! to move PCM out of the real-time native audio callback (see
//! `audio::ffi`) into the non-real-time Rust recording worker
//! (`session::manager`).
//!
//! Each slot is a fixed-size, `Copy` struct so pushing a chunk from the
//! real-time thread never allocates. If the consumer falls behind and the
//! ring fills up, `push` drops the chunk and increments a counter instead of
//! blocking or growing memory -- this is surfaced to the UI as a
//! `droppedFrames` counter rather than silently corrupting the recording.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rtrb::{Consumer, Producer, RingBuffer};

/// Generous upper bound on frames per callback. Observed Core Audio process
/// tap callbacks deliver ~480 frames at 48kHz (~10ms); AVAudioEngine mic
/// taps are configured for 2048-frame buffers. 8192 leaves headroom for
/// larger buffer sizes some audio devices request.
pub const MAX_CHUNK_FRAMES: usize = 8192;
pub const MAX_CHANNELS: usize = 2;
const SAMPLE_CAPACITY: usize = MAX_CHUNK_FRAMES * MAX_CHANNELS;

/// Number of in-flight chunks the ring can hold before the producer starts
/// dropping. At ~10ms/chunk this is roughly half a second of buffering.
const RING_CAPACITY: usize = 64;

#[derive(Clone, Copy)]
pub struct AudioChunk {
    pub samples: [f32; SAMPLE_CAPACITY],
    pub frame_count: u32,
    pub channel_count: u32,
    pub sample_rate: f64,
    pub host_time_ns: u64,
}

impl AudioChunk {
    pub fn empty() -> Self {
        Self {
            samples: [0.0; SAMPLE_CAPACITY],
            frame_count: 0,
            channel_count: 0,
            sample_rate: 0.0,
            host_time_ns: 0,
        }
    }

    #[inline]
    pub fn as_slice(&self) -> &[f32] {
        let len = (self.frame_count as usize) * (self.channel_count as usize);
        &self.samples[..len.min(SAMPLE_CAPACITY)]
    }
}

pub struct RingProducer {
    inner: Producer<AudioChunk>,
    dropped: Arc<AtomicU64>,
}

impl RingProducer {
    /// Copies `samples` (interleaved, `frame_count * channel_count` long)
    /// into a chunk and pushes it. Never blocks and never allocates.
    /// Oversized input is truncated defensively rather than panicking or
    /// writing out of bounds -- this should not happen given
    /// `MAX_CHUNK_FRAMES`, but a real-time callback must never crash the
    /// whole app on an unexpected buffer size from the OS.
    #[inline]
    pub fn push(
        &mut self,
        samples: &[f32],
        frame_count: u32,
        channel_count: u32,
        sample_rate: f64,
        host_time_ns: u64,
    ) {
        let mut chunk = AudioChunk::empty();
        let requested_len = (frame_count as usize) * (channel_count as usize);
        let copy_len = requested_len.min(samples.len()).min(SAMPLE_CAPACITY);
        chunk.samples[..copy_len].copy_from_slice(&samples[..copy_len]);
        let actual_frames = if channel_count > 0 {
            (copy_len / channel_count as usize) as u32
        } else {
            0
        };
        chunk.frame_count = actual_frames;
        chunk.channel_count = channel_count;
        chunk.sample_rate = sample_rate;
        chunk.host_time_ns = host_time_ns;

        if self.inner.push(chunk).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub struct RingConsumer {
    inner: Consumer<AudioChunk>,
    dropped: Arc<AtomicU64>,
}

impl RingConsumer {
    pub fn pop(&mut self) -> Option<AudioChunk> {
        self.inner.pop().ok()
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

pub fn channel() -> (RingProducer, RingConsumer) {
    let (producer, consumer) = RingBuffer::<AudioChunk>::new(RING_CAPACITY);
    let dropped = Arc::new(AtomicU64::new(0));
    (
        RingProducer {
            inner: producer,
            dropped: dropped.clone(),
        },
        RingConsumer {
            inner: consumer,
            dropped,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_pop_round_trips_samples() {
        let (mut tx, mut rx) = channel();
        let samples = [0.1_f32, 0.2, 0.3, 0.4];
        tx.push(&samples, 2, 2, 48000.0, 123);
        let chunk = rx.pop().expect("chunk should be available");
        assert_eq!(chunk.frame_count, 2);
        assert_eq!(chunk.channel_count, 2);
        assert_eq!(chunk.sample_rate, 48000.0);
        assert_eq!(chunk.host_time_ns, 123);
        assert_eq!(chunk.as_slice(), &samples[..]);
    }

    #[test]
    fn full_ring_drops_and_counts_instead_of_blocking() {
        let (mut tx, rx) = channel();
        let samples = [0.0_f32; 4];
        for _ in 0..(RING_CAPACITY + 10) {
            tx.push(&samples, 2, 2, 48000.0, 0);
        }
        assert!(rx.dropped_count() > 0);
    }

    #[test]
    fn oversized_input_is_truncated_not_out_of_bounds() {
        let (mut tx, mut rx) = channel();
        let big = vec![1.0_f32; SAMPLE_CAPACITY * 4];
        tx.push(&big, (MAX_CHUNK_FRAMES * 4) as u32, MAX_CHANNELS as u32, 48000.0, 0);
        let chunk = rx.pop().expect("chunk should still be produced");
        assert!(chunk.frame_count as usize <= MAX_CHUNK_FRAMES);
    }
}
