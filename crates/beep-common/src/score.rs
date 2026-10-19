//! 時刻付き Note へのフラット化。
//!
//! MIDI の Note On / Note Off を、開始/終了時刻 (秒) と MIDI 番号を持つ
//! `Note` 列 (=Score) に変換する。テンポ変更にも追従する。

use serde::{Deserialize, Serialize};

/// 一つの音符。時刻は演奏開始からの経過秒。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// MIDI ノート番号 (0-127)
    pub key: u8,
    /// Velocity (0-127, Note On のもの)
    pub velocity: u8,
    /// トラック番号 (SMF 上の順序)
    pub track: u16,
    /// チャンネル (0-15)
    pub channel: u8,
    /// 開始時刻 (秒)
    pub start: f64,
    /// 終了時刻 (秒)
    pub end: f64,
}

impl Note {
    #[inline]
    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }

    /// ある時刻 `t` にこの音が鳴っているか (開始時刻 <= t < 終了時刻)。
    #[inline]
    pub fn is_active_at(&self, t: f64) -> bool {
        self.start <= t && t < self.end
    }
}

/// 一曲まるごとのスコア。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Score {
    pub notes: Vec<Note>,
    /// 曲全体の長さ (秒)。
    pub duration: f64,
    /// 元 SMF のタイトル (あれば)。
    pub title: Option<String>,
}

impl Score {
    pub fn new(mut notes: Vec<Note>, title: Option<String>) -> Self {
        // start でソートしておくと後の処理が単純化できる
        notes.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        let duration = notes.iter().map(|n| n.end).fold(0.0_f64, f64::max);
        Self {
            notes,
            duration,
            title,
        }
    }
}
