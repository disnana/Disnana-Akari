use std::{
    fs::OpenOptions,
    io::{self, BufWriter, Seek, SeekFrom, Write},
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use songbird::{Event, EventContext, EventHandler};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

// Songbird の既定の DecodeConfig と VoiceTick の間隔に揃えます。
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const BITS_PER_SAMPLE: u16 = 16;
const SAMPLES_PER_TICK: usize = 1920;
const BYTES_PER_TICK: usize = SAMPLES_PER_TICK * size_of::<i16>();
const RIFF_OVERHEAD: u32 = 36;
const WAV_HEADER_BYTES: u32 = RIFF_OVERHEAD + 8;
const QUEUE_TICKS: usize = 64;
const WRITE_BUFFER_BYTES: usize = 64 * 1024;
const MIX_SOURCES: usize = 128;
type PcmTick = [i16; SAMPLES_PER_TICK];

#[derive(Debug)]
pub(super) struct RecordingStats {
    pub(super) ticks_written: u64,
    pub(super) ticks_dropped: u64,
}

/// 録音先が指定されたときにだけ作る、WAV 書き出しの管理用ハンドルです。
pub(super) struct Recording {
    queue: Arc<RecordingQueue>,
    writer: JoinHandle<io::Result<u64>>,
    writer_stopped: oneshot::Receiver<()>,
}

impl Recording {
    pub(super) fn start(path: &Path) -> io::Result<(Self, VoiceReceiver)> {
        Self::start_with_limit(path, None)
    }

    pub(super) fn start_limited(path: &Path, limit: u32) -> io::Result<(Self, VoiceReceiver)> {
        Self::start_with_limit(path, Some(limit))
    }

    fn start_with_limit(path: &Path, limit: Option<u32>) -> io::Result<(Self, VoiceReceiver)> {
        // 既存ファイルを上書きしません。ファイル名は呼び出し側が明示します。
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let writer = WaveWriter::new(BufWriter::with_capacity(WRITE_BUFFER_BYTES, file))?;
        let (sender, receiver) = mpsc::channel(QUEUE_TICKS);
        let queue = Arc::new(RecordingQueue {
            sender: Mutex::new(Some(sender)),
            dropped: AtomicU64::new(0),
        });
        let sink = VoiceReceiver {
            queue: Arc::clone(&queue),
        };
        let (notify_stop, writer_stopped) = oneshot::channel();
        let writer = tokio::task::spawn_blocking(move || {
            // 解放時に待機側を起こし、書き出しのエラーを finish から返します。
            let _notify_stop = notify_stop;
            write_recording(writer, receiver, limit)
        });
        Ok((
            Self {
                queue,
                writer,
                writer_stopped,
            },
            sink,
        ))
    }

    pub(super) async fn stopped(&mut self) {
        let _ = (&mut self.writer_stopped).await;
    }

    pub(super) async fn finish(mut self) -> io::Result<RecordingStats> {
        self.queue.close();
        let ticks_written = (&mut self.writer).await.map_err(io::Error::other)??;
        Ok(RecordingStats {
            ticks_written,
            ticks_dropped: self.queue.dropped.load(Ordering::Relaxed),
        })
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        // エラーなどで finish を待てない場合にも、バックグラウンドでファイルを閉じます。
        self.queue.close();
    }
}

struct RecordingQueue {
    sender: Mutex<Option<mpsc::Sender<PcmTick>>>,
    dropped: AtomicU64,
}

impl RecordingQueue {
    fn close(&self) {
        // 所有する送信口はここだけです。閉じると書き出し側は待ち行列を最後まで排出します。
        self.sender
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }
}

pub(super) struct VoiceReceiver {
    queue: Arc<RecordingQueue>,
}

impl VoiceReceiver {
    fn capture<'a>(&self, sources: impl IntoIterator<Item = &'a [i16]>) {
        let Ok(sender) = self.queue.sender.try_lock() else {
            self.queue.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let Some(sender) = sender.as_ref() else {
            return;
        };
        // 空き枠を予約してからミックスします。イベント内でディスクや空き枠を待ちません。
        match sender.try_reserve() {
            Ok(permit) => permit.send(mix(sources)),
            Err(mpsc::error::TrySendError::Full(())) => {
                self.queue.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Closed(())) => {}
        }
    }
}

#[async_trait]
impl EventHandler for VoiceReceiver {
    async fn act(&self, context: &EventContext<'_>) -> Option<Event> {
        if let EventContext::VoiceTick(tick) = context {
            self.capture(
                tick.speaking
                    .values()
                    .take(MIX_SOURCES)
                    .filter_map(|voice| voice.decoded_voice.as_deref()),
            );
        }
        None
    }
}

fn mix<'a>(sources: impl IntoIterator<Item = &'a [i16]>) -> PcmTick {
    let mut sums = [0_i32; SAMPLES_PER_TICK];
    for source in sources.into_iter().take(MIX_SOURCES) {
        for (sum, sample) in sums.iter_mut().zip(source) {
            *sum += i32::from(*sample);
        }
    }
    sums.map(|sum| sum.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16)
}

