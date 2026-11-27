use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Cursor, Seek, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use disnana_akari::model::id::{Id, marker::UserMarker};
use songbird::{Event, EventContext, EventHandler};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{Instant, sleep_until},
};

use super::{
    control::MAX_RECORD_USERS,
    flac::{FlacTrack, HEADER_BYTES as FLAC_HEADER_BYTES},
    receive::{BYTES_PER_TICK, MIX_SOURCES, PcmTick, WAV_HEADER_BYTES, WaveWriter, mix},
    speaker::Speakers,
};

const TRACK_COUNT: usize = MAX_RECORD_USERS + 1;
const QUEUE_CAPACITY: usize = 16;
const MAX_DURATION: Duration = Duration::from_secs(5 * 60);
const MAX_TICKS: u64 = MAX_DURATION.as_secs() * 50;
type Tick = [PcmTick; TRACK_COUNT];

pub(super) enum TrackFile {
    Disk(PathBuf),
    Memory { name: String, bytes: Vec<u8> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Format {
    Wav,
    Flac,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
        }
    }

    fn header_bytes(self) -> u64 {
        match self {
            Self::Wav => u64::from(WAV_HEADER_BYTES),
            Self::Flac => FLAC_HEADER_BYTES as u64,
        }
    }
}

enum Encoder<W: Write + Seek> {
    Wav(WaveWriter<W>),
    Flac(Box<FlacTrack<W>>),
}

impl<W: Write + Seek> Encoder<W> {
    fn new(output: W, format: Format) -> io::Result<Self> {
        match format {
            Format::Wav => Ok(Self::Wav(WaveWriter::new(output)?)),
            Format::Flac => Ok(Self::Flac(Box::new(FlacTrack::new(output)?))),
        }
    }

    fn stage(&mut self, pcm: &PcmTick) -> io::Result<u64> {
        match self {
            Self::Wav(_) => Ok(BYTES_PER_TICK as u64),
            Self::Flac(writer) => Ok(writer.encode(pcm)?.len() as u64),
        }
    }

    fn commit(&mut self, pcm: &PcmTick) -> io::Result<()> {
        match self {
            Self::Wav(writer) => writer.write_tick(pcm),
            Self::Flac(writer) => writer.commit(),
        }
    }

    fn finish(self) -> io::Result<W> {
        match self {
            Self::Wav(writer) => writer.finish(),
            Self::Flac(writer) => writer.finish(),
        }
    }
}

enum TrackWriter {
    Disk {
        path: PathBuf,
        writer: Encoder<BufWriter<File>>,
    },
    Memory {
        name: String,
        writer: Encoder<Cursor<Vec<u8>>>,
    },
}

impl TrackWriter {
    fn new(directory: Option<&Path>, name: String, format: Format) -> io::Result<Self> {
        match directory {
            Some(directory) => {
                let path = directory.join(name);
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?;
                let writer = Encoder::new(BufWriter::new(file), format)?;
                Ok(Self::Disk { path, writer })
            }
            None => Ok(Self::Memory {
                name,
                writer: Encoder::new(Cursor::new(Vec::new()), format)?,
            }),
        }
    }

    fn stage(&mut self, pcm: &PcmTick) -> io::Result<u64> {
        match self {
            Self::Disk { writer, .. } => writer.stage(pcm),
            Self::Memory { writer, .. } => writer.stage(pcm),
        }
    }

    fn commit(&mut self, pcm: &PcmTick) -> io::Result<()> {
        match self {
            Self::Disk { writer, .. } => writer.commit(pcm),
            Self::Memory { writer, .. } => writer.commit(pcm),
        }
    }

    fn discard(self) -> io::Result<()> {
        if let Self::Disk { path, writer } = self {
            drop(writer);
            fs::remove_file(path)?;
        }
        Ok(())
    }

    fn finish(self) -> io::Result<TrackFile> {
        match self {
            Self::Disk { path, writer } => {
                writer.finish()?;
                Ok(TrackFile::Disk(path))
            }
            Self::Memory { name, writer } => Ok(TrackFile::Memory {
                name,
                bytes: writer.finish()?.into_inner(),
            }),
        }
    }
}

pub(super) struct Finished {
    pub(super) tracks: Vec<TrackFile>,
    pub(super) ticks_written: u64,
    pub(super) ticks_dropped: u64,
}

struct Queue {
    sender: Mutex<Option<mpsc::Sender<Tick>>>,
    dropped: AtomicU64,
}

pub(super) struct Recorder {
    queue: Arc<Queue>,
    finished: oneshot::Receiver<()>,
    deadline: Instant,
    writer: JoinHandle<io::Result<(Vec<TrackFile>, u64)>>,
}

