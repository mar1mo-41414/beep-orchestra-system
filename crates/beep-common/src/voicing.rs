//! パート割り当て (voicing) アルゴリズム。
//!
//! 2 種類の戦略:
//!
//! - **Pitch**: 各 Note に「同時発音中の音の高さ降順ランク」を割り当て、
//!   ランク i の音を「ボイス i」に振り分ける。ノード数 N より多い同時音は
//!   下位から捨てる (=優先順位で切り捨て)。単一トラックしかないスコアや
//!   トライアド展開に便利。
//!
//! - **Track**: SMF のトラックを 1 声 = 1 ノードに単純対応させる。
//!   DAW で「メロディ」「ハーモニー」「ベース」と別トラックに分けた
//!   スコアで、どの声部をどのノードに乗せるかを意図通りに制御できる。
//!   音符が 0 個のトラック (Tempo 等のメタトラック) は自動でスキップされる。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::score::{Note, Score};

/// パート割り当ての戦略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VoicingStrategy {
    /// 高音降順ランクで割り当てる (デフォルト)
    #[default]
    Pitch,
    /// SMF のトラック順で割り当てる
    Track,
}

/// 戦略に応じて `assign_voices_*` を呼び分ける。
pub fn assign(score: &Score, n_voices: usize, strategy: VoicingStrategy) -> Vec<Score> {
    match strategy {
        VoicingStrategy::Pitch => assign_voices(score, n_voices),
        VoicingStrategy::Track => assign_tracks(score, n_voices),
    }
}

/// スコアを SMF トラック順に最大 `n_voices` 個へ分割する。
///
/// - 音符 0 個のトラック (テンポメタトラック等) は無視
/// - トラック順は SMF の並び順を保つ (BTreeMap で track 番号昇順)
/// - トラック数 > n_voices なら先頭 n_voices 個のみ採用 (残りは切り捨て)
/// - トラック数 < n_voices なら余った voice は空の Score
pub fn assign_tracks(score: &Score, n_voices: usize) -> Vec<Score> {
    let mut by_track: BTreeMap<u16, Vec<Note>> = BTreeMap::new();
    for note in &score.notes {
        by_track.entry(note.track).or_default().push(*note);
    }
    // 音符の無いトラックは無視
    let mut track_scores: Vec<Score> = by_track
        .into_iter()
        .filter(|(_, notes)| !notes.is_empty())
        .map(|(_, notes)| Score::new(notes, score.title.clone()))
        .collect();
    track_scores.truncate(n_voices);
    while track_scores.len() < n_voices {
        track_scores.push(Score::default());
    }
    track_scores
}

/// スコアを最大 `n_voices` 個のボイス Score に分割する。
///
/// - ボイス 0 = 最高音側
/// - ボイス N-1 = 最低音側
/// - N 個を超える同時音は捨てる
pub fn assign_voices(score: &Score, n_voices: usize) -> Vec<Score> {
    let mut voices: Vec<Vec<Note>> = (0..n_voices).map(|_| Vec::new()).collect();
    if n_voices == 0 {
        return voices.into_iter().map(|_| Score::default()).collect();
    }

    for (i, note) in score.notes.iter().enumerate() {
        let t = note.start;
        // 開始時点で鳴っている音のインデックス集合を集める
        let mut concurrent: Vec<(usize, u8)> = score
            .notes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_active_at(t))
            .map(|(j, n)| (j, n.key))
            .collect();
        // 高い音から順に voice 0, 1, 2, ...
        concurrent.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let rank = concurrent
            .iter()
            .position(|(j, _)| *j == i)
            .unwrap_or(usize::MAX);
        if rank < n_voices {
            voices[rank].push(*note);
        }
    }

    voices
        .into_iter()
        .map(|notes| Score::new(notes, score.title.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::Note;

    fn n(key: u8, s: f64, e: f64) -> Note {
        Note {
            key,
            velocity: 100,
            track: 0,
            channel: 0,
            start: s,
            end: e,
        }
    }

    #[test]
    fn triad_splits_to_three_voices() {
        // Cメジャートライアド C(60) E(64) G(67) を 3 ボイスへ
        let score = Score::new(vec![n(60, 0.0, 1.0), n(64, 0.0, 1.0), n(67, 0.0, 1.0)], None);
        let voices = assign_voices(&score, 3);
        assert_eq!(voices.len(), 3);
        assert_eq!(voices[0].notes[0].key, 67); // 最高音
        assert_eq!(voices[1].notes[0].key, 64);
        assert_eq!(voices[2].notes[0].key, 60);
    }

    #[test]
    fn triad_with_two_voices_drops_bass() {
        // C E G だけどノード 2 台 → 下 (C) が捨てられる
        let score = Score::new(vec![n(60, 0.0, 1.0), n(64, 0.0, 1.0), n(67, 0.0, 1.0)], None);
        let voices = assign_voices(&score, 2);
        assert_eq!(voices[0].notes[0].key, 67);
        assert_eq!(voices[1].notes[0].key, 64);
        // Cは捨てられる
        assert!(voices.iter().all(|v| v.notes.iter().all(|n| n.key != 60)));
    }

    #[test]
    fn assign_tracks_maps_smf_tracks_to_voices() {
        // track 1 = メロディ (C E G ...), track 2 = ベース (E3 のみ)
        let notes = vec![
            Note { key: 60, velocity: 100, track: 1, channel: 0, start: 0.0, end: 0.5 },
            Note { key: 64, velocity: 100, track: 1, channel: 0, start: 0.5, end: 1.0 },
            Note { key: 67, velocity: 100, track: 1, channel: 0, start: 1.0, end: 1.5 },
            Note { key: 52, velocity: 100, track: 2, channel: 0, start: 0.0, end: 1.5 },
        ];
        let score = Score::new(notes, None);
        let voices = assign_tracks(&score, 2);
        assert_eq!(voices.len(), 2);
        let v0: Vec<u8> = voices[0].notes.iter().map(|n| n.key).collect();
        let v1: Vec<u8> = voices[1].notes.iter().map(|n| n.key).collect();
        assert_eq!(v0, vec![60, 64, 67]);
        assert_eq!(v1, vec![52]);
    }

    #[test]
    fn assign_tracks_skips_empty_tempo_track() {
        // track 0 = tempo (音符無し), track 1 = メロディ
        let notes = vec![Note {
            key: 60, velocity: 100, track: 1, channel: 0, start: 0.0, end: 0.5,
        }];
        let score = Score::new(notes, None);
        let voices = assign_tracks(&score, 1);
        assert_eq!(voices[0].notes.len(), 1);
        assert_eq!(voices[0].notes[0].key, 60);
    }

    #[test]
    fn melody_stays_on_top_voice_across_chord_changes() {
        // 0-1s: C-E (2音), 1-2s: D-F#-A (3音)
        // ボイス 0 は 最高音のみを追う: E → A
        let score = Score::new(
            vec![
                n(60, 0.0, 1.0),
                n(64, 0.0, 1.0),
                n(62, 1.0, 2.0),
                n(66, 1.0, 2.0),
                n(69, 1.0, 2.0),
            ],
            None,
        );
        let voices = assign_voices(&score, 3);
        // ボイス0: 64, 69
        let v0: Vec<u8> = voices[0].notes.iter().map(|n| n.key).collect();
        assert_eq!(v0, vec![64, 69]);
    }
}
