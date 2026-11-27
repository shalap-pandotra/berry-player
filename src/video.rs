use crate::App;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FPS: u32 = 12;

/// A running decode: ffmpeg turning a video into frames from `start` onwards.
struct Stream {
    key: String, // which video (its file, or its YouTube id)
    size: (usize, usize),
    start: Duration,
    frames: Receiver<(Duration, String)>,
    ffmpeg: Child,
    next: Option<(Duration, String)>, // the next frame, waiting for its moment
    shown: Option<Duration>,          // when the frame on screen belongs
    said: Arc<Mutex<String>>,         // what ffmpeg printed (its errors, if it fails)
    done: Option<Arc<AtomicBool>>,    // set when this stream stops (YouTube's relay listens for it)
}

impl Drop for Stream {
    fn drop(&mut self) {
        if let Some(done) = &self.done { done.store(true, Ordering::Relaxed); }
        let _ = self.ffmpeg.kill();
        let _ = self.ffmpeg.wait();
    }
}

pub struct State {
    pub on: bool,
    stream: Option<Stream>,
    pub failed: HashSet<String>, // videos that didn't work, so berry doesn't keep retrying them
    showing: bool,               // a frame is on screen (so the cover needs repainting when video stops)
}

impl State {
    pub fn new() -> Self {
        State { on: crate::library::setting("video").or_else(|| crate::library::setting("yt.video")).as_deref() == Some("on"), stream: None, failed: HashSet::new(), showing: false }
    }
}

/// Whether ffmpeg can be run.
pub fn ffmpeg_available() -> bool {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-version").stdout(Stdio::null()).stderr(Stdio::null());
    no_window(&mut cmd);
    cmd.status().is_ok()
}

/// Background programs shouldn't flash a console window up on Windows.
pub fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let _ = cmd;
}

/// Video's own log, always kept (it's small: a few lines per video), so when a video fails the
/// reason's already written down: %LOCALAPPDATA%\berry\video.log. Trimmed to its most recent part
/// once it grows past about 400 KB.
pub fn log(line: &str) {
    let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let path = base.join("berry").join("video.log");
    if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 400_000) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let cut = (text.len() - 200_000..text.len()).find(|&i| text.is_char_boundary(i)).unwrap_or(0);
            let keep = &text[cut..];
            let keep = keep.find('\n').map_or(keep, |i| &keep[i + 1..]); // from a whole line
            let _ = std::fs::write(&path, keep);
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(f, "[{:02}:{:02}:{:02} UTC] {line}", secs / 3600 % 24, secs / 60 % 60, secs % 60);
    }
}

/// `:video`: the picture for videos, on or off (remembered).
pub fn toggle(app: &mut App) {
    if !app.video.on && !ffmpeg_available() {
        return app.toast("video needs ffmpeg installed (videos still play as sound without it)");
    }
    app.video.on = !app.video.on;
    crate::library::save_setting("video", if app.video.on { "on" } else { "off" });
    if app.video.on {
        app.toast("video on: videos play their picture in the cover box");
    } else {
        stop(app);
        app.toast("video off");
    }
}

/// Whether a video's picture is playing, so berry wakes often enough to keep up with it.
pub fn active(app: &App) -> bool {
    app.video.stream.is_some()
}

fn stop(app: &mut App) {
    app.video.stream = None;
    if app.video.showing {
        app.video.showing = false;
        app.art_dirty = true; // the cover goes back
    }
}

/// Where the picture comes from.
enum From {
    File(String),
    #[cfg(feature = "youtube")]
    YouTube(String),
}

/// The video playing right now, if any: a video file, or a YouTube song.
fn playing_video(app: &App) -> Option<(String, From)> {
    if app.waiting { return None; }
    let path = &app.playing()?.path;
    if crate::library::is_video(path) {
        let p = path.to_string_lossy().into_owned();
        return Some((p.clone(), From::File(p)));
    }
    #[cfg(feature = "youtube")]
    if let Some(id) = crate::youtube::video::id_for(path) {
        return Some((id.clone(), From::YouTube(id)));
    }
    None
}

