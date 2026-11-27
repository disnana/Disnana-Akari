use std::f64::consts::TAU;

use disnana_akari::Result;
use songbird::input::Input;

const SAMPLE_RATE: u32 = 48_000;
const TONE_SAMPLES: usize = SAMPLE_RATE as usize * 300 / 1000;
const GAP_SAMPLES: usize = SAMPLE_RATE as usize * 80 / 1000;
const FADE_SAMPLES: usize = SAMPLE_RATE as usize * 20 / 1000;
const PEAK: f64 = i16::MAX as f64 * 0.15;

pub(super) async fn test_tone() -> Result<Input> {
    super::load_audio(Input::from(wav())).await
}

fn wav() -> Vec<u8> {
    const HEADER_BYTES: usize = 44;
    const DATA_BYTES: usize = (TONE_SAMPLES * 2 + GAP_SAMPLES) * size_of::<i16>();

    let mut bytes = Vec::with_capacity(HEADER_BYTES + DATA_BYTES);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + DATA_BYTES as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes()); // PCM fmt チャンクのバイト数
    bytes.extend_from_slice(&1_u16.to_le_bytes()); // PCM 形式
    bytes.extend_from_slice(&1_u16.to_le_bytes()); // モノラル
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes()); // 1 サンプルのバイト数
    bytes.extend_from_slice(&16_u16.to_le_bytes()); // サンプルのビット数
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(DATA_BYTES as u32).to_le_bytes());

    append_tone(&mut bytes, 440.0);
    bytes.resize(bytes.len() + GAP_SAMPLES * size_of::<i16>(), 0);
    append_tone(&mut bytes, 660.0);
    bytes
}

fn append_tone(output: &mut Vec<u8>, frequency: f64) {
    let step = TAU * frequency / f64::from(SAMPLE_RATE);
    for index in 0..TONE_SAMPLES {
        // 各音の出だしと終わりを無音にして、振幅が急に変わる音を抑えます。
        let edge = index.min(TONE_SAMPLES - 1 - index).min(FADE_SAMPLES);
        let gain = edge as f64 / FADE_SAMPLES as f64;
        let sample = ((step * index as f64).sin() * PEAK * gain).round() as i16;
        output.extend_from_slice(&sample.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_contains_two_distinct_limited_tones_with_silent_edges() {
        let bytes = wav();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[4..8], &((bytes.len() - 8) as u32).to_le_bytes());
        assert_eq!(&bytes[22..24], &1_u16.to_le_bytes());
        assert_eq!(&bytes[24..28], &48_000_u32.to_le_bytes());
        assert_eq!(&bytes[34..36], &16_u16.to_le_bytes());
        assert_eq!(&bytes[40..44], &((bytes.len() - 44) as u32).to_le_bytes());

        let samples: Vec<_> = bytes[44..]
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
            .collect();
        let (first, remainder) = samples.split_at(TONE_SAMPLES);
        let (gap, second) = remainder.split_at(GAP_SAMPLES);
        assert_eq!(second.len(), TONE_SAMPLES);
        assert!(gap.iter().all(|sample| *sample == 0));

        for (tone, expected_crossings) in [(first, 44), (second, 66)] {
            assert_eq!(tone.first(), Some(&0));
            assert_eq!(tone.last(), Some(&0));
            let peak = tone
                .iter()
                .map(|sample| sample.unsigned_abs())
                .max()
                .unwrap();
            assert!((4500..=4915).contains(&peak), "{peak}");
            assert!(tone[..100].iter().all(|sample| sample.unsigned_abs() < 520));
            let excerpt = &tone[FADE_SAMPLES..FADE_SAMPLES + SAMPLE_RATE as usize / 10];
            let crossings = excerpt
                .windows(2)
                .filter(|pair| pair[0] <= 0 && pair[1] > 0)
                .count();
            assert!(crossings.abs_diff(expected_crossings) <= 1, "{crossings}");
        }
    }

    #[tokio::test]
    async fn songbird_accepts_the_generated_pcm_wav() -> Result<()> {
        let input = test_tone().await?;
        assert!(input.is_playable());
        let params = input
            .parsed()
            .expect("prepared input")
            .decoder
            .codec_params();
        assert_eq!(params.sample_rate, Some(SAMPLE_RATE));
        assert_eq!(params.channels.map(|channels| channels.count()), Some(1));
        Ok(())
    }
}