impl Recorder {
    pub(super) fn start(
        directory: Option<&Path>,
        include_mix: bool,
        users: Vec<Id<UserMarker>>,
        max_bytes: u32,
        format: Format,
        speakers: Speakers,
    ) -> io::Result<(Self, Capture)> {
        if users.len() > MAX_RECORD_USERS
            || (!include_mix && users.is_empty())
            || users
                .iter()
                .enumerate()
                .any(|(index, user)| users.iter().take(index).any(|previous| previous == user))
        {
            return Err(io::Error::other("無効な録音トラックの構成です"));
        }
        let sources: Vec<_> = include_mix
            .then_some(None)
            .into_iter()
            .chain(users.iter().copied().map(Some))
            .collect();
        let byte_budget = payload_budget(sources.len(), max_bytes, format)?;
        let extension = format.extension();
        if let Some(directory) = directory {
            fs::create_dir(directory)?;
        }
        let writers: Vec<_> = sources
            .iter()
            .map(|source| {
                let name = source.map_or_else(
                    || format!("mix.{extension}"),
                    |user| format!("user-{user}.{extension}"),
                );
                TrackWriter::new(directory, name, format)
            })
            .collect::<io::Result<_>>()?;
        let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
        let queue = Arc::new(Queue {
            sender: Mutex::new(Some(sender)),
            dropped: AtomicU64::new(0),
        });
        let capture = Capture {
            queue: Arc::clone(&queue),
            speakers,
            sources,
        };
        let (notify, finished) = oneshot::channel();
        let writer = tokio::task::spawn_blocking(move || {
            let _notify = notify;
            write_tracks(writers, receiver, byte_budget)
        });
        Ok((
            Self {
                queue,
                finished,
                deadline: Instant::now() + MAX_DURATION,
                writer,
            },
            capture,
        ))
    }

    pub(super) async fn stopped(&mut self) {
        tokio::select! {
            _ = &mut self.finished => {}
            _ = sleep_until(self.deadline) => {}
        }
    }

    pub(super) async fn finish(mut self) -> io::Result<Finished> {
        self.close();
        let (tracks, ticks_written) = (&mut self.writer).await.map_err(io::Error::other)??;
        Ok(Finished {
            tracks,
            ticks_written,
            ticks_dropped: self.queue.dropped.load(Ordering::Relaxed),
        })
    }

    fn close(&self) {
        self.queue
            .sender
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.close();
    }
}

fn payload_budget(tracks: usize, bytes: u32, format: Format) -> io::Result<u64> {
    let headers = tracks as u64 * format.header_bytes();
    u64::from(bytes)
        .checked_sub(headers)
        .filter(|remaining| *remaining > 0 && tracks != 0)
        .ok_or_else(|| io::Error::other("録音サイズでは全トラックのヘッダーを保存できません"))
}

fn write_tracks(
    mut writers: Vec<TrackWriter>,
    mut receiver: mpsc::Receiver<Tick>,
    mut remaining: u64,
) -> io::Result<(Vec<TrackFile>, u64)> {
    let mut written = 0;
    let result = (|| {
        while written < MAX_TICKS {
            let Some(tick) = receiver.blocking_recv() else {
                break;
            };
            // 全トラックの符号化サイズを確認してから、同じフレームをまとめて保存します。
            let bytes = writers
                .iter_mut()
                .zip(&tick)
                .try_fold(0, |total, (writer, pcm)| {
                    writer.stage(pcm).map(|size| total + size)
                })?;
            let Some(next_budget) = remaining.checked_sub(bytes) else {
                break;
            };
            for (writer, pcm) in writers.iter_mut().zip(&tick) {
                writer.commit(pcm)?;
            }
            remaining = next_budget;
            written += 1;
            if remaining == 0 {
                break;
            }
        }
        Ok::<_, io::Error>(())
    })();
    if written == 0 && result.is_ok() {
        // FLAC には1フレーム以上必要です。音声がないときは添付を作りません。
        let discarded: Vec<_> = writers.into_iter().map(TrackWriter::discard).collect();
        discarded.into_iter().collect::<io::Result<()>>()?;
        return Ok((Vec::new(), 0));
    }
    let results: Vec<_> = writers.into_iter().map(TrackWriter::finish).collect();
    result?;
    Ok((results.into_iter().collect::<io::Result<_>>()?, written))
}

pub(super) struct Capture {
    queue: Arc<Queue>,
    speakers: Speakers,
    sources: Vec<Option<Id<UserMarker>>>,
}

