//! Turning raw provider output into the normalized transcript: attaching
//! speakers to words, grouping words into segments, and interleaving tracks
//! into one timeline.
//!
//! Everything here is a pure function over plain data. That is the whole
//! reason this logic lives in Rust rather than in the Swift adapters -- the
//! interesting decisions (what to do with an ambiguous overlap, where to split
//! a segment, how to break a tie) are exactly the ones worth pinning with
//! tests, and none of them need a model to exercise.

use std::collections::BTreeMap;

use crate::transcription::types::{
    RawTrackResult, Segment, Speaker, SpeakerSpan, Track, TrackTranscript, Word,
};

/// A pause longer than this starts a new segment even when the speaker has not
/// changed. Chosen to match natural sentence breaks: short enough that a
/// segment stays readable, long enough that it does not split mid-sentence on
/// ordinary speech rhythm.
const SEGMENT_GAP_SECONDS: f64 = 1.0;

/// Builds one track's normalized transcript from what the provider returned.
///
/// The microphone track is always the local user, so its diarization is
/// ignored even if a provider offered some. The meeting track is diarized when
/// spans are present and falls back to a track-level label when they are not,
/// which is what makes Apple Speech and FluidAudio produce the same shape.
pub fn build_track_transcript(
    track: Track,
    provider: crate::transcription::types::ProviderId,
    raw: &RawTrackResult,
) -> TrackTranscript {
    let (speakers, speaker_count, diarized) = match track {
        Track::Microphone => (vec![Speaker::You; raw.words.len()], 0, false),
        Track::Meeting if raw.speaker_spans.is_empty() => {
            (vec![Speaker::MeetingAudio; raw.words.len()], 0, false)
        }
        Track::Meeting => {
            let assigned = align_words_to_speakers(&raw.words, &raw.speaker_spans);
            let count = distinct_remote_speakers(&assigned);
            (assigned, count, true)
        }
    };

    TrackTranscript {
        track,
        provider,
        model_id: raw.model_id.clone(),
        language: raw.language.clone(),
        diarized,
        speaker_count,
        segments: group_words_into_segments(track, &raw.words, &speakers),
    }
}

/// Assigns each word to the speaker span it overlaps most.
///
/// A word with no overlapping span, or with two spans overlapping it by
/// exactly the same amount, becomes [`Speaker::Unknown`]. Guessing in those
/// cases would produce a confident-looking transcript that is wrong, which is
/// worse for the user than an honest "unknown".
///
/// Speaker keys are renumbered to 1-based indices in order of first appearance
/// in the audio, so the UI never sees an engine's internal cluster IDs and the
/// numbering is reproducible.
pub fn align_words_to_speakers(words: &[Word], spans: &[SpeakerSpan]) -> Vec<Speaker> {
    let index_of = remote_speaker_indices(spans);

    words
        .iter()
        .map(|word| {
            let mut best: Option<(&str, f64)> = None;
            let mut tied = false;

            for span in spans {
                let overlap = overlap_seconds(
                    word.start_seconds,
                    word.end_seconds,
                    span.start_seconds,
                    span.end_seconds,
                );
                if overlap <= 0.0 {
                    continue;
                }
                match best {
                    Some((_, best_overlap)) if (overlap - best_overlap).abs() < f64::EPSILON => {
                        tied = true;
                    }
                    Some((_, best_overlap)) if overlap > best_overlap => {
                        best = Some((span.speaker_key.as_str(), overlap));
                        tied = false;
                    }
                    Some(_) => {}
                    None => best = Some((span.speaker_key.as_str(), overlap)),
                }
            }

            match best {
                Some((key, _)) if !tied => index_of
                    .get(key)
                    .map(|&index| Speaker::Remote { index })
                    .unwrap_or(Speaker::Unknown),
                _ => Speaker::Unknown,
            }
        })
        .collect()
}

