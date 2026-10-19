//! Standard MIDI File (SMF) 解析。
//!
//! `midly` で読み込んだトラックをフラットな Note 列にまとめ、
//! Tempo Meta イベントに追従した実時刻 (秒) を割り当てる。

use anyhow::{Context, Result};
use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use std::path::Path;

use crate::score::{Note, Score};

/// 途中で発音中の音を追跡するための一時レコード。
#[derive(Debug)]
struct Pending {
    key: u8,
    velocity: u8,
    track: u16,
    channel: u8,
    start: f64,
}

/// SMF ファイルを読み込んでスコアに変換する。
pub fn load_from_path<P: AsRef<Path>>(path: P) -> Result<Score> {
    let path = path.as_ref();
    let bytes =
        std::fs::read(path).with_context(|| format!("read MIDI file: {}", path.display()))?;
    let title = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned());
    load_from_bytes(&bytes, title)
}

/// SMF バイト列を読み込んでスコアに変換する。
pub fn load_from_bytes(bytes: &[u8], title: Option<String>) -> Result<Score> {
    let smf = Smf::parse(bytes).context("parse SMF")?;
    let ticks_per_qn = match smf.header.timing {
        Timing::Metrical(tpq) => u32::from(tpq.as_int()),
        // SMPTE 形式は現状未対応。等分ティックとして扱う。
        Timing::Timecode(fps, subframe) => {
            (fps.as_f32() as u32) * (u32::from(subframe)).max(1)
        }
    };
    if ticks_per_qn == 0 {
        anyhow::bail!("ticks-per-quarter is zero; unsupported timing");
    }

    // 全トラックから Tempo イベントを絶対 tick で拾い、統一タイムラインを作る。
    // SMF Format 1 では通常 Track 0 だけに Tempo が並ぶが、
    // どのトラックに書かれていても正しく反映されるようマージする。
    let tempo_map = build_tempo_map(&smf.tracks);

    let mut notes = Vec::new();

    for (track_idx, track) in smf.tracks.iter().enumerate() {
        let mut abs_tick: u64 = 0;
        let mut pending: Vec<Pending> = Vec::new();

        for event in track {
            let delta = u32::from(event.delta.as_int()) as u64;
            abs_tick += delta;
            let elapsed_sec = tick_to_sec(abs_tick, &tempo_map, ticks_per_qn);

            match event.kind {
                TrackEventKind::Midi { channel, message } => match message {
                    MidiMessage::NoteOn { key, vel } => {
                        let key = key.as_int();
                        let vel = vel.as_int();
                        if vel == 0 {
                            // 実質 Note Off
                            close_pending(
                                &mut pending,
                                &mut notes,
                                key,
                                channel.as_int(),
                                track_idx as u16,
                                elapsed_sec,
                            );
                        } else {
                            pending.push(Pending {
                                key,
                                velocity: vel,
                                track: track_idx as u16,
                                channel: channel.as_int(),
                                start: elapsed_sec,
                            });
                        }
                    }
                    MidiMessage::NoteOff { key, vel: _ } => {
                        close_pending(
                            &mut pending,
                            &mut notes,
                            key.as_int(),
                            channel.as_int(),
                            track_idx as u16,
                            elapsed_sec,
                        );
                    }
                    _ => {}
                },
                _ => {}
            }

        }

        // トラック末尾で閉じ切れなかった音は末尾時刻で強制終了させる
        let end_sec = tick_to_sec(abs_tick, &tempo_map, ticks_per_qn);
        for p in pending.drain(..) {
            notes.push(Note {
                key: p.key,
                velocity: p.velocity,
                track: p.track,
                channel: p.channel,
                start: p.start,
                end: end_sec.max(p.start),
            });
        }
    }

    Ok(Score::new(notes, title))
}

