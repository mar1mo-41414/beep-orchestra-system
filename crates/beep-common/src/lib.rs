//! beep-orchestra 共通ライブラリ。
//!
//! - `midi`: SMF (Standard MIDI File) 解析
//! - `score`: 時刻付き Note へのフラット化
//! - `pitch`: MIDI ノート番号 ↔ 周波数
//! - `select`: 単ノード再生用の選音アルゴリズム
//! - `node`: LAN ノード情報 (Phase3 以降で使用)

pub mod midi;
pub mod node;
pub mod pitch;
pub mod score;
pub mod select;
pub mod voicing;

pub use score::{Note, Score};
pub use select::{monophonic_stream, SelectPolicy};
pub use voicing::{assign as assign_voicing, assign_tracks, assign_voices, VoicingStrategy};
