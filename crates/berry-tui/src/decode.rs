//! berry's own decoder: Symphonia (plus libopus, for Opus) turning a file into samples for rodio
//! to play. Doing this ourselves means a file the decoder can't handle is an ordinary error rather
//! than a crash, and berry can play Opus, which rodio's built-in decoder can't.
//!
//! For the rest, ffmpeg (if it's installed) steps in: surround AAC and HE-AAC, Dolby and DTS (the
//! sound in most films), and formats like WMA and APE. Only for files berry's own decoder turns
//! down, so everything that already played keeps playing exactly as before.

use rodio::source::SeekError;
use rodio::Source;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::OnceLock;
use std::time::Duration;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, Timestamp};

/// Every codec Symphonia ships with, plus Opus through libopus.
fn codecs() -> &'static CodecRegistry {
    static CODECS: OnceLock<CodecRegistry> = OnceLock::new();
    CODECS.get_or_init(|| {
        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        registry.register_audio_decoder::<symphonia_adapter_libopus::OpusDecoder>();
        registry
    })
}

/// A song being decoded by berry itself (Symphonia), handed to rodio as a stream of samples.
struct Native {
    path: PathBuf,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track: u32,
    channels: u16,
    rate: u32,
    total: Option<Duration>,
    buf: Vec<f32>, // the current decoded chunk, interleaved
    pos: usize,
}

impl Native {
    /// Open a file for playback. `None` if it isn't audio berry can decode.
    fn open(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(&hint, stream, FormatOptions::default(), MetadataOptions::default())
            .ok()?;
        let track = format.default_track(TrackType::Audio)?;
        let Some(CodecParameters::Audio(params)) = &track.codec_params else { return None };
        let decoder = codecs().make_audio_decoder(params, &AudioDecoderOptions::default()).ok()?;
        let total = match (track.time_base, track.duration) {
            (Some(tb), Some(d)) => tb
                .calc_time(Timestamp::from(d.get() as i64))
                .map(|t| Duration::from_secs_f64(t.as_secs_f64().max(0.0))),
            _ => None,
        };
        let mut source = Native {
            path: path.to_path_buf(),
            track: track.id,
            channels: params.channels.as_ref().map_or(2, |c| c.count() as u16),
            rate: params.sample_rate.unwrap_or(44_100),
            format,
            decoder,
            total,
            buf: Vec::new(),
            pos: 0,
        };
        // decode the first chunk now: an undecodable file is caught here, before it's handed to the
        // speakers, and the real channel count and sample rate come from actual audio
        source.refill().then_some(source)
    }

    /// Decode the next chunk of audio into the buffer. False once there's nothing more to play.
    fn refill(&mut self) -> bool {
        self.buf.clear();
        self.pos = 0;
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                _ => return false, // the end, or a container we can't read past
            };
            if packet.track_id != self.track { continue; }
            match self.decoder.decode(&packet) {
                Ok(audio) => {
                    let spec = audio.spec();
                    self.channels = spec.channels().count() as u16;
                    self.rate = spec.rate();
                    audio.copy_to_vec_interleaved(&mut self.buf);
                    if !self.buf.is_empty() { return true; }
                }
                // a damaged packet: skip it and keep going, like any player would
                Err(Error::DecodeError(_)) | Err(Error::IoError(_)) => continue,
                Err(_) => return false,
            }
        }
    }
}

impl Iterator for Native {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = *self.buf.get(self.pos)?;
        self.pos += 1;
        // refill straight away, so the frame length rodio sees is only ever 0 at the very end
        if self.pos >= self.buf.len() { self.refill(); }
        Some(sample)
    }
}

impl Source for Native {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.buf.len() - self.pos)
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let time = Time::try_from_secs_f64(pos.as_secs_f64())
            .ok_or_else(|| SeekError::Other(Box::new(std::io::Error::other("seek time out of range"))))?;
        self.format
            .seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(self.track) })
            .map_err(|e| SeekError::Other(Box::new(std::io::Error::other(e.to_string()))))?;
        self.decoder.reset();
        self.refill();
        Ok(())
    }
}

// ---------- the one berry plays ----------

/// A song for rodio to play: decoded by berry itself when it can, by ffmpeg when it can't.
pub struct FileSource(Inner);

enum Inner {
    Native(Native),
    Piped(Piped),
}

impl FileSource {
    /// Open a file for playback. `None` if neither berry nor ffmpeg can play it.
    pub fn open(path: &Path) -> Option<Self> {
        // WebM goes to ffmpeg first: berry's own decoder plays it, but lands its seeks seconds off
        // (while saying they worked, so nothing could catch it)
        let webm = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("webm"));
        if webm {
            if let Some(p) = Piped::open(path, Duration::ZERO) { return Some(FileSource(Inner::Piped(p))); }
        }
        if let Some(native) = Native::open(path) {
            return Some(FileSource(Inner::Native(native)));
        }
        Piped::open(path, Duration::ZERO).map(|p| FileSource(Inner::Piped(p)))
    }
}

impl Iterator for FileSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        match &mut self.0 {
            Inner::Native(s) => s.next(),
            Inner::Piped(s) => s.next(),
        }
    }
}

impl Source for FileSource {
    fn current_frame_len(&self) -> Option<usize> {
        match &self.0 {
            Inner::Native(s) => s.current_frame_len(),
            Inner::Piped(s) => s.current_frame_len(),
        }
    }