fn write_recording<W: Write + Seek>(
    mut writer: WaveWriter<W>,
    mut receiver: mpsc::Receiver<PcmTick>,
    limit: Option<u32>,
) -> io::Result<u64> {
    let mut ticks_written = 0;
    let result = loop {
        let Some(tick) = receiver.blocking_recv() else {
            break Ok(());
        };
        if limit.is_some_and(|limit| {
            writer.data_bytes + WAV_HEADER_BYTES + BYTES_PER_TICK as u32 > limit
        }) {
            break Ok(());
        }
        if let Err(error) = writer.write_tick(&tick) {
            break Err(error);
        }
        ticks_written += 1;
    };
    // RIFF の容量上限で止まった場合にも、すでに書けた部分の長さを確定させます。
    let finalized = writer.finish();
    result.and(finalized)?;
    Ok(ticks_written)
}

struct WaveWriter<W> {
    output: W,
    data_bytes: u32,
}

impl<W: Write + Seek> WaveWriter<W> {
    fn new(mut output: W) -> io::Result<Self> {
        write_header(&mut output, 0)?;
        Ok(Self {
            output,
            data_bytes: 0,
        })
    }

    fn write_tick(&mut self, pcm: &PcmTick) -> io::Result<()> {
        let size = self
            .data_bytes
            .checked_add(BYTES_PER_TICK as u32)
            .filter(|size| *size <= u32::MAX - RIFF_OVERHEAD)
            .ok_or_else(|| {
                io::Error::other("WAV の RIFF チャンクが 32 ビットの上限に達しました")
            })?;
        let mut bytes = [0_u8; BYTES_PER_TICK];
        for (out, sample) in bytes.chunks_exact_mut(size_of::<i16>()).zip(pcm) {
            out.copy_from_slice(&sample.to_le_bytes());
        }
        self.output.write_all(&bytes)?;
        self.data_bytes = size;
        Ok(())
    }

    fn finish(mut self) -> io::Result<()> {
        self.output.seek(SeekFrom::Start(0))?;
        write_header(&mut self.output, self.data_bytes)?;
        self.output.flush()
    }
}

