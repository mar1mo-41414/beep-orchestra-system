//! 4 拍分の Cメジャートライアド (C-E-G の 3 声和音) を書き出す。
//! 検証用:
//!   cargo run -p beep-common --example gen_triad_midi -- examples/triad.mid

use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, Track, TrackEvent, TrackEventKind};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "triad.mid".into());
    let tpq: u16 = 480;
    let quarter = 480_u32;

    // 3 声を別トラックで作る (C, E, G を同時発音 → 4 回連続で鳴らす)
    let voices = [60, 64, 67]; // C4 E4 G4

    let mut tracks: Vec<Track> = Vec::new();

    // 先頭トラックに Tempo を置く
    {
        let mut t: Track = Vec::new();
        t.push(TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
        });
        t.push(TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        });
        tracks.push(t);
    }

    for key in voices {
        let mut t: Track = Vec::new();
        // 4 音: C-E-G を 4 回同期
        for _ in 0..4 {
            t.push(TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOn {
                        key: u7::new(key),
                        vel: u7::new(100),
                    },
                },
            });
            t.push(TrackEvent {
                delta: u28::new(quarter),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOff {
                        key: u7::new(key),
                        vel: u7::new(0),
                    },
                },
            });
        }
        t.push(TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
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

    let mut buf = Vec::new();
    smf.write(&mut buf).unwrap();
    std::fs::write(&out, buf).unwrap();
    eprintln!("wrote {}", out);
}
