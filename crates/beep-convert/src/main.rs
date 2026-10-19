//! beep-convert: MIDI → 内部キャッシュ (JSON) への変換ツール。
//! Phase5 で本格対応。今は情報表示 + JSON dump のみ。

use anyhow::{Context, Result};
use beep_common::midi;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "beep-convert", version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// MIDI の中身を要約表示する
    Info { file: PathBuf },
    /// MIDI をノート列 JSON に変換して stdout に出す
    Dump {
        file: PathBuf,
        /// 出力先。省略時 stdout。
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// N 台のノードにボイス分割した結果を確認する
    Voices {
        file: PathBuf,
        /// ノード数
        #[arg(short, long, default_value_t = 3)]
        voices: usize,
    },
    /// `beep -f X -l Y -D Z -n ...` の羅列を含むシェルスクリプトを解析し
    /// SMF に変換する。
    ///
    /// デフォルトでは以下 3 トラックに振り分ける (Track voicing 前提):
    ///   Track 1: MELODY   (freq >= threshold, 原型)
    ///   Track 2: BASS BEAT (freq <  threshold, 原型)
    ///   Track 3: BASS DRONE (freq <  threshold, 次のベースまで伸ばす)
    ImportBeepsh {
        input: PathBuf,
        output: PathBuf,
        /// メロディ / ベースの分岐周波数 (Hz)
        #[arg(long, default_value_t = 450.0)]
        threshold: f64,
        /// 出力トラック数 (2 or 3)
        #[arg(long, default_value_t = 3)]
        layers: usize,
        /// BASS DRONE の 1 音の最大長 (ms)。長すぎる drone を抑制するため
        #[arg(long, default_value_t = 4000)]
        drone_max_ms: u32,
    },
}

/// 1 個の beep コマンドから解析した (start_ms, freq_hz, duration_ms)
#[derive(Debug, Clone, Copy)]
struct TimedNote {
    start_ms: u64,
    freq_hz: f64,
    duration_ms: u32,
}