    fn channels(&self) -> u16 {
        match &self.0 {
            Inner::Native(s) => s.channels(),
            Inner::Piped(_) => PIPED_CHANNELS,
        }
    }

    fn sample_rate(&self) -> u32 {
        match &self.0 {
            Inner::Native(s) => s.sample_rate(),
            Inner::Piped(_) => PIPED_RATE,
        }
    }

    fn total_duration(&self) -> Option<Duration> {
        match &self.0 {
            Inner::Native(s) => s.total_duration(),
            Inner::Piped(s) => s.total,
        }
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        match &mut self.0 {
            // berry's own decoder can't seek in some files (AVI, for one): carry on from there with
            // ffmpeg instead, if it's installed
            Inner::Native(s) => match s.try_seek(pos) {
                Ok(()) => Ok(()),
                Err(e) => match Piped::open(&s.path.clone(), pos) {
                    Some(p) => {
                        self.0 = Inner::Piped(p);
                        Ok(())
                    }
                    None => Err(e),
                },
            },
            Inner::Piped(s) => s.seek(pos),
        }
    }
}

// ---------- ffmpeg, for what berry can't decode ----------

/// ffmpeg always hands over the same thing, whatever the file: stereo (surround is mixed down
/// properly, centre channel and all), 48 kHz, as 32-bit floats.
const PIPED_RATE: u32 = 48_000;
const PIPED_CHANNELS: u16 = 2;
/// How much sound is read ahead of what's playing: chunks of 4096 samples (about 43 ms), up to
/// 48 of them (about two seconds), so a busy moment never makes the sound stutter.
const CHUNK: usize = 4096;
const AHEAD: usize = 48;

struct Piped {
    path: PathBuf,
    ffmpeg: Child,
    chunks: Receiver<Vec<f32>>,
    buf: Vec<f32>,
    pos: usize,
    total: Option<Duration>,
}

impl Drop for Piped {
    fn drop(&mut self) {
        let _ = self.ffmpeg.kill();
        let _ = self.ffmpeg.wait();
    }
}

/// Start ffmpeg decoding a file's sound from `from`, and a thread reading it ahead in chunks.
fn pipe(path: &Path, from: Duration) -> Option<(Child, Receiver<Vec<f32>>)> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-loglevel", "error", "-nostdin", "-ss", &format!("{:.3}", from.as_secs_f64()), "-i"])
        .arg(path)
        // the file's first sound track (a film with several languages plays its first)
        .args(["-map", "0:a:0", "-vn", "-ac", &PIPED_CHANNELS.to_string(), "-ar", &PIPED_RATE.to_string(), "-f", "f32le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::video::no_window(&mut cmd);
    let mut ffmpeg = cmd.spawn().ok()?;
    let mut out = ffmpeg.stdout.take()?;
    let (tx, chunks) = sync_channel(AHEAD);
    std::thread::spawn(move || {
        let mut bytes = vec![0u8; CHUNK * 4];
        loop {
            // fill a whole chunk (pipes hand data over in whatever sizes they like)
            let mut filled = 0;
            while filled < bytes.len() {
                match out.read(&mut bytes[filled..]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => filled += n,
                }
            }
            let samples: Vec<f32> = bytes[..filled - filled % 4].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
            let last = filled < bytes.len();
            if !samples.is_empty() && tx.send(samples).is_err() { break; }
            if last { break; }
        }
    });
    Some((ffmpeg, chunks))
}

/// How long a file is, according to ffprobe.
fn probe_length(path: &Path) -> Option<Duration> {
    let mut cmd = Command::new("ffprobe");
    cmd.args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"]).arg(path);
    crate::video::no_window(&mut cmd);
    let out = cmd.output().ok().filter(|o| o.status.success())?;
    let secs: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(Duration::from_secs_f64(secs.max(0.0)))
}

impl Piped {
    fn open(path: &Path, from: Duration) -> Option<Self> {
        let (ffmpeg, chunks) = pipe(path, from)?;
        let mut piped = Piped { path: path.to_path_buf(), ffmpeg, chunks, buf: Vec::new(), pos: 0, total: probe_length(path) };
        // the first sound now: no ffmpeg, or nothing it can decode, means this file can't play
        piped.refill().then_some(piped)
    }

    /// The next chunk of sound. False once there's no more.
    fn refill(&mut self) -> bool {
        self.pos = 0;
        match self.chunks.recv() {
            Ok(chunk) => {
                self.buf = chunk;
                true
            }
            Err(_) => {
                self.buf.clear();
                false
            }
        }
    }

    fn next(&mut self) -> Option<f32> {
        let sample = *self.buf.get(self.pos)?;
        self.pos += 1;
        // refill straight away, so the frame length rodio sees is only ever 0 at the very end
        if self.pos >= self.buf.len() { self.refill(); }
        Some(sample)
    }

    fn current_frame_len(&self) -> Option<usize> {
        Some(self.buf.len() - self.pos)
    }

    /// Jump: start ffmpeg again from there (quick for files on disk).
    fn seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let (ffmpeg, chunks) = pipe(&self.path, pos).ok_or_else(|| SeekError::Other(Box::new(std::io::Error::other("couldn't restart ffmpeg"))))?;
        let _ = self.ffmpeg.kill();
        let _ = self.ffmpeg.wait();
        self.ffmpeg = ffmpeg;
        self.chunks = chunks;
        self.refill();
        Ok(())
    }
}
