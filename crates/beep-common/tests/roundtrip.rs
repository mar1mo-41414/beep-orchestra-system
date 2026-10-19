//! MIDI → Score → JSON → Score のラウンドトリップ確認。
//! POST /play_part の JSON body 経路が壊れないためのガード。

use beep_common::{midi, Score};

#[test]
fn score_json_roundtrip() {
    // examples/scale.mid が無いと動かないので、テスト時は SMF を組み立てる
    use midly::num::{u15, u24, u28, u4, u7};
    use midly::{
        Format, Header, MetaMessage, MidiMessage, Smf, Timing, Track, TrackEvent,
        TrackEventKind as K,
    };

    let mut track: Track = Vec::new();
    track.push(TrackEvent {
        delta: u28::new(0),
        kind: K::Meta(MetaMessage::Tempo(u24::new(500_000))),
    });
    for key in [60_u8, 64, 67] {
        track.push(TrackEvent {
            delta: u28::new(0),
            kind: K::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOn {
                    key: u7::new(key),
                    vel: u7::new(90),
                },
            },
        });
    }
    track.push(TrackEvent {
        delta: u28::new(480),
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

    let s = midi::load_from_bytes(&buf, Some("triad".into())).unwrap();
    let json = serde_json::to_string(&s).unwrap();
    let back: Score = serde_json::from_str(&json).unwrap();

    assert_eq!(s.notes.len(), back.notes.len());
    for (a, b) in s.notes.iter().zip(back.notes.iter()) {
        assert_eq!(a.key, b.key);
        assert_eq!(a.channel, b.channel);
        assert!((a.start - b.start).abs() < 1e-12);
        assert!((a.end - b.end).abs() < 1e-12);
    }
    assert!((s.duration - back.duration).abs() < 1e-12);
}
