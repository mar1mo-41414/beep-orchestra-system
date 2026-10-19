//! モノフォニックプレイヤー。
//!
//! Score → monophonic stream → バックエンドへ逐次発音。
//! ドリフト補正のため wall clock 基準で残り時間を計算する。

use anyhow::Result;
use beep_common::pitch::midi_to_hz_rounded;
use beep_common::score::Score;
use beep_common::select::{monophonic_stream, SelectPolicy};
use std::time::{Duration, Instant};
use tracing::info;

use crate::backend::ToneBackend;

pub struct MonoPlayer {
    backend: Box<dyn ToneBackend>,
    policy: SelectPolicy,
}

impl MonoPlayer {
    pub fn new(backend: Box<dyn ToneBackend>, policy: SelectPolicy) -> Self {
        Self { backend, policy }
    }

    pub fn play(&mut self, score: &Score) -> Result<()> {
        let events = monophonic_stream(score, self.policy);
        info!(events = events.len(), "playing");

        let started = Instant::now();
        let mut i = 0;
        while i < events.len() {
            let ev = events[i];
            let target = Duration::from_secs_f64(ev.start.max(0.0));
            let now = started.elapsed();
            if target > now {
                std::thread::sleep(target - now);
            }
            match ev.key {
                None => {
                    let dur = Duration::from_secs_f64(ev.duration().max(0.0));
                    self.backend.rest(dur)?;
                    i += 1;
                }
                Some(k0) => {
                    // 隣接ノート (次の start が現在の end と一致) をバッチ化
                    let mut batch: Vec<(u32, Duration)> = vec![(
                        midi_to_hz_rounded(k0),
                        Duration::from_secs_f64(ev.duration().max(0.0)),
                    )];
                    let mut j = i + 1;
                    let mut prev_end = ev.end;
                    let mut batch_ms: f64 = ev.duration().max(0.0) * 1000.0;
                    const BATCH_MAX_LEN: usize = 16;
                    const BATCH_MAX_MS: f64 = 200.0;
                    while j < events.len()
                        && batch.len() < BATCH_MAX_LEN
                        && batch_ms < BATCH_MAX_MS
                    {
                        let nxt = events[j];
                        if let Some(k) = nxt.key {
                            if (nxt.start - prev_end).abs() < 0.002 {
                                let d = nxt.duration().max(0.0);
                                batch.push((midi_to_hz_rounded(k), Duration::from_secs_f64(d)));
                                batch_ms += d * 1000.0;
                                prev_end = nxt.end;
                                j += 1;
                                continue;
                            }
                        }
                        break;
                    }
                    if batch.len() == 1 {
                        self.backend.tone(batch[0].0, batch[0].1)?;
                    } else {
                        self.backend.tone_sequence(&batch)?;
                    }
                    i = j;
                }
            }
        }
        Ok(())
    }
}
