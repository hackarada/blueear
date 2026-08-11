//! Small, dependency-free conversion helpers used to bring every source
//! onto one canonical timeline: 48 kHz, mono, `f32`.
//!
//! Both capture sources reported 48 kHz in practice (see
//! `spike/capture-spike/RESULTS.md`), so linear interpolation here is a
//! defensive fallback (e.g. AirPods entering 24 kHz HFP mode) rather than
//! the common case -- it deliberately favors simplicity and correctness
//! over audio-engineering-grade quality.

pub const CANONICAL_SAMPLE_RATE: u32 = 48_000;

/// Averages interleaved multi-channel samples down to mono.
pub fn downmix_to_mono(interleaved: &[f32], channel_count: usize) -> Vec<f32> {
    if channel_count <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channel_count)
        .map(|frame| frame.iter().sum::<f32>() / channel_count as f32)
        .collect()
}

/// Linear-interpolation resample of mono samples from `input_rate` to
/// `output_rate`. Returns the input unchanged if the rates already match
/// (the common path, so no quality loss in the expected case).
pub fn resample_linear(input: &[f32], input_rate: f64, output_rate: f64) -> Vec<f32> {
    if input.is_empty() || (input_rate - output_rate).abs() < f64::EPSILON {
        return input.to_vec();
    }
    let ratio = input_rate / output_rate;
    let output_len = ((input.len() as f64) / ratio).round().max(0.0) as usize;
    let mut output = Vec::with_capacity(output_len);
    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;
        let a = input.get(idx).copied().unwrap_or(0.0);
        let b = input.get(idx + 1).copied().unwrap_or(a);
        output.push(a + (b - a) * frac);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_stereo_pairs() {
        let interleaved = [1.0_f32, -1.0, 0.5, 0.5];
        let mono = downmix_to_mono(&interleaved, 2);
        assert_eq!(mono, vec![0.0, 0.5]);
    }

    #[test]
    fn downmix_is_noop_for_mono() {
        let mono_in = [0.1_f32, 0.2, 0.3];
        assert_eq!(downmix_to_mono(&mono_in, 1), mono_in.to_vec());
    }

    #[test]
    fn resample_matching_rate_is_identity() {
        let input = [0.1_f32, 0.2, 0.3];
        assert_eq!(resample_linear(&input, 48000.0, 48000.0), input.to_vec());
    }

    #[test]
    fn resample_upsamples_to_expected_length() {
        let input = vec![0.0_f32; 24000];
        let output = resample_linear(&input, 24000.0, 48000.0);
        assert!((output.len() as i64 - 48000).abs() <= 2);
    }

    #[test]
    fn resample_downsamples_to_expected_length() {
        let input = vec![0.0_f32; 48000];
        let output = resample_linear(&input, 48000.0, 24000.0);
        assert!((output.len() as i64 - 24000).abs() <= 2);
    }
}
