//! 再生エンジン。専用スレッドで動き、Play/Pause/Resume/Stop を制御チャネルで受け取る。
//!
//! 現段階は「単ノード・モノフォニック」の再生。Phase4 でパート割り当てに拡張する。

use anyhow::Result;
use beep_common::pitch::midi_to_hz_rounded;
use beep_common::score::Score;
use beep_common::select::{monophonic_stream, MonoEvent, SelectPolicy};
use serde::Serialize;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::backend::{build as build_backend, ToneBackend};
use crate::config::BackendConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    Idle,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlaybackStatus {
    pub state: PlaybackState,
    pub title: Option<String>,
    pub position_sec: f64,
    pub duration_sec: f64,
    pub notes: usize,
}

impl PlaybackStatus {
    fn idle() -> Self {
        Self {
            state: PlaybackState::Idle,
            title: None,
            position_sec: 0.0,
            duration_sec: 0.0,
            notes: 0,
        }
    }
}

pub enum Command {
    Play {
        score: Score,
        policy: SelectPolicy,
        /// Absolute wall-clock start time (unix epoch seconds). None → 即時開始。
        start_at_epoch: Option<f64>,
    },
    Pause,
    Resume,
    Stop,
}

#[derive(Clone)]
pub struct Engine {
    tx: mpsc::Sender<Command>,
    status: Arc<Mutex<PlaybackStatus>>,
}

impl Engine {
    pub fn spawn(backend_cfg: BackendConfig) -> Self {
        let (tx, rx) = mpsc::channel::<Command>();
        let status = Arc::new(Mutex::new(PlaybackStatus::idle()));

        let status_bg = status.clone();
        thread::spawn(move || {
            let mut backend = build_backend(&backend_cfg);
            run_engine(rx, status_bg, backend.as_mut());
        });

        Self { tx, status }
    }

    pub fn send(&self, cmd: Command) -> Result<()> {
        self.tx
            .send(cmd)
            .map_err(|e| anyhow::anyhow!("engine channel closed: {e}"))
    }

    pub fn status(&self) -> PlaybackStatus {
        self.status.lock().unwrap().clone()
    }
}

fn run_engine(
    rx: mpsc::Receiver<Command>,
    status: Arc<Mutex<PlaybackStatus>>,
    backend: &mut dyn ToneBackend,
) {
    loop {
        // Idle: block for next command
        let cmd = match rx.recv() {
            Ok(c) => c,
            Err(_) => return,
        };
        match cmd {
            Command::Play {
                score,
                policy,
                start_at_epoch,
            } => {
                if let Err(e) = play_score(&rx, &status, backend, score, policy, start_at_epoch) {
                    warn!(error = %e, "playback error");
                }
                *status.lock().unwrap() = PlaybackStatus::idle();
            }
            Command::Pause | Command::Resume | Command::Stop => {
                // Idle 中に来たコマンドは無視 (状態は Idle のまま)
            }
        }
    }
}

fn play_score(
    rx: &mpsc::Receiver<Command>,
    status: &Arc<Mutex<PlaybackStatus>>,
    backend: &mut dyn ToneBackend,
    score: Score,
    policy: SelectPolicy,
    start_at_epoch: Option<f64>,
) -> Result<()> {
    let events: Vec<MonoEvent> = monophonic_stream(&score, policy);
    let total = score.duration;
    let title = score.title.clone();
    {
        let mut s = status.lock().unwrap();
        *s = PlaybackStatus {
            state: PlaybackState::Playing,
            title: title.clone(),
            position_sec: 0.0,
            duration_sec: total,
            notes: events.len(),
        };
    }
    info!(events = events.len(), duration = total, ?title, "engine: play");

    // 絶対時刻指定があれば、その時刻まで待つ (同期再生)
    if let Some(epoch) = start_at_epoch {
        wait_until_epoch(epoch);
    }

    let mut started = Instant::now();
    // 一時停止で経過した累計時間 (これで started を伸ばす)
    let mut paused_total = Duration::ZERO;

    let mut i = 0;
    while i < events.len() {
        // pending ドレイン: Pause/Stop に応答
        loop {
            match rx.try_recv() {
                Ok(Command::Stop) => {
                    info!("engine: stop");
                    return Ok(());
                }
                Ok(Command::Pause) => {
                    let pause_start = Instant::now();
                    status.lock().unwrap().state = PlaybackState::Paused;
                    info!("engine: pause");
                    loop {
                        match rx.recv() {
                            Ok(Command::Resume) => {
                                let elapsed = pause_start.elapsed();
                                paused_total += elapsed;
                                started = started + elapsed;
                                status.lock().unwrap().state = PlaybackState::Playing;
                                info!(paused_ms = elapsed.as_millis(), "engine: resume");
                                break;
                            }
                            Ok(Command::Stop) => {
                                info!("engine: stop (from pause)");
                                return Ok(());
                            }
                            Ok(_) => {}
                            Err(_) => return Ok(()),
                        }
                    }
                }
                Ok(Command::Play { .. }) => {
                    warn!("engine: ignoring Play during playback");
                }
                Ok(Command::Resume) => {}
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }

        let ev = events[i];
        // ev.start に達するまで待機
        let target = Duration::from_secs_f64(ev.start.max(0.0));
        let now = started.elapsed();
        if target > now {
            std::thread::sleep(target - now);
        }
        // 位置を更新
        status.lock().unwrap().position_sec = ev.start;

        match ev.key {
            None => {
                let dur = Duration::from_secs_f64(ev.duration().max(0.0));
                backend.rest(dur)?;
                i += 1;
            }
            Some(k0) => {
                // 隣接する後続ノートを 1 バッチにまとめる
                // 「隣接」= 次の start が現在の end とほぼ一致 (2ms 以内)
                let mut batch: Vec<(u32, Duration)> = vec![(
                    midi_to_hz_rounded(k0),
                    Duration::from_secs_f64(ev.duration().max(0.0)),
                )];
                let mut j = i + 1;
                let mut prev_end = ev.end;
                let mut batch_ms: f64 = ev.duration().max(0.0) * 1000.0;
                // Pause/Stop の応答性を保つため、バッチは短めで打ち切る
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
                    backend.tone(batch[0].0, batch[0].1)?;
                } else {
                    backend.tone_sequence(&batch)?;
                }
                i = j;
            }
        }
    }

    let _ = paused_total;
    Ok(())
}

fn wait_until_epoch(epoch_secs: f64) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let delta = epoch_secs - now;
    if delta > 0.0 {
        info!(wait_sec = delta, "waiting for synchronized start");
        std::thread::sleep(Duration::from_secs_f64(delta));
    }
}
