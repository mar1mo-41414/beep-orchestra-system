//! C4→C5 の C メジャースケール SMF を作って `examples/scale.mid` として書き出す。
//! 動作確認用のダミーデータ生成:
//!   cargo run -p beep-common --example gen_scale_midi -- examples/scale.mid

use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, Track, TrackEvent, TrackEventKind};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "scale.mid".into());

    let tpq: u16 = 480;
    let quarter = 480_u32; // 1 quarter = tpq ticks

    let mut track: Track = Vec::new();
    track.push(TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))), // 120 BPM
    });

    let scale = [60, 62, 64, 65, 67, 69, 71, 72]; // C major
    for (i, key) in scale.iter().enumerate() {
        let delta_on = if i == 0 { 0 } else { 0 };
        track.push(TrackEvent {
            delta: u28::new(delta_on),
            kind: TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOn {
                    key: u7::new(*key),
                    vel: u7::new(100),
                },
            },
        });
        track.push(TrackEvent {
            delta: u28::new(quarter),
            kind: TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOff {
                    key: u7::new(*key),
                    vel: u7::new(0),
                },
            },
        });
    }
    track.push(TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });

    let smf = Smf {
        header: Header {
            format: Format::SingleTrack,
            timing: Timing::Metrical(u15::new(tpq)),
        },
        tracks: vec![track],
    };

    let mut buf = Vec::new();
    smf.write(&mut buf).unwrap();
    std::fs::write(&out, buf).unwrap();
    eprintln!("wrote {}", out);
}