/// 全トラックの Tempo イベントを絶対 tick 位置で集めて sort する。
/// 冒頭に Tempo が無いスコアは SMF 既定値 (500_000 μs/qn = 120 BPM) から始まる。
fn build_tempo_map(tracks: &[Vec<midly::TrackEvent<'_>>]) -> Vec<(u64, u32)> {
    let mut map: Vec<(u64, u32)> = Vec::new();
    for track in tracks {
        let mut abs_tick: u64 = 0;
        for event in track {
            abs_tick += u32::from(event.delta.as_int()) as u64;
            if let TrackEventKind::Meta(MetaMessage::Tempo(t)) = event.kind {
                map.push((abs_tick, t.as_int()));
            }
        }
    }
    map.sort_by_key(|&(t, _)| t);
    // 冒頭 (tick=0) に Tempo が無ければ SMF 既定値を挿入
    if map.first().map(|&(t, _)| t).unwrap_or(1) != 0 {
        map.insert(0, (0, 500_000));
    }
    map
}

/// 与えられた絶対 tick が、テンポマップに従って何秒経過にあたるかを返す。
fn tick_to_sec(tick: u64, tempo_map: &[(u64, u32)], tpq: u32) -> f64 {
    let mut sec = 0.0_f64;
    let mut prev_tick = 0_u64;
    let mut cur_us_per_qn = tempo_map[0].1;
    for &(t, us) in tempo_map.iter().skip(1) {
        if t >= tick {
            break;
        }
        sec += (t - prev_tick) as f64 * cur_us_per_qn as f64 / 1_000_000.0 / tpq as f64;
        prev_tick = t;
        cur_us_per_qn = us;
    }
    sec += (tick - prev_tick) as f64 * cur_us_per_qn as f64 / 1_000_000.0 / tpq as f64;
    sec
}

fn close_pending(
    pending: &mut Vec<Pending>,
    out: &mut Vec<Note>,
    key: u8,
    channel: u8,
    track: u16,
    now: f64,
) {
    // 同じ key / channel / track に対応する最古の Note On を閉じる (FIFO)
    if let Some(idx) = pending
        .iter()
        .position(|p| p.key == key && p.channel == channel && p.track == track)
    {
        let p = pending.remove(idx);
        out.push(Note {
            key: p.key,
            velocity: p.velocity,
            track: p.track,
            channel: p.channel,
            start: p.start,
            end: now.max(p.start),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midly::{Format, Header, Track, TrackEvent};

    /// 最低限のワンノート SMF (C4 を 1 拍鳴らす) を作って解析できるか確認。
    #[test]
    fn parse_single_note_smf() {
        use midly::num::{u15, u24, u28, u4, u7};
        use midly::TrackEventKind as K;

        let mut track: Track = Vec::new();
        // Tempo: 500,000 µs / quarter = 120 BPM (default だが明示)
        track.push(TrackEvent {
            delta: u28::new(0),
            kind: K::Meta(MetaMessage::Tempo(u24::new(500_000))),
        });
        // Note On C4
        track.push(TrackEvent {
            delta: u28::new(0),
            kind: K::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOn {
                    key: u7::new(60),
                    vel: u7::new(100),
                },
            },
        });
        // Note Off C4 after 480 ticks (=1 quarter with 480 tpq)
        track.push(TrackEvent {
            delta: u28::new(480),
            kind: K::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOff {
                    key: u7::new(60),
                    vel: u7::new(0),
                },
            },
        });
        // End of track
        track.push(TrackEvent {
            delta: u28::new(0),
            kind: K::Meta(MetaMessage::EndOfTrack),
        });

        let smf = Smf {
            header: Header {
                format: Format::SingleTrack,
                timing: Timing::Metrical(u15::new(480)),
            },
            tracks: vec![track],
        };
        let mut buf = Vec::new();
        smf.write(&mut buf).unwrap();

        let score = load_from_bytes(&buf, Some("t".into())).unwrap();
        assert_eq!(score.notes.len(), 1);
        let n = &score.notes[0];
        assert_eq!(n.key, 60);
        assert!((n.start - 0.0).abs() < 1e-9);
        // 480 ticks / 480 tpq * 0.5s/qn = 0.5s
        assert!((n.end - 0.5).abs() < 1e-9);
    }

    /// Format 1 で Track 0 に Tempo が置かれている場合、
    /// Track 1 のノート時刻もその Tempo で解釈されるべき。
    #[test]
    fn tempo_from_track0_applies_to_track1() {
        use midly::num::{u15, u24, u28, u4, u7};
        use midly::TrackEventKind as K;

        // Track 0: Tempo(1_000_000 µs/qn = 60 BPM)
        let mut t0: Track = Vec::new();
        t0.push(TrackEvent {
            delta: u28::new(0),
            kind: K::Meta(MetaMessage::Tempo(u24::new(1_000_000))),
        });
        t0.push(TrackEvent {
            delta: u28::new(0),
            kind: K::Meta(MetaMessage::EndOfTrack),
        });
        // Track 1: 1 quarter note (480 ticks)
        let mut t1: Track = Vec::new();
        t1.push(TrackEvent {
            delta: u28::new(0),
            kind: K::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOn {
                    key: u7::new(60),
                    vel: u7::new(100),
                },
            },
        });
        t1.push(TrackEvent {
            delta: u28::new(480),
            kind: K::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOff {
                    key: u7::new(60),
                    vel: u7::new(0),
                },
            },
        });
        t1.push(TrackEvent {
            delta: u28::new(0),
            kind: K::Meta(MetaMessage::EndOfTrack),
        });

        let smf = Smf {
            header: Header {
                format: Format::Parallel,
                timing: Timing::Metrical(u15::new(480)),
            },
            tracks: vec![t0, t1],
        };
        let mut buf = Vec::new();
        smf.write(&mut buf).unwrap();

        let score = load_from_bytes(&buf, Some("t".into())).unwrap();
        // Tempo = 60 BPM → 1 quarter = 1.0 秒。以前は Track 1 が独立して 120 BPM を
        // 使い 0.5 秒に誤解釈していた
        assert_eq!(score.notes.len(), 1);
        let n = &score.notes[0];
        assert!((n.end - 1.0).abs() < 1e-9, "expected 1.0s got {}", n.end);
    }
}