fn write_header(output: &mut impl Write, data_bytes: u32) -> io::Result<()> {
    let block_align = CHANNELS * BITS_PER_SAMPLE / 8;
    for part in [
        b"RIFF".as_slice(),
        &(RIFF_OVERHEAD + data_bytes).to_le_bytes(),
        b"WAVEfmt ",
        &16_u32.to_le_bytes(), // PCM の fmt チャンクの長さ
        &1_u16.to_le_bytes(),  // WAV_FORMAT_PCM
        &CHANNELS.to_le_bytes(),
        &SAMPLE_RATE.to_le_bytes(),
        &(SAMPLE_RATE * u32::from(block_align)).to_le_bytes(),
        &block_align.to_le_bytes(),
        &BITS_PER_SAMPLE.to_le_bytes(),
        b"data",
        &data_bytes.to_le_bytes(),
    ] {
        output.write_all(part)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn mixes_stereo_with_saturation_and_silence_padding() {
        let one = [30_000, -30_000, 30_000];
        let two = [20_000, -20_000, 30_000];
        let three = [0, 0, -30_000];
        let mixed = mix([one.as_slice(), two.as_slice(), three.as_slice()]);
        assert_eq!(&mixed[..4], &[i16::MAX, i16::MIN, 30_000, 0]);
        assert_eq!(mix([]), [0; SAMPLES_PER_TICK]);

        let sample = [1_i16];
        let capped = mix(std::iter::repeat_n(sample.as_slice(), MIX_SOURCES + 1));
        assert_eq!(capped[0], MIX_SOURCES as i16);
    }

    #[test]
    fn wav_writer_finalizes_riff_and_data_lengths_in_little_endian() -> io::Result<()> {
        let mut bytes = Cursor::new(Vec::new());
        let mut wav = WaveWriter::new(&mut bytes)?;
        let test = [0x1234, -2];
        wav.write_tick(&mix([test.as_slice()]))?;
        wav.write_tick(&mix([]))?;
        wav.finish()?;
        let bytes = bytes.into_inner();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(&bytes[20..24], &[1, 0, 2, 0]);
        assert_eq!(&bytes[24..28], &48_000_u32.to_le_bytes());
        assert_eq!(&bytes[28..32], &192_000_u32.to_le_bytes());
        assert_eq!(&bytes[32..36], &[4, 0, 16, 0]);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(&bytes[44..48], &[0x34, 0x12, 0xFE, 0xFF]);
        assert_eq!(&bytes[4..8], &(bytes.len() as u32 - 8).to_le_bytes());
        assert_eq!(&bytes[40..44], &(2 * BYTES_PER_TICK as u32).to_le_bytes());
        assert!(bytes[48..].iter().all(|byte| *byte == 0));
        Ok(())
    }

    #[test]
    fn rejects_riff_overflow_without_writing_partial_tick() -> io::Result<()> {
        let mut wav = WaveWriter::new(Cursor::new(Vec::new()))?;
        wav.data_bytes = u32::MAX - RIFF_OVERHEAD;
        assert!(wav.write_tick(&mix([])).is_err());
        assert_eq!(wav.data_bytes, u32::MAX - RIFF_OVERHEAD);
        assert_eq!(wav.output.get_ref().len(), 44);
        Ok(())
    }

    #[test]
    fn recording_limit_finalizes_only_complete_ticks() -> io::Result<()> {
        let (sender, receiver) = mpsc::channel(2);
        sender.try_send(mix([])).unwrap();
        sender.try_send(mix([])).unwrap();
        drop(sender);

        let mut bytes = Cursor::new(Vec::new());
        let writer = WaveWriter::new(&mut bytes)?;
        let limit = WAV_HEADER_BYTES + BYTES_PER_TICK as u32 + 1;
        assert_eq!(write_recording(writer, receiver, Some(limit))?, 1);
        assert_eq!(
            bytes.get_ref().len(),
            WAV_HEADER_BYTES as usize + BYTES_PER_TICK
        );
        Ok(())
    }

    #[test]
    fn full_queue_drops_next_tick_and_close_drains_without_waiting_for_callback_drop() {
        let (sender, mut receiver) = mpsc::channel(1);
        let queue = Arc::new(RecordingQueue {
            sender: Mutex::new(Some(sender)),
            dropped: AtomicU64::new(0),
        });
        let sink = VoiceReceiver {
            queue: Arc::clone(&queue),
        };
        sink.capture([[7_i16].as_slice()]);
        sink.capture([[8_i16].as_slice()]);
        assert_eq!(queue.dropped.load(Ordering::Relaxed), 1);
        queue.close();
        sink.capture([[9_i16].as_slice()]);
        assert_eq!(receiver.blocking_recv().unwrap()[0], 7);
        assert!(receiver.blocking_recv().is_none());
    }
}