/// The frame to paint now, as Sixel, or None if nothing new is due. Called once per loop.
pub fn frame(app: &mut App) -> Option<String> {
    let wanted = if app.video.on { playing_video(app) } else { None };
    // nothing to show, or something's drawn over the cover box
    let covered = app.help.is_some() || app.picker.is_some() || app.prompt.is_some() || app.chooser.is_some() || app.menu.is_some();
    let Some((key, from)) = wanted else {
        stop(app);
        return None;
    };
    if covered || app.video.failed.contains(&key) { return None; }
    let area = app.cover_area;
    if area.width == 0 || area.height == 0 { return None; }
    // a 16:9 picture as big as the box allows (even sizes, which video scaling likes)
    let (bw, bh) = (area.width as usize * crate::CELL_W as usize, area.height as usize * crate::CELL_H as usize);
    let (mut w, mut h) = (bw, bw * 9 / 16);
    if h > bh { h = bh; w = bh * 16 / 9; }
    let size = (w & !1, h & !1);
    let pos = app.sink.get_pos();

    // the right stream running for this video, at this size, near this moment?
    let restart = match &app.video.stream {
        None => true,
        Some(s) => s.key != key || s.size != size || pos + Duration::from_secs(1) < s.start || s.shown.is_some_and(|t| pos > t + Duration::from_secs(3)),
    };
    if restart {
        app.video.stream = None;
        let (input, done) = match from {
            From::File(path) => (path, None),
            // YouTube mode finds the stream (it may still be looking: the cover shows meanwhile)
            #[cfg(feature = "youtube")]
            From::YouTube(id) => match crate::youtube::video::open(app, &id) {
                Some((url, done)) => (url, Some(done)),
                None => return None,
            },
        };
        match start(&key, &input, size, pos, done) {
            Some(stream) => app.video.stream = Some(stream),
            None => return None,
        }
    }

    // the newest frame that's due by now
    let stream = app.video.stream.as_mut()?;
    let mut due = None;
    loop {
        let next = match stream.next.take() {
            Some(f) => f,
            None => match stream.frames.try_recv() {
                Ok(f) => f,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // ffmpeg stopped. Fine at the end of the video; if it never showed a frame, it failed
                    if stream.shown.is_none() {
                        let said = stream.said.lock().map(|s| s.clone()).unwrap_or_default();
                        log(&format!("ffmpeg stopped without a frame for {}: {said}", stream.key));
                        let reason = said.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("no frames came out").to_string();
                        let failed = stream.key.clone();
                        app.video.stream = None;
                        app.video.failed.insert(failed);
                        app.toast(format!("video: ffmpeg couldn't play this one ({reason}; details in berry's video.log)"));
                        return None;
                    }
                    break;
                }
            },
        };
        if next.0 <= pos {
            due = Some(next);
        } else {
            stream.next = Some(next);
            break;
        }
    }
    let (time, sixel) = due?;
    if stream.shown.is_none() {
        log(&format!("first frame for {} at {:.2}s ({} bytes of sixel)", stream.key, time.as_secs_f64(), sixel.len()));
    }
    stream.shown = Some(time);
    app.video.showing = true;
    Some(sixel)
}

