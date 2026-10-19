//! beepd 内部で使う ToneBackend 実装 (beep-player の実装と同等)。
//!
//! 将来 beep-player 側と共通化しても良いが、まずはコピーを持たせて
//! それぞれ独立に進化できるようにする。

use anyhow::{Context, Result};
use std::process::Command;
use std::time::Duration;
use tracing::{debug, warn};

use crate::config::BackendConfig;

pub trait ToneBackend: Send {
    fn tone(&mut self, freq_hz: u32, duration: Duration) -> Result<()>;
    fn rest(&mut self, duration: Duration) -> Result<()> {
        std::thread::sleep(duration);
        Ok(())
    }
    /// 連続する短い音のバッチ発音。デフォルトは 1 音ずつ tone を呼ぶが、
    /// beep バックエンドは 1 プロセスで `-f X -l Y -n -f X2 -l Y2 -n ...`
    /// と連結して呼び出すことで、プロセス起動オーバヘッドを 1 回に抑える。
    fn tone_sequence(&mut self, seq: &[(u32, Duration)]) -> Result<()> {
        for (f, d) in seq {
            self.tone(*f, *d)?;
        }
        Ok(())
    }
}

pub fn build(cfg: &BackendConfig) -> Box<dyn ToneBackend> {
    if cfg.dry_run {
        Box::new(DryRunBackend::default())
    } else {
        Box::new(BeepBackend { cfg: cfg.clone() })
    }
}

pub struct BeepBackend {
    cfg: BackendConfig,
}

/// 周波数に pitch_shift_octaves を適用する (20Hz-20kHz にクランプ)。
fn shift_freq(freq_hz: u32, octaves: i8) -> u32 {
    if octaves == 0 {
        return freq_hz.clamp(20, 20_000);
    }
    let f = f64::from(freq_hz) * 2f64.powi(i32::from(octaves));
    (f.round() as i64).clamp(20, 20_000) as u32
}

/// 要求長・gap・gain_percent から (発音 ms, 直後の delay ms) を算出する。
/// 合計は必ず requested に一致し、engine 側のタイミング補正とズレない。
fn shape_note(requested_ms: u64, gap_ms: u64, min_ms: u64, gain_percent: u8) -> (u64, u64) {
    let base_sound = requested_ms.saturating_sub(gap_ms).max(1);
    let sound_ms = if gain_percent >= 100 {
        // 通常モード: min_ms を下限として鳴らす
        base_sound.max(min_ms).max(1)
    } else {
        // 減衰モード: base_sound を gain_percent% にする (min_ms は無視して短くしていい)
        let g = u64::from(gain_percent).min(100);
        ((base_sound as u128) * (g as u128) / 100).max(1) as u64
    };
    // sound_ms が requested を超える (短音で min_ms が効いた) 場合は delay 0
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
    /// これで同一プロセス内でも刻みが表現される (`-n` だけだと隙間なく連結される)。
    /// 各音の (発音, delay) は shape_note で決めるので gain_percent の減衰は
    /// 発音側に集約され、delay 側に差分が乗ってタイミングが要求長に一致する。
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
            // バッチではプロセス起動オーバヘッドが無いので min_ms 縛りは緩めていい
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