/// Groups consecutive same-speaker words into segments, splitting on a speaker
/// change or a pause longer than [`SEGMENT_GAP_SECONDS`].
fn group_words_into_segments(track: Track, words: &[Word], speakers: &[Speaker]) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();

    for (word, &speaker) in words.iter().zip(speakers.iter()) {
        let starts_new = match segments.last() {
            None => true,
            Some(current) => {
                current.speaker != speaker
                    || word.start_seconds - current.end_seconds > SEGMENT_GAP_SECONDS
            }
        };

        if starts_new {
            segments.push(Segment {
                track,
                speaker,
                start_seconds: word.start_seconds,
                end_seconds: word.end_seconds,
                text: word.text.clone(),
                words: vec![word.clone()],
            });
        } else {
            let current = segments.last_mut().expect("checked above");
            current.end_seconds = current.end_seconds.max(word.end_seconds);
            current.text.push(' ');
            current.text.push_str(&word.text);
            current.words.push(word.clone());
        }
    }

    segments
}

/// Interleaves every track's segments into one timeline.
///
/// Ties are broken by track (Meeting first) and then by speaker and text, so the
/// same inputs always produce byte-identical output. That determinism is what
/// makes exports diffable and the tests here meaningful; without it, two runs
/// over the same recording could disagree about the order of two segments that
/// began at the same instant.
pub fn merge_tracks(tracks: &[TrackTranscript]) -> Vec<Segment> {
    let mut merged: Vec<Segment> = tracks
        .iter()
        .flat_map(|t| t.segments.iter().cloned())
        .collect();

    merged.sort_by(|a, b| {
        a.start_seconds
            .partial_cmp(&b.start_seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.track.order().cmp(&b.track.order()))
            .then_with(|| a.speaker.order().cmp(&b.speaker.order()))
            .then_with(|| a.text.cmp(&b.text))
    });

    merged
}

/// Maps provider-local speaker keys to 1-based indices in order of first
/// appearance in the audio.
fn remote_speaker_indices(spans: &[SpeakerSpan]) -> BTreeMap<&str, u32> {
    let mut ordered: Vec<&SpeakerSpan> = spans.iter().collect();
    ordered.sort_by(|a, b| {
        a.start_seconds
            .partial_cmp(&b.start_seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.speaker_key.cmp(&b.speaker_key))
    });

    let mut indices = BTreeMap::new();
    let mut next = 1u32;
    for span in ordered {
        indices.entry(span.speaker_key.as_str()).or_insert_with(|| {
            let index = next;
            next += 1;
            index
        });
    }
    indices
}

fn distinct_remote_speakers(speakers: &[Speaker]) -> u32 {
    let mut seen: Vec<u32> = speakers
        .iter()
        .filter_map(|s| match s {
            Speaker::Remote { index } => Some(*index),
            _ => None,
        })
        .collect();
    seen.sort_unstable();
    seen.dedup();
    seen.len() as u32
}

