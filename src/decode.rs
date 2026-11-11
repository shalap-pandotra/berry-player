//! berry's own decoder: Symphonia (plus libopus, for Opus) turning a file into samples for rodio
//! to play. Doing this ourselves means a file the decoder can't handle is an ordinary error rather
//! than a crash, and berry can play Opus, which rodio's built-in decoder can't.

use rodio::source::SeekError;
use rodio::Source;
use std::fs::File;
use std::path::Path;
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

/// A song being decoded, handed to rodio as a stream of samples.
pub struct FileSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track: u32,
    channels: u16,
    rate: u32,
    total: Option<Duration>,
    buf: Vec<f32>, // the current decoded chunk, interleaved
    pos: usize,
}

impl FileSource {
    /// Open a file for playback. `None` if it isn't audio berry can decode.
    pub fn open(path: &Path) -> Option<Self> {
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
        let mut source = FileSource {
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

impl Iterator for FileSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = *self.buf.get(self.pos)?;
        self.pos += 1;
        // refill straight away, so the frame length rodio sees is only ever 0 at the very end
        if self.pos >= self.buf.len() { self.refill(); }
        Some(sample)
    }
}

impl Source for FileSource {
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
