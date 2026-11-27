use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Cursor},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use disnana_akari::model::id::{Id, marker::UserMarker};
use songbird::{Event, EventContext, EventHandler};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use super::{
    control::MAX_RECORD_USERS,
    receive::{BYTES_PER_TICK, MIX_SOURCES, PcmTick, WAV_HEADER_BYTES, WaveWriter, mix},
    speaker::Speakers,
};

const TRACK_COUNT: usize = MAX_RECORD_USERS + 1;
const QUEUE_CAPACITY: usize = 16;
type Tick = [PcmTick; TRACK_COUNT];

pub(super) enum TrackFile {
    Disk(PathBuf),
    Memory { name: String, bytes: Vec<u8> },
}

enum TrackWriter {
    Disk {
        path: PathBuf,
        writer: WaveWriter<BufWriter<File>>,
    },
    Memory {
        name: String,
        writer: WaveWriter<Cursor<Vec<u8>>>,
    },
}

impl TrackWriter {
    fn new(directory: Option<&Path>, name: String) -> io::Result<Self> {
        match directory {
            Some(directory) => {
                let path = directory.join(name);
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?;
                let writer = WaveWriter::new(BufWriter::new(file))?;
                Ok(Self::Disk { path, writer })
            }
            None => Ok(Self::Memory {
                name,
                writer: WaveWriter::new(Cursor::new(Vec::new()))?,
            }),
        }
    }

    fn write(&mut self, pcm: &PcmTick) -> io::Result<()> {
        match self {
            Self::Disk { writer, .. } => writer.write_tick(pcm),
            Self::Memory { writer, .. } => writer.write_tick(pcm),
        }
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
    writer: JoinHandle<io::Result<(Vec<TrackFile>, u64)>>,
}

impl Recorder {
    pub(super) fn start(
        directory: Option<&Path>,
        include_mix: bool,
        users: Vec<Id<UserMarker>>,
        max_bytes: u32,
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
        let max_ticks = group_budget(sources.len(), max_bytes)?;
        if let Some(directory) = directory {
            fs::create_dir(directory)?;
        }
        let writers: Vec<_> = sources
            .iter()
            .map(|source| {
                let name =
                    source.map_or_else(|| "mix.wav".into(), |user| format!("user-{user}.wav"));
                TrackWriter::new(directory, name)
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
            write_tracks(writers, receiver, max_ticks)
        });
        Ok((
            Self {
                queue,
                finished,
                writer,
            },
            capture,
        ))
    }

    pub(super) async fn stopped(&mut self) {
        let _ = (&mut self.finished).await;
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

fn group_budget(tracks: usize, bytes: u32) -> io::Result<u64> {
    let tracks = tracks as u64;
    let headers = tracks * u64::from(WAV_HEADER_BYTES);
    let tick_bytes = tracks * BYTES_PER_TICK as u64;
    u64::from(bytes)
        .checked_sub(headers)
        .and_then(|remaining| remaining.checked_div(tick_bytes))
        .filter(|ticks| *ticks != 0)
        .ok_or_else(|| io::Error::other("録音サイズでは全トラックの1フレームを保存できません"))
}

fn write_tracks(
    mut writers: Vec<TrackWriter>,
    mut receiver: mpsc::Receiver<Tick>,
    max_ticks: u64,
) -> io::Result<(Vec<TrackFile>, u64)> {
    let mut written = 0;
    let result = (|| {
        while written < max_ticks {
            let Some(tick) = receiver.blocking_recv() else {
                break;
            };
            for (writer, pcm) in writers.iter_mut().zip(&tick) {
                writer.write(pcm)?;
            }
            written += 1;
        }
        Ok::<_, io::Error>(())
    })();
    // I/O エラーのときも、開いたストリームをすべて確定します。
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
        assert_eq!(group_budget(3, one_group).unwrap(), 1);
        assert!(group_budget(3, one_group - 1).is_err());
        assert!(group_budget(0, one_group).is_err());
    }

    #[test]
    fn writer_keeps_tracks_aligned_and_finalizes_in_memory() -> io::Result<()> {
        let tracks = vec![
            TrackWriter::new(None, "mix.wav".into())?,
            TrackWriter::new(None, "user.wav".into())?,
        ];
        let (sender, receiver) = mpsc::channel(2);
        let mut tick = [[0; super::super::receive::BYTES_PER_TICK / 2]; TRACK_COUNT];
        tick[0][0] = 42;
        tick[1][0] = 9;
        sender.try_send(tick).unwrap();
        sender.try_send(tick).unwrap();
        let (files, ticks) = write_tracks(tracks, receiver, 1)?;
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

    #[tokio::test]
    async fn mix_and_selected_tracks_keep_only_their_own_samples() -> io::Result<()> {
        let first = Id::new(10);
        let unknown = Id::new(20);
        let speakers = Speakers::default();
        speakers.observe_for_test(first, 5);
        let (recorder, capture) =
            Recorder::start(None, true, vec![first, unknown], 1024 * 1024, speakers)?;
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