fn overlap_seconds(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> f64 {
    (a_end.min(b_end) - a_start.max(b_start)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::types::ProviderId;

    fn word(text: &str, start: f64, end: f64) -> Word {
        Word {
            text: text.to_string(),
            start_seconds: start,
            end_seconds: end,
            confidence: None,
        }
    }

    fn span(key: &str, start: f64, end: f64) -> SpeakerSpan {
        SpeakerSpan {
            speaker_key: key.to_string(),
            start_seconds: start,
            end_seconds: end,
        }
    }

    #[test]
    fn words_are_assigned_to_the_span_they_overlap_most() {
        let words = [word("hello", 0.0, 1.0), word("there", 2.0, 3.0)];
        let spans = [span("spk_b", 0.0, 1.5), span("spk_a", 1.5, 3.5)];

        // Indices follow first appearance in the audio, not the engine's key
        // ordering: spk_b starts first, so it becomes Speaker 1.
        assert_eq!(
            align_words_to_speakers(&words, &spans),
            vec![Speaker::Remote { index: 1 }, Speaker::Remote { index: 2 }]
        );
    }

    #[test]
    fn a_word_overlapping_two_spans_takes_the_larger_overlap() {
        let words = [word("maybe", 1.0, 2.0)];
        let spans = [span("a", 0.0, 1.3), span("b", 1.3, 4.0)];
        assert_eq!(
            align_words_to_speakers(&words, &spans),
            vec![Speaker::Remote { index: 2 }]
        );
    }

    #[test]
    fn a_word_with_no_overlap_is_unknown_rather_than_guessed() {
        let words = [word("orphan", 10.0, 10.5)];
        let spans = [span("a", 0.0, 5.0)];
        assert_eq!(align_words_to_speakers(&words, &spans), vec![Speaker::Unknown]);
    }

    #[test]
    fn an_exactly_ambiguous_overlap_is_unknown_rather_than_guessed() {
        let words = [word("split", 1.0, 3.0)];
        let spans = [span("a", 0.0, 2.0), span("b", 2.0, 4.0)];
        assert_eq!(align_words_to_speakers(&words, &spans), vec![Speaker::Unknown]);
    }

    #[test]
    fn segments_split_on_speaker_change_and_on_long_pauses() {
        let words = [
            word("one", 0.0, 0.5),
            word("two", 0.6, 1.0),
            // Same speaker, but a 3s pause.
            word("three", 4.0, 4.5),
        ];
        let speakers = [Speaker::MeetingAudio; 3];
        let segments = group_words_into_segments(Track::Meeting, &words, &speakers);

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "one two");
        assert_eq!(segments[1].text, "three");
        assert!((segments[0].end_seconds - 1.0).abs() < 1e-9);
    }

    #[test]
    fn microphone_track_is_always_labelled_as_the_local_user() {
        let raw = RawTrackResult {
            words: vec![word("hi", 0.0, 0.4)],
            // A provider offering diarization on the mic track is ignored:
            // Blue Ear already knows whose microphone this is.
            speaker_spans: vec![span("a", 0.0, 1.0)],
            model_id: None,
            language: None,
        };
        let result = build_track_transcript(Track::Microphone, ProviderId::FluidAudio, &raw);

        assert!(!result.diarized);
        assert_eq!(result.speaker_count, 0);
        assert_eq!(result.segments[0].speaker, Speaker::You);
    }

    #[test]
    fn meeting_track_without_diarization_falls_back_to_a_track_label() {
        let raw = RawTrackResult {
            words: vec![word("agenda", 0.0, 0.8)],
            speaker_spans: vec![],
            model_id: None,
            language: None,
        };
        let result = build_track_transcript(Track::Meeting, ProviderId::AppleSpeech, &raw);

        assert!(!result.diarized);
        assert_eq!(result.segments[0].speaker, Speaker::MeetingAudio);
    }

    #[test]
    fn meeting_track_with_diarization_counts_distinct_remote_speakers() {
        let raw = RawTrackResult {
            words: vec![word("hello", 0.0, 1.0), word("hi", 2.0, 3.0)],
            speaker_spans: vec![span("a", 0.0, 1.5), span("b", 1.5, 3.5)],
            model_id: Some("parakeet".into()),
            language: Some("en".into()),
        };
        let result = build_track_transcript(Track::Meeting, ProviderId::FluidAudio, &raw);

        assert!(result.diarized);
        assert_eq!(result.speaker_count, 2);
    }

    #[test]
    fn merge_orders_by_time_and_is_deterministic_on_ties() {
        let meeting = TrackTranscript {
            track: Track::Meeting,
            provider: ProviderId::FluidAudio,
            model_id: None,
            language: None,
            diarized: false,
            speaker_count: 0,
            segments: vec![
                Segment {
                    track: Track::Meeting,
                    speaker: Speaker::MeetingAudio,
                    start_seconds: 5.0,
                    end_seconds: 6.0,
                    text: "later".into(),
                    words: vec![],
                },
                Segment {
                    track: Track::Meeting,
                    speaker: Speaker::MeetingAudio,
                    start_seconds: 1.0,
                    end_seconds: 2.0,
                    text: "tie".into(),
                    words: vec![],
                },
            ],
        };
        let mic = TrackTranscript {
            track: Track::Microphone,
            provider: ProviderId::FluidAudio,
            model_id: None,
            language: None,
            diarized: false,
            speaker_count: 0,
            segments: vec![Segment {
                track: Track::Microphone,
                speaker: Speaker::You,
                // Same start time as the meeting segment above.
                start_seconds: 1.0,
                end_seconds: 2.0,
                text: "tie".into(),
                words: vec![],
            }],
        };

        let merged = merge_tracks(&[meeting.clone(), mic.clone()]);
        assert_eq!(
            merged.iter().map(|s| s.track).collect::<Vec<_>>(),
            vec![Track::Meeting, Track::Microphone, Track::Meeting]
        );

        // Feeding the tracks in the opposite order must not change anything.
        assert_eq!(merge_tracks(&[mic, meeting]), merged);
    }
}
