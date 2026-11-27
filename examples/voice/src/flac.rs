use std::io::{self, Seek, SeekFrom, Write};

use flacenc::{
    bitsink::ByteSink,
    component::{BitRepr, StreamInfo},
    config::Encoder,
    encode_fixed_size_frame,
    error::{Verified, Verify},
    source::{Context, Fill, FrameBuf},
};

use super::receive::{BYTES_PER_TICK, PcmTick};

const SAMPLE_RATE: usize = 48_000;
const CHANNELS: usize = 2;
const BITS_PER_SAMPLE: usize = 16;
const SAMPLE_BYTES: usize = BITS_PER_SAMPLE / 8;
const FRAMES_PER_TICK: usize = BYTES_PER_TICK / CHANNELS / SAMPLE_BYTES;
const MAX_SAMPLE_FRAMES: u64 = (1_u64 << 36) - 1;
const STREAMINFO_BYTES: usize = 34;
// fLaC マーカーの次に、最後のメタデータブロックとして STREAMINFO を置きます。
const PREFIX: &[u8] = b"fLaC\x80\x00\x00\x22";

pub(super) const HEADER_BYTES: usize = PREFIX.len() + STREAMINFO_BYTES;

/// 1 tick を 1 フレームにし、上限を確かめてから出力へ追加します。
pub(super) struct FlacTrack<W> {
    output: W,
    encoder: Verified<Encoder>,
    input: FrameBuf,
    pcm_bytes: [u8; BYTES_PER_TICK],
    encoded: ByteSink,
    info: StreamInfo,
    committed: Context,
    frame_count: usize,
    frame_range: Option<(usize, usize)>,
    staged: bool,
    failed: bool,
}

impl<W: Write + Seek> FlacTrack<W> {
    pub(super) fn new(mut output: W) -> io::Result<Self> {
        let mut encoder = Encoder::default();
        // 短い音声フレームを複数トラック同時に処理するため、固定予測を使います。
        encoder.subframe_coding.use_lpc = false;
        let encoder = encoder
            .into_verified()
            .map_err(|(_, error)| codec_error(error))?;
        let mut info =
            StreamInfo::new(SAMPLE_RATE, CHANNELS, BITS_PER_SAMPLE).map_err(codec_error)?;
        info.set_block_sizes(FRAMES_PER_TICK, FRAMES_PER_TICK)
            .map_err(codec_error)?;
        info.set_frame_sizes(0, 0).map_err(codec_error)?;
        write_header(&mut output, &info)?;
        Ok(Self {
            output,
            encoder,
            input: FrameBuf::with_size(CHANNELS, FRAMES_PER_TICK).map_err(codec_error)?,
            pcm_bytes: [0; BYTES_PER_TICK],
            encoded: ByteSink::new(),
            info,
            committed: Context::new(BITS_PER_SAMPLE, CHANNELS),
            frame_count: 0,
            frame_range: None,
            staged: false,
            failed: false,
        })
    }

    pub(super) fn encode(&mut self, tick: &PcmTick) -> io::Result<&[u8]> {
        if self.staged || self.failed {
            return Err(io::Error::other(
                "前の FLAC フレームの書き込みが完了していません",
            ));
        }
        if self
            .committed
            .total_samples()
            .checked_add(FRAMES_PER_TICK)
            .and_then(|frames| u64::try_from(frames).ok())
            .is_none_or(|frames| frames > MAX_SAMPLE_FRAMES)
        {
            return Err(io::Error::other("FLAC のサンプル数上限に達しました"));
        }
        for (out, sample) in self.pcm_bytes.chunks_exact_mut(SAMPLE_BYTES).zip(tick) {
            out.copy_from_slice(&sample.to_le_bytes());
        }
        self.input
            .fill_le_bytes(&self.pcm_bytes, SAMPLE_BYTES)
            .map_err(codec_error)?;
        let frame =
            encode_fixed_size_frame(&self.encoder, &self.input, self.frame_count, &self.info)
                .map_err(codec_error)?;
        self.encoded.clear();
        frame.write(&mut self.encoded).map_err(codec_error)?;
        self.staged = true;
        Ok(self.encoded.as_slice())
    }

    pub(super) fn commit(&mut self) -> io::Result<()> {
        if !std::mem::take(&mut self.staged) || self.failed {
            return Err(io::Error::other("書き込む FLAC フレームがありません"));
        }
        let bytes = self.encoded.as_slice();
        self.failed = true;
        self.output.write_all(bytes)?;
        self.committed
            .fill_le_bytes(&self.pcm_bytes, SAMPLE_BYTES)
            .map_err(codec_error)?;
        self.frame_count += 1;
        let size = bytes.len();
        self.frame_range = Some(
            self.frame_range
                .map_or((size, size), |(min, max)| (min.min(size), max.max(size))),
        );
        self.failed = false;
        Ok(())
    }

