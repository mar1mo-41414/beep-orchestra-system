//! MIDI ノート番号と周波数の変換。

/// A4 = 440Hz, MIDI 69。
pub const A4_MIDI: u8 = 69;
pub const A4_HZ: f64 = 440.0;

/// MIDI ノート番号 → 周波数 (Hz, 小数)。
#[inline]
pub fn midi_to_hz(note: u8) -> f64 {
    A4_HZ * 2f64.powf((note as f64 - A4_MIDI as f64) / 12.0)
}

/// MIDI ノート番号 → 整数 Hz へ丸め。beep コマンドは整数を渡す。
#[inline]
pub fn midi_to_hz_rounded(note: u8) -> u32 {
    midi_to_hz(note).round().max(1.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a4_is_440hz() {
        assert_eq!(midi_to_hz_rounded(69), 440);
    }

    #[test]
    fn c4_is_262hz() {
        // MIDI 60 = C4 ≈ 261.6255653Hz → 262
        assert_eq!(midi_to_hz_rounded(60), 262);
    }

    #[test]
    fn octave_doubles_frequency() {
        let a3 = midi_to_hz(57);
        let a4 = midi_to_hz(69);
        let a5 = midi_to_hz(81);
        assert!((a4 - a3 * 2.0).abs() < 1e-6);
        assert!((a5 - a4 * 2.0).abs() < 1e-6);
    }
}