/// `beep -f X -l Y [-D Z] -n ...` の羅列から時刻付きノートに変換する。
///
/// 走査規則:
///   - バックスラッシュ改行を空白に変換
///   - 全トークンを空白区切りに分解
///   - `-f FREQ` を見たら「前のノートを確定 (freq/len 揃っていれば)」→
///     新しいノートを開始
///   - `-l LEN` を見たら現在のノート長を設定
///   - `-D DELAY` を見たら現在のノート後の遅延を設定
///   - `-n` は無視 (直後にどうせ `-f` が来る)
///   - 末尾のノートも忘れず確定
fn parse_beepsh(content: &str) -> Vec<TimedNote> {
    // シェルの \\<改行> は 1 行にまとめる
    let joined = content.replace("\\\n", " ");
    let tokens: Vec<&str> = joined.split_ascii_whitespace().collect();

    let mut notes = Vec::new();
    let mut cur_freq: Option<f64> = None;
    let mut cur_len: Option<u32> = None;
    let mut cur_delay: u32 = 0;
    let mut t_ms: u64 = 0;

    let flush = |cur_freq: &mut Option<f64>,
                 cur_len: &mut Option<u32>,
                 cur_delay: &mut u32,
                 t_ms: &mut u64,
                 notes: &mut Vec<TimedNote>| {
        if let (Some(f), Some(l)) = (*cur_freq, *cur_len) {
            notes.push(TimedNote {
                start_ms: *t_ms,
                freq_hz: f,
                duration_ms: l,
            });
            *t_ms += u64::from(l) + u64::from(*cur_delay);
        }
        *cur_freq = None;
        *cur_len = None;
        *cur_delay = 0;
    };

    let mut i = 0;
    while i < tokens.len() {
        match tokens[i] {
            "-f" => {
                flush(&mut cur_freq, &mut cur_len, &mut cur_delay, &mut t_ms, &mut notes);
                if let Some(t) = tokens.get(i + 1) {
                    cur_freq = t.parse().ok();
                }
                i += 2;
            }
            "-l" => {
                if let Some(t) = tokens.get(i + 1) {
                    cur_len = t.parse().ok();
                }
                i += 2;
            }
            "-D" => {
                if let Some(t) = tokens.get(i + 1) {
                    cur_delay = t.parse().unwrap_or(0);
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    flush(&mut cur_freq, &mut cur_len, &mut cur_delay, &mut t_ms, &mut notes);
    notes
}

/// メロディ (freq >= threshold) と ベース (freq < threshold) と
/// サスティン化したベース (次のベースまで伸ばす、上限 drone_max_ms) に分割。
fn split_layers(
    notes: &[TimedNote],
    threshold: f64,
    drone_max_ms: u32,
) -> (Vec<TimedNote>, Vec<TimedNote>, Vec<TimedNote>) {
    let mut melody = Vec::new();
    let mut bass = Vec::new();
    for n in notes {
        if n.freq_hz >= threshold {
            melody.push(*n);
        } else {
            bass.push(*n);
        }
    }
    // BASS DRONE: 次のベース開始まで伸ばす (上限 drone_max_ms)
    let mut drone = bass.clone();
    for i in 0..drone.len() {
        let end_orig = drone[i].start_ms + u64::from(drone[i].duration_ms);
        let next_start = drone
            .get(i + 1)
            .map(|n| n.start_ms)
            .unwrap_or(end_orig);
        let ext = next_start.saturating_sub(drone[i].start_ms);
        let capped = ext.min(u64::from(drone_max_ms));
        drone[i].duration_ms = capped.max(u64::from(drone[i].duration_ms)) as u32;
    }
    (melody, bass, drone)
}

/// 周波数 (Hz) から MIDI ノート番号 (0..=127) へ丸める。
fn freq_to_midi(freq_hz: f64) -> u8 {
    if freq_hz <= 0.0 {
        return 0;
    }
    let n = 12.0 * (freq_hz / 440.0).log2() + 69.0;
    n.round().clamp(0.0, 127.0) as u8
}

/// 複数トラックの TimedNote 群から SMF バイト列を生成。
///
/// タイム基準:
///   - Tempo   = 500,000 μs / quarter (= 120 BPM, SMF 既定値)
///   - TPQ     = 500
///   → 1 tick = 1 ms として扱えるので ms 単位の時刻をそのまま tick に流し込める
///
/// 明示的な Tempo は Track 0 に置くが、値がデフォルトと同じなので、
/// トラックごとに tempo state を独立に持つ簡易パーサでも正しく動く。
fn build_smf(track_sets: &[Vec<TimedNote>]) -> Vec<u8> {
    use midly::num::{u15, u24, u28, u4, u7};
    use midly::{
        Format, Header, MetaMessage, MidiMessage, Smf, Timing, Track, TrackEvent,
        TrackEventKind as K,
    };

    let tpq: u16 = 500;
    let tempo_us: u32 = 500_000;

    let mut tracks: Vec<Track> = Vec::new();

    // Track 0: Tempo のみ
    {
        let mut t: Track = Vec::new();
        t.push(TrackEvent {
            delta: u28::new(0),
            kind: K::Meta(MetaMessage::Tempo(u24::new(tempo_us))),
        });
        t.push(TrackEvent {
            delta: u28::new(0),
            kind: K::Meta(MetaMessage::EndOfTrack),
        });
        tracks.push(t);
    }

    for notes in track_sets {
        let mut t: Track = Vec::new();
        // NoteOn / NoteOff を絶対 tick で並べてから delta 化
        #[derive(Debug)]
        enum Ev {
            On(u8, u8),
            Off(u8),
        }
        let mut evs: Vec<(u64, u8, Ev)> = Vec::new(); // (tick, priority, ev)
        // priority: 同時刻のとき Off を On より先にすると自然
        for n in notes {
            let key = freq_to_midi(n.freq_hz);
            let start = n.start_ms;
            let end = n.start_ms + u64::from(n.duration_ms);
            evs.push((start, 1, Ev::On(key, 100)));
            evs.push((end, 0, Ev::Off(key)));
        }
        evs.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        let mut prev_tick: u64 = 0;
        for (tick, _, ev) in evs {
            let delta = tick.saturating_sub(prev_tick);
            prev_tick = tick;
            let kind = match ev {
                Ev::On(k, v) => K::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOn {
                        key: u7::new(k),
                        vel: u7::new(v),
                    },
                },
                Ev::Off(k) => K::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOff {
                        key: u7::new(k),
                        vel: u7::new(0),
                    },
                },
            };
            t.push(TrackEvent {
                delta: u28::new(delta as u32),
                kind,
            });
        }
        t.push(TrackEvent {
            delta: u28::new(0),
            kind: K::Meta(MetaMessage::EndOfTrack),
        });
        tracks.push(t);
    }

    let smf = Smf {
        header: Header {
            format: Format::Parallel,
            timing: Timing::Metrical(u15::new(tpq)),
        },
        tracks,
    };
    let mut out = Vec::new();
    smf.write(&mut out).unwrap();
    out
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    match args.cmd {
        Cmd::Info { file } => {
            let s = midi::load_from_path(&file)
                .with_context(|| format!("load MIDI: {}", file.display()))?;
            println!(
                "title: {}\nnotes: {}\nduration: {:.3}s",
                s.title.as_deref().unwrap_or("(none)"),
                s.notes.len(),
                s.duration
            );
        }
        Cmd::Dump { file, out } => {
            let s = midi::load_from_path(&file)
                .with_context(|| format!("load MIDI: {}", file.display()))?;
            let j = serde_json::to_string_pretty(&s)?;
            match out {
                Some(p) => {
                    std::fs::write(&p, &j)
                        .with_context(|| format!("write: {}", p.display()))?;
                    eprintln!("wrote {} ({} bytes)", p.display(), j.len());
                }
                None => println!("{}", j),
            }
        }
        Cmd::ImportBeepsh {
            input,
            output,
            threshold,
            layers,
            drone_max_ms,
        } => {
            let content = std::fs::read_to_string(&input)
                .with_context(|| format!("read {}", input.display()))?;
            let notes = parse_beepsh(&content);
            eprintln!(
                "parsed {} notes from {}",
                notes.len(),
                input.display()
            );
            if notes.is_empty() {
                anyhow::bail!("no beep tokens found in {}", input.display());
            }
            let (melody, bass, bass_drone) =
                split_layers(&notes, threshold, drone_max_ms);
            eprintln!(
                "  MELODY (>= {} Hz): {} notes",
                threshold,
                melody.len()
            );
            eprintln!("  BASS BEAT (<  {} Hz): {} notes", threshold, bass.len());
            if layers >= 3 {
                eprintln!(
                    "  BASS DRONE (sustained): {} notes",
                    bass_drone.len()
                );
            }
            let track_sets: Vec<Vec<TimedNote>> = if layers <= 2 {
                vec![melody, bass]
            } else {
                vec![melody, bass, bass_drone]
            };
            let smf_bytes = build_smf(&track_sets);
            std::fs::write(&output, &smf_bytes)
                .with_context(|| format!("write {}", output.display()))?;
            eprintln!("wrote {} ({} bytes)", output.display(), smf_bytes.len());
        }
        Cmd::Voices { file, voices } => {
            let s = midi::load_from_path(&file)
                .with_context(|| format!("load MIDI: {}", file.display()))?;
            let split = beep_common::assign_voices(&s, voices);
            for (i, v) in split.iter().enumerate() {
                let notes_str: Vec<String> = v
                    .notes
                    .iter()
                    .take(8)
                    .map(|n| format!("{}({:.2}-{:.2})", n.key, n.start, n.end))
                    .collect();
                let more = if v.notes.len() > 8 {
                    format!(" +{}...", v.notes.len() - 8)
                } else {
                    "".into()
                };
                println!(
                    "voice {}: {} notes, dur={:.3}s  [{}]{}",
                    i,
                    v.notes.len(),
                    v.duration,
                    notes_str.join(" "),
                    more
                );
            }
        }
    }
    Ok(())
}