/// Start ffmpeg decoding `input` (a file, or a local address YouTube mode's relay serves) from
/// `from`, fitted into `size` (with bars if its shape differs), and a thread turning its frames
/// into Sixel. Only a few frames are kept ready at a time, so ffmpeg waits rather than racing ahead
/// (which is also what makes pausing free).
fn start(key: &str, input: &str, size: (usize, usize), from: Duration, done: Option<Arc<AtomicBool>>) -> Option<Stream> {
    let (w, h) = size;
    let fit = format!("fps={FPS},scale={w}:{h}:force_original_aspect_ratio=decrease:flags=fast_bilinear,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2");
    let mut cmd = Command::new("ffmpeg");
    // the graphics card decodes when it can (much lighter for heavy videos like 4K HEVC);
    // ffmpeg quietly falls back to doing it in software when it can't
    cmd.args(["-loglevel", "error", "-nostdin", "-hwaccel", "auto", "-ss", &format!("{:.2}", from.as_secs_f64())]);
    cmd.args(["-i", input, "-an", "-vf", &fit, "-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_window(&mut cmd);
    log(&format!("starting ffmpeg for {key} at {:.2}s, {w}x{h}: ffmpeg {}", from.as_secs_f64(), cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")));
    let mut ffmpeg = match cmd.spawn() {
        Ok(f) => f,
        Err(e) => {
            if let Some(d) = &done { d.store(true, Ordering::Relaxed); }
            log(&format!("couldn't start ffmpeg: {e}"));
            return None;
        }
    };
    let mut out = ffmpeg.stdout.take()?;
    // keep what ffmpeg says (on the side, so it can never stall on a full pipe)
    let said = Arc::new(Mutex::new(String::new()));
    if let Some(mut err) = ffmpeg.stderr.take() {
        let said = said.clone();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = err.read_to_string(&mut text);
            if let Ok(mut s) = said.lock() { *s = text; }
        });
    }
    let (tx, frames) = sync_channel(3);
    std::thread::spawn(move || {
        let mut buf = vec![0u8; w * h * 3];
        let mut n: u32 = 0;
        while out.read_exact(&mut buf).is_ok() {
            let time = from + Duration::from_secs_f64(n as f64 / FPS as f64);
            if tx.send((time, encode(&buf, w, h))).is_err() { break; }
            n += 1;
        }
    });
    Some(Stream { key: key.to_string(), size, start: from, frames, ffmpeg, next: None, shown: None, said, done })
}

// ---------- the fast Sixel encoder ----------

/// The fixed palette: 6 levels of red, 7 of green (eyes are pickiest about green), 6 of blue.
const LEVELS: (usize, usize, usize) = (6, 7, 6);

/// 4x4 ordered-dither pattern: nudges each pixel up or down a little before rounding, in a fixed
/// pattern, so in-between colours come out as an even texture instead of bands.
const BAYER: [[i32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

fn level(v: u8, levels: usize, nudge: i32) -> usize {
    let step = 255 / (levels as i32 - 1);
    let v = v as i32 + (nudge - 8) * step / 16;
    ((v.clamp(0, 255) * (levels as i32 - 1) + 127) / 255) as usize
}

/// One RGB frame as Sixel.
pub fn encode(rgb: &[u8], w: usize, h: usize) -> String {
    let (lr, lg, lb) = LEVELS;
    let colours = lr * lg * lb;
    let index: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let nudge = BAYER[y % 4][x % 4];
            let p = &rgb[i * 3..i * 3 + 3];
            (level(p[0], lr, nudge) * lg * lb + level(p[1], lg, nudge) * lb + level(p[2], lb, nudge)) as u8
        })
        .collect();
    let mut out = String::with_capacity(w * h / 2);
    out.push_str(&format!("\x1bP0;1;0q\"1;1;{w};{h}"));
    for c in 0..colours {
        let (r, g, b) = (c / (lg * lb), c / lb % lg, c % lb);
        out.push_str(&format!("#{c};2;{};{};{}", r * 100 / (lr - 1), g * 100 / (lg - 1), b * 100 / (lb - 1)));
    }
    let mut used = vec![false; colours];
    let mut row = vec![0u8; w];
    for top in (0..h).step_by(6) {
        let rows = (h - top).min(6);
        used.iter_mut().for_each(|u| *u = false);
        for y in top..top + rows {
            for x in 0..w { used[index[y * w + x] as usize] = true; }
        }
        let mut first = true;
        for c in (0..colours).filter(|&c| used[c]) {
            for (x, cell) in row.iter_mut().enumerate() {
                let mut bits = 0u8;
                for dy in 0..rows {
                    if index[(top + dy) * w + x] as usize == c { bits |= 1 << dy; }
                }
                *cell = bits;
            }
            if !first { out.push('$'); } // back to the start of the band for the next colour
            first = false;
            out.push_str(&format!("#{c}"));
            // run-length: "!12~" is twelve of "~"
            let mut x = 0;
            while x < w {
                let ch = (63 + row[x]) as char;
                let mut run = 1;
                while x + run < w && row[x + run] == row[x] { run += 1; }
                if run > 3 { out.push_str(&format!("!{run}{ch}")); } else { (0..run).for_each(|_| out.push(ch)); }
                x += run;
            }
        }
        out.push('-'); // next band
    }
    out.push_str("\x1b\\");
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn encodes_a_frame() {
        // a 12x7 frame: left half red, right half blue, bottom row white
        let (w, h) = (12, 7);
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let p = &mut rgb[(y * w + x) * 3..(y * w + x) * 3 + 3];
                p.copy_from_slice(if y == 6 { &[255, 255, 255] } else if x < 6 { &[255, 0, 0] } else { &[0, 0, 255] });
            }
        }
        let s = super::encode(&rgb, w, h);
        assert!(s.starts_with("\x1bP0;1;0q\"1;1;12;7") && s.ends_with("\x1b\\"));
        // red is colour 5*42 = 210, blue is 5, white is 251; first band: six rows of each half
        assert!(s.contains("#210!6~"), "{s}");
        assert!(s.contains("#5??????!6~") || s.contains("#5!6?!6~"), "{s}");
        // second band: one row of white across the whole width
        assert!(s.contains("#251!12@"), "{s}");
    }
}