impl Capture {
    fn capture<'a>(
        &self,
        all: impl IntoIterator<Item = &'a [i16]>,
        selected: impl Fn(u32) -> Option<&'a [i16]>,
    ) -> Option<Event> {
        let Ok(sender) = self.queue.sender.try_lock() else {
            self.queue.dropped.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let Some(sender) = sender.as_ref() else {
            return Some(Event::Cancel);
        };
        let permit = match sender.try_reserve() {
            Ok(permit) => permit,
            Err(mpsc::error::TrySendError::Full(())) => {
                self.queue.dropped.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            Err(mpsc::error::TrySendError::Closed(())) => return Some(Event::Cancel),
        };
        let mut all = Some(all);
        let mut tick = [[0; super::receive::BYTES_PER_TICK / 2]; TRACK_COUNT];
        for (out, source) in tick.iter_mut().zip(&self.sources) {
            if let Some(user) = source {
                if let Some(pcm) = self.speakers.ssrc(*user).and_then(&selected) {
                    for (sample, input) in out.iter_mut().zip(pcm) {
                        *sample = *input;
                    }
                }
            } else if let Some(input) = all.take() {
                *out = mix(input);
            }
        }
        permit.send(tick);
        None
    }
}

#[async_trait]
impl EventHandler for Capture {
    async fn act(&self, context: &EventContext<'_>) -> Option<Event> {
        let EventContext::VoiceTick(tick) = context else {
            return None;
        };
        self.capture(
            tick.speaking
                .values()
                .take(MIX_SOURCES)
                .filter_map(|voice| voice.decoded_voice.as_deref()),
            |ssrc| tick.speaking.get(&ssrc)?.decoded_voice.as_deref(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_respect_the_combined_byte_budget() {
        let one_group = 3 * (WAV_HEADER_BYTES + BYTES_PER_TICK as u32);
        assert_eq!(
            payload_budget(3, one_group, Format::Wav).unwrap(),
            3 * BYTES_PER_TICK as u64
        );
        assert!(payload_budget(3, 3 * WAV_HEADER_BYTES, Format::Wav).is_err());
        assert!(payload_budget(0, one_group, Format::Wav).is_err());
    }

    #[test]
    fn writer_keeps_tracks_aligned_and_finalizes_in_memory() -> io::Result<()> {
        let tracks = vec![
            TrackWriter::new(None, "mix.wav".into(), Format::Wav)?,
            TrackWriter::new(None, "user.wav".into(), Format::Wav)?,
        ];
        let (sender, receiver) = mpsc::channel(2);
        let mut tick = [[0; super::super::receive::BYTES_PER_TICK / 2]; TRACK_COUNT];
        tick[0][0] = 42;
        tick[1][0] = 9;
        sender.try_send(tick).unwrap();
        sender.try_send(tick).unwrap();
        let (files, ticks) = write_tracks(tracks, receiver, 2 * BYTES_PER_TICK as u64)?;
        let [
            TrackFile::Memory { bytes: left, .. },
            TrackFile::Memory { bytes: right, .. },
        ] = &files[..]
        else {
            return Err(io::Error::other("2 本のメモリ上のトラックが必要です"));
        };
        assert_eq!(ticks, 1);
        assert_eq!(left.len(), WAV_HEADER_BYTES as usize + BYTES_PER_TICK);
        assert_eq!(right.len(), left.len());
        assert_eq!(&left[44..46], &[42, 0]);
        assert_eq!(&right[44..46], &[9, 0]);
        Ok(())
    }

    #[test]
    fn compressed_frames_fit_or_stop_together() -> io::Result<()> {
        let mut pcm = [[0; BYTES_PER_TICK / 2]; TRACK_COUNT];
        pcm[1][0] = 123;
        let names = ["mix.flac", "user-9.flac"];
        let expected: u64 = names
            .into_iter()
            .zip(&pcm)
            .map(|(name, tick)| TrackWriter::new(None, name.into(), Format::Flac)?.stage(tick))
            .collect::<io::Result<Vec<_>>>()?
            .into_iter()
            .sum();

        for (budget, count) in [(expected, 1), (expected - 1, 0)] {
            let writers = names
                .map(|name| TrackWriter::new(None, name.into(), Format::Flac))
                .into_iter()
                .collect::<io::Result<Vec<_>>>()?;
            let (sender, receiver) = mpsc::channel(2);
            sender.try_send(pcm).unwrap();
            sender.try_send(pcm).unwrap();
            drop(sender);
            let (tracks, written) = write_tracks(writers, receiver, budget)?;
            let mut size = 0;
            for track in tracks {
                let TrackFile::Memory { bytes, .. } = track else {
                    return Err(io::Error::other("メモリ上の FLAC が必要です"));
                };
                assert!(bytes.starts_with(b"fLaC"));
                size += bytes.len() as u64;
            }
            assert_eq!(written, count);
            assert_eq!(size, count * (2 * Format::Flac.header_bytes() + expected));
        }
        Ok(())
    }

    #[tokio::test]
    async fn mix_and_selected_tracks_keep_only_their_own_samples() -> io::Result<()> {
        let first = Id::new(10);
        let unknown = Id::new(20);
        let speakers = Speakers::default();
        speakers.observe_for_test(first, 5);
        let (recorder, capture) = Recorder::start(
            None,
            true,
            vec![first, unknown],
            1024 * 1024,
            Format::Wav,
            speakers,
        )?;
        let alice = [11, 12];
        let bob = [20, 21];
        capture.capture([alice.as_slice(), bob.as_slice()], |ssrc| {
            (ssrc == 5).then_some(alice.as_slice())
        });
        let finished = recorder.finish().await?;
        assert_eq!(finished.ticks_written, 1);
        assert_eq!(finished.ticks_dropped, 0);
        let expected = [[31, 0, 33, 0], [11, 0, 12, 0], [0; 4]];
        for (track, samples) in finished.tracks.into_iter().zip(expected) {
            let TrackFile::Memory { bytes, .. } = track else {
                return Err(io::Error::other("メモリ上のファイルが必要です"));
            };
            assert_eq!(bytes.get(44..48), Some(samples.as_slice()));
        }
        Ok(())
    }
}