    pub(super) fn finish(mut self) -> io::Result<W> {
        if self.failed {
            return Err(io::Error::other(
                "FLAC フレームの書き込みに失敗したため確定できません",
            ));
        }
        if let Some((min, max)) = self.frame_range {
            self.info.set_frame_sizes(min, max).map_err(codec_error)?;
        }
        // バイト上限で最後のフレームを見送った場合も、書き込み済みの範囲だけを確定します。
        self.info.set_total_samples(self.committed.total_samples());
        self.info.set_md5_digest(&self.committed.md5_digest());
        self.output.seek(SeekFrom::Start(0))?;
        write_header(&mut self.output, &self.info)?;
        self.output.flush()?;
        Ok(self.output)
    }
}

fn write_header(output: &mut impl Write, info: &StreamInfo) -> io::Result<()> {
    let mut bytes = ByteSink::with_capacity(STREAMINFO_BYTES * 8);
    info.write(&mut bytes).map_err(codec_error)?;
    output.write_all(PREFIX)?;
    output.write_all(bytes.as_slice())
}

fn codec_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("FLAC エンコードエラー: {error}"))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use symphonia::core::{
        audio::SampleBuffer, codecs::DecoderOptions, errors::Error as DecodeError,
        formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions, probe::Hint,
    };

    use super::*;

    #[test]
    fn flac_round_trips_stereo_ticks_and_ignores_uncommitted_frame() {
        let mut left_first = true;
        let mut rng = 1234_u32;
        let noise: PcmTick = std::array::from_fn(|_| {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            left_first = !left_first;
            if left_first { rng as i16 } else { i16::MIN }
        });
        let silence = [0; BYTES_PER_TICK / SAMPLE_BYTES];
        let discard = [300; BYTES_PER_TICK / SAMPLE_BYTES];
        let mut track = FlacTrack::new(Cursor::new(Vec::new())).unwrap();
        let first = track.encode(&noise).unwrap().len();
        track.commit().unwrap();
        let second = track.encode(&silence).unwrap().len();
        track.commit().unwrap();
        track.encode(&discard).unwrap();
        let bytes = track.finish().unwrap().into_inner();
        assert_eq!(bytes.get(..PREFIX.len()), Some(PREFIX));
        assert_eq!(bytes.len(), HEADER_BYTES + first + second);

        let expected: Vec<_> = noise.into_iter().chain(silence).collect();
        assert_eq!(decode(bytes), expected);
    }

    fn decode(bytes: Vec<u8>) -> Vec<i16> {
        let source = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
        let probed = symphonia::default::get_probe()
            .format(
                Hint::new().with_extension("flac"),
                source,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .unwrap();
        let mut format = probed.format;
        let track = format.default_track().unwrap();
        assert_eq!(track.codec_params.sample_rate, Some(48_000));
        assert_eq!(track.codec_params.bits_per_sample, Some(16));
        assert_eq!(track.codec_params.n_frames, Some(1_920));
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions { verify: true })
            .unwrap();
        let mut samples = Vec::new();
        loop {
            match format.next_packet() {
                Ok(packet) => {
                    let decoded = decoder.decode(&packet).unwrap();
                    assert_eq!(decoded.spec().channels.count(), 2);
                    let mut buffer =
                        SampleBuffer::<i16>::new(decoded.capacity() as u64, *decoded.spec());
                    buffer.copy_interleaved_ref(decoded);
                    samples.extend_from_slice(buffer.samples());
                }
                Err(DecodeError::IoError(error))
                    if error.kind() == io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(error) => panic!("FLAC decode: {error}"),
            }
        }
        assert_eq!(decoder.finalize().verify_ok, Some(true));
        samples
    }

    #[test]
    fn failed_output_cannot_be_finalized_after_a_partial_frame() {
        struct Limited(Cursor<Vec<u8>>);
        impl Write for Limited {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.0.position() > HEADER_BYTES as u64 {
                    return Err(io::Error::other("fixture limit"));
                }
                let available = (HEADER_BYTES + 1) as u64 - self.0.position();
                self.0.write(&bytes[..bytes.len().min(available as usize)])
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        impl Seek for Limited {
            fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
                self.0.seek(position)
            }
        }

        let mut track = FlacTrack::new(Limited(Cursor::new(Vec::new()))).unwrap();
        assert!(track.commit().is_err());
        let tick = [0; BYTES_PER_TICK / SAMPLE_BYTES];
        track.encode(&tick).unwrap();
        assert!(track.encode(&tick).is_err());
        assert!(track.commit().is_err());
        assert!(track.finish().is_err());
    }
}
