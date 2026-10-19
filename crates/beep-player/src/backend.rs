//! 音を鳴らすバックエンド。デフォルト実装は `beep` 外部コマンド。
//!
//! 将来 rodio / ALSA / speaker-test 等に差し替えられるよう trait 化しておく。

use anyhow::{Context, Result};
use std::process::Command;
use std::time::Duration;
use tracing::{debug, warn};

use crate::config::BackendConfig;

/// 単音を鳴らすバックエンド。
///
/// - `tone`: 周波数 Hz と持続時間で鳴らす。
/// - `rest`: 何も鳴らさず持続時間ぶん待つ。
pub trait ToneBackend {
    fn tone(&mut self, freq_hz: u32, duration: Duration) -> Result<()>;
    fn rest(&mut self, duration: Duration) -> Result<()> {
        std::thread::sleep(duration);
        Ok(())
    }
    /// 連続する短い音を 1 プロセスで発音する。デフォルトは 1 音ずつ。
    fn tone_sequence(&mut self, seq: &[(u32, Duration)]) -> Result<()> {
        for (f, d) in seq {
            self.tone(*f, *d)?;
        }
        Ok(())
    }
}

pub fn build(cfg: &BackendConfig) -> Result<Box<dyn ToneBackend>> {
    if cfg.dry_run {
        Ok(Box::new(DryRunBackend::default()))
    } else {
        Ok(Box::new(BeepBackend::new(cfg.clone())))
    }
}

/// `beep` 外部コマンドを叩くバックエンド。
pub struct BeepBackend {
    cfg: BackendConfig,
}

impl BeepBackend {
    pub fn new(cfg: BackendConfig) -> Self {
        Self { cfg }
    }
}

fn shift_freq(freq_hz: u32, octaves: i8) -> u32 {
    if octaves == 0 {
        return freq_hz.clamp(20, 20_000);
    }
    let f = f64::from(freq_hz) * 2f64.powi(i32::from(octaves));
    (f.round() as i64).clamp(20, 20_000) as u32
}

fn shape_note(requested_ms: u64, gap_ms: u64, min_ms: u64, gain_percent: u8) -> (u64, u64) {
    let base_sound = requested_ms.saturating_sub(gap_ms).max(1);
    let sound_ms = if gain_percent >= 100 {
        base_sound.max(min_ms).max(1)
    } else {
        let g = u64::from(gain_percent).min(100);
        ((base_sound as u128) * (g as u128) / 100).max(1) as u64
    };
    let delay_ms = requested_ms.saturating_sub(sound_ms);
    (sound_ms, delay_ms)
}

impl ToneBackend for BeepBackend {
    fn tone(&mut self, freq_hz: u32, duration: Duration) -> Result<()> {
        let gap = u64::from(self.cfg.inter_note_gap_ms);
        let min_ms = u64::from(self.cfg.min_duration_ms);
        let requested = duration.as_millis().min(u64::MAX as u128) as u64;
        let (sound_ms, delay_ms) = shape_note(requested, gap, min_ms, self.cfg.gain_percent);
        let freq = shift_freq(freq_hz, self.cfg.pitch_shift_octaves);
        // beep -f <hz> -l <ms> [extra_args...]
        let mut cmd = Command::new(&self.cfg.command);
        cmd.arg("-f")
            .arg(freq.to_string())
            .arg("-l")
            .arg(sound_ms.to_string());
        for a in &self.cfg.extra_args {
            cmd.arg(a);
        }
        debug!(?cmd, "invoking beep");
        let status = cmd
            .status()
            .with_context(|| format!("spawn beep: {}", self.cfg.command))?;
        if !status.success() {
            warn!(?status, freq_hz = freq, ms = sound_ms, "beep exited non-zero");
        }
        if delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
        Ok(())
    }

    /// バッチ発音: 各音の後に `-D <delay>` を挟んで
    /// `beep -f F1 -l L1 -D D1 -n -f F2 -l L2 -D D2 -n ...` の形にする。
    fn tone_sequence(&mut self, seq: &[(u32, Duration)]) -> Result<()> {
        if seq.is_empty() {
            return Ok(());
        }
        if seq.len() == 1 {
            return self.tone(seq[0].0, seq[0].1);
        }
        let gap = u64::from(self.cfg.inter_note_gap_ms);
        let min_ms = u64::from(self.cfg.min_duration_ms);
        let mut cmd = Command::new(&self.cfg.command);
        for (i, (freq, dur)) in seq.iter().enumerate() {
            if i > 0 {
                cmd.arg("-n");
            }
            let requested = dur.as_millis().min(u64::MAX as u128) as u64;
            let (sound_ms, delay_ms) =
                shape_note(requested, gap, min_ms.min(1), self.cfg.gain_percent);
            let f = shift_freq(*freq, self.cfg.pitch_shift_octaves);
            cmd.arg("-f").arg(f.to_string()).arg("-l").arg(sound_ms.to_string());
            if delay_ms > 0 {
                cmd.arg("-D").arg(delay_ms.to_string());
            }
        }
        for a in &self.cfg.extra_args {
            cmd.arg(a);
        }
        debug!(?cmd, batch_len = seq.len(), "invoking beep (batch)");
        let status = cmd
            .status()
            .with_context(|| format!("spawn beep (batch): {}", self.cfg.command))?;
        if !status.success() {
            warn!(?status, batch_len = seq.len(), "beep (batch) exited non-zero");
        }
        Ok(())
    }
}

/// 実際には鳴らさず、ログのみ吐くバックエンド。開発/CI 用。
#[derive(Debug, Default)]
pub struct DryRunBackend;

impl ToneBackend for DryRunBackend {
    fn tone(&mut self, freq_hz: u32, duration: Duration) -> Result<()> {
        tracing::info!(freq_hz, ms = duration.as_millis(), "TONE (dry-run)");
        std::thread::sleep(duration);
        Ok(())
    }
    fn rest(&mut self, duration: Duration) -> Result<()> {
        tracing::info!(ms = duration.as_millis(), "REST (dry-run)");
        std::thread::sleep(duration);
        Ok(())
    }
    fn tone_sequence(&mut self, seq: &[(u32, Duration)]) -> Result<()> {
        let total: u128 = seq.iter().map(|(_, d)| d.as_millis()).sum();
        tracing::info!(batch_len = seq.len(), total_ms = total, "TONE-SEQ (dry-run)");
        for (_, d) in seq {
            std::thread::sleep(*d);
        }
        Ok(())
    }
}
