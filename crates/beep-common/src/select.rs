//! 単ノード再生用の選音アルゴリズム。
//!
//! モノフォニックな出力を生成する。同時に鳴る音の中から
//! ポリシーに従って 1 音を選び、時刻順の「イベント列」に変換する。

use serde::{Deserialize, Serialize};

use crate::score::{Note, Score};

/// 同時発音時にどの音を採用するか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SelectPolicy {
    /// 最高音 (typical: 主旋律を取り出す)
    #[default]
    Highest,
    /// 最低音 (bass line)
    Lowest,
    /// 最初に鳴り始めた音 (先着優先)
    First,
    /// 最後に鳴り始めた音 (割り込み優先)
    Last,
}

/// モノフォニックの再生イベント。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonoEvent {
    /// このイベント区間の開始秒 (演奏開始からの経過)
    pub start: f64,
    /// このイベント区間の終了秒
    pub end: f64,
    /// この区間で鳴らす音。None = 無音区間 (休符)。
    pub key: Option<u8>,
}

impl MonoEvent {
    #[inline]
    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }
}

/// スコアをモノフォニックなイベント列に変換する。
///
/// アルゴリズム: 全 Note の start/end を境界タイムとして sweep line で
/// 走らせ、各区間ごとにアクティブな Note 集合からポリシーで 1 音を選ぶ。
pub fn monophonic_stream(score: &Score, policy: SelectPolicy) -> Vec<MonoEvent> {
    if score.notes.is_empty() {
        return Vec::new();
    }

    // 境界時刻を収集し重複除去 + ソート
    let mut boundaries: Vec<f64> = Vec::with_capacity(score.notes.len() * 2);
    for n in &score.notes {
        boundaries.push(n.start);
        boundaries.push(n.end);
    }
    boundaries.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    boundaries.dedup_by(|a, b| (*a - *b).abs() < 1e-9);

    let mut events: Vec<MonoEvent> = Vec::new();

    for w in boundaries.windows(2) {
        let (t0, t1) = (w[0], w[1]);
        if t1 - t0 < 1e-9 {
            continue;
        }
        let mid = t0; // 区間の左端で判定 (start<=t<end)
        let key = pick(&score.notes, mid, policy);
        // 直前と同じ休符区間 (None) だけ結合する。
        // 同一キーの連続打鍵 (再アタック) は MIDI 上で別ノートなので結合しない。
        // 結合してしまうと「刻み」が長音に化けるため。
        if let Some(prev) = events.last_mut() {
            if prev.key.is_none() && key.is_none() && (prev.end - t0).abs() < 1e-9 {
                prev.end = t1;
                continue;
            }
        }
        events.push(MonoEvent {
            start: t0,
            end: t1,
            key,
        });
    }

    events
}

fn pick(notes: &[Note], t: f64, policy: SelectPolicy) -> Option<u8> {
    // アクティブな音を集める (小さいスコアなら十分速い)
    let active: Vec<&Note> = notes.iter().filter(|n| n.is_active_at(t)).collect();
    if active.is_empty() {
        return None;
    }
    let chosen = match policy {
        SelectPolicy::Highest => active.iter().max_by_key(|n| n.key)?,
        SelectPolicy::Lowest => active.iter().min_by_key(|n| n.key)?,
        SelectPolicy::First => active
            .iter()
            .min_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal))?,
        SelectPolicy::Last => active
            .iter()
            .max_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal))?,
    };
    Some(chosen.key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::Note;

    fn n(key: u8, start: f64, end: f64) -> Note {
        Note {
            key,
            velocity: 100,
            track: 0,
            channel: 0,
            start,
            end,
        }
    }

    #[test]
    fn highest_picks_top_note_in_chord() {
        // C-E-G の和音 (60,64,67) を 0..1 秒に鳴らす
        let score = Score::new(vec![n(60, 0.0, 1.0), n(64, 0.0, 1.0), n(67, 0.0, 1.0)], None);
        let ev = monophonic_stream(&score, SelectPolicy::Highest);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].key, Some(67));
    }

    #[test]
    fn lowest_policy_picks_bass() {
        let score = Score::new(vec![n(60, 0.0, 1.0), n(64, 0.0, 1.0), n(67, 0.0, 1.0)], None);
        let ev = monophonic_stream(&score, SelectPolicy::Lowest);
        assert_eq!(ev[0].key, Some(60));
    }

    #[test]
    fn rest_is_produced_between_notes() {
        // 0-1 秒に C, 2-3 秒に E。1-2 秒は休符になるはず。
        let score = Score::new(vec![n(60, 0.0, 1.0), n(64, 2.0, 3.0)], None);
        let ev = monophonic_stream(&score, SelectPolicy::Highest);
        assert_eq!(ev.len(), 3);
        assert_eq!(ev[0].key, Some(60));
        assert_eq!(ev[1].key, None); // 休符
        assert_eq!(ev[2].key, Some(64));
    }

    #[test]
    fn repeated_same_pitch_stays_split() {
        // 同じ 60 を 0-1s / 1-2s と隣接して 2 回打鍵。
        // 「刻み」を長音に化かさないため、結合せず 2 イベントで出す。
        let score = Score::new(vec![n(60, 0.0, 1.0), n(60, 1.0, 2.0)], None);
        let ev = monophonic_stream(&score, SelectPolicy::Highest);
        assert_eq!(ev.len(), 2, "same-pitch re-attacks must not be merged");
        assert_eq!(ev[0].key, Some(60));
        assert_eq!(ev[1].key, Some(60));
        assert!((ev[0].end - 1.0).abs() < 1e-9);
        assert!((ev[1].start - 1.0).abs() < 1e-9);
    }
}
