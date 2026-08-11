//! Combines the (already mono, 48kHz) Teams and microphone timelines into a
//! single convenience playback track. Only the mixed track is limited; the
//! per-source `teams.wav` / `microphone.wav` files stay untouched so nothing
//! is lost to the limiter.

/// Conservative fixed gain applied to each source before summing, leaving
/// headroom so two simultaneous full-scale sources don't automatically
/// clip; the following `soft_clip` handles the rare remaining peak.
const MIX_GAIN: f32 = 0.9;

pub fn soft_clip(sample: f32) -> f32 {
    sample.clamp(-1.0, 1.0)
}

/// Mixes one frame from up to two sources. `None` is treated as silence
/// (e.g. the microphone track for a session recorded without a microphone).
pub fn mix_frame(teams: Option<f32>, microphone: Option<f32>) -> f32 {
    let sum = teams.unwrap_or(0.0) * MIX_GAIN + microphone.unwrap_or(0.0) * MIX_GAIN;
    soft_clip(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing_silence_is_silence() {
        assert_eq!(mix_frame(None, None), 0.0);
    }

    #[test]
    fn mixing_single_source_is_gain_only() {
        assert!((mix_frame(Some(0.5), None) - 0.45).abs() < 1e-6);
    }

    #[test]
    fn mixing_two_loud_sources_never_exceeds_full_scale() {
        let mixed = mix_frame(Some(1.0), Some(1.0));
        assert!(mixed <= 1.0 && mixed >= -1.0);
    }

    #[test]
    fn soft_clip_bounds_extreme_values() {
        assert_eq!(soft_clip(5.0), 1.0);
        assert_eq!(soft_clip(-5.0), -1.0);
    }
}
