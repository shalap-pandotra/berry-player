use lofty::prelude::*;
use lofty::probe::Probe;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use walkdir::WalkDir;

const AUDIO: &[&str] = &["mp3", "flac", "ogg", "opus", "m4a", "aac", "wav", "aiff", "aif", "wma", "ape", "wv", "mka", "ac3", "dts", "tta", "mpc"];

const VIDEO: &[&str] = &["mp4", "m4v", "mkv", "webm", "mov", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "m2ts", "mts", "3gp", "ogv", "vob", "divx"];

pub fn is_video(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| VIDEO.iter().any(|v| v.eq_ignore_ascii_case(e)))
}

#[derive(Clone)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub number: u32,
    pub disc: u32,
    pub year: u32,
    pub added: i64, // when the file landed on disk (seconds since 1970), for "recently added"
    pub duration: Duration,
}

pub struct Album {
    pub title: String,
    pub artist: String,
    pub year: u32,  // earliest year among its tracks, 0 if untagged
    pub added: i64, // newest file in the album
    pub tracks: Vec<Track>,
}

/// Where berry keeps its library cache: %LOCALAPPDATA%\berry\library.db on Windows.
fn db_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))?;
    Some(base.join("berry").join("library.db"))
}

fn open_db() -> Option<Connection> {
    let path = db_path()?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    let conn = Connection::open(path).ok()?;
    // the cache gained columns in version 2; it's only a cache, so older ones are simply rebuilt
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).ok()?;
    if version < 2 {
        conn.execute_batch("DROP TABLE IF EXISTS tracks; PRAGMA user_version = 2;").ok()?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS tracks (
path     TEXT PRIMARY KEY,
root     TEXT NOT NULL,
mtime    INTEGER NOT NULL,
size     INTEGER NOT NULL,
title    TEXT NOT NULL,
artist   TEXT NOT NULL,
album    TEXT NOT NULL,
number   INTEGER NOT NULL,
disc     INTEGER NOT NULL,
year     INTEGER NOT NULL,
added    INTEGER NOT NULL,
duration INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS tracks_root ON tracks(root);",
    )
        .ok()?;
    Some(conn)
}

/// A remembered setting (like how a list is sorted), if there is one.
pub fn setting(key: &str) -> Option<String> {
    open_db()?.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0)).ok()
}

pub fn save_setting(key: &str, value: &str) {
    if let Some(conn) = open_db() {
        let _ = conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)", params![key, value]);
    }
}

/// A file's last-modified time and size (if either changed, its tags get read again),
/// plus when it was created on this disk, which is when it was added to the library.
fn stamp(path: &Path) -> (i64, i64, i64) {
    let secs = |t: std::io::Result<std::time::SystemTime>| {
        t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
    };
    std::fs::metadata(path)
        .map(|m| {
            let mtime = secs(m.modified());
            let created = secs(m.created());
            (mtime, m.len() as i64, if created > 0 { created } else { mtime })
        })
        .unwrap_or((0, 0, 0))
}

/// Walk the music folder and group its tracks into albums. Tags are only read from files that are
/// new or changed since the last scan; everything else comes from the cache, which is what makes
/// launches fast. If the cache can't be opened, every file is read, same as before.
pub fn scan(root: &Path) -> Vec<Album> {
    let files: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| { let x = x.to_ascii_lowercase(); AUDIO.contains(&x.as_str()) || VIDEO.contains(&x.as_str()) })
        })
        .map(|e| e.into_path())
        .collect();

    let mut tracks: Vec<Track> = match open_db() {
        Some(mut conn) => cached_read(&mut conn, root, &files).unwrap_or_else(|_| files.iter().map(|p| read_track(p, stamp(p).2)).collect()),
        None => files.iter().map(|p| read_track(p, stamp(p).2)).collect(),
    };

    // disc before track number, so two-disc albums don't interleave
    tracks.sort_by(|a, b| {
        (a.artist.to_lowercase(), a.album.to_lowercase(), a.disc, a.number, a.title.to_lowercase())
            .cmp(&(b.artist.to_lowercase(), b.album.to_lowercase(), b.disc, b.number, b.title.to_lowercase()))
    });

    let mut albums: Vec<Album> = Vec::new();
    for t in tracks {
        match albums.last_mut() {
            Some(a) if a.title == t.album && a.artist == t.artist => {
                if t.year > 0 && (a.year == 0 || t.year < a.year) { a.year = t.year; }
                a.added = a.added.max(t.added);
                a.tracks.push(t);
            }
            _ => albums.push(Album {
                title: t.album.clone(),
                artist: t.artist.clone(),
                year: t.year,
                added: t.added,
                tracks: vec![t],
            }),
        }
    }
    albums
}

/// Read tracks through the cache: reuse rows whose file is unchanged, read and store the rest,
/// and forget files that are gone.
fn cached_read(conn: &mut Connection, root: &Path, files: &[PathBuf]) -> rusqlite::Result<Vec<Track>> {
    let root_key = root.to_string_lossy().to_string();
    let mut cached: HashMap<String, (i64, i64, Track)> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT path, mtime, size, title, artist, album, number, disc, year, added, duration FROM tracks WHERE root = ?1",
        )?;
        let rows = stmt.query_map(params![root_key], |r| {
            let path: String = r.get(0)?;
            Ok((
                path.clone(),
                (
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    Track {
                        path: PathBuf::from(path),
                        title: r.get(3)?,
                        artist: r.get(4)?,
                        album: r.get(5)?,
                        number: r.get::<_, i64>(6)? as u32,
                        disc: r.get::<_, i64>(7)? as u32,
                        year: r.get::<_, i64>(8)? as u32,
                        added: r.get(9)?,
                        duration: Duration::from_millis(r.get::<_, i64>(10)? as u64),
                    },
                ),
            ))
        })?;
        for row in rows {
            let (k, v) = row?;
            cached.insert(k, v);
        }
    }

    let tx = conn.transaction()?;
    let mut tracks = Vec::with_capacity(files.len());
    {
        let mut upsert = tx.prepare(
            "INSERT OR REPLACE INTO tracks (path, root, mtime, size, title, artist, album, number, disc, year, added, duration)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )?;
        for file in files {
            let key = file.to_string_lossy().to_string();
            let (mtime, size, added) = stamp(file);
            match cached.remove(&key) {
                Some((m, s, t)) if m == mtime && s == size => tracks.push(t),
                _ => {
                    let t = read_track(file, added);
                    upsert.execute(params![
                        key, root_key, mtime, size, t.title, t.artist, t.album, t.number as i64,
                        t.disc as i64, t.year as i64, t.added, t.duration.as_millis() as i64
                    ])?;
                    tracks.push(t);
                }
            }
        }
        // whatever's left in `cached` wasn't found on disk anymore
        let mut forget = tx.prepare("DELETE FROM tracks WHERE path = ?1")?;
        for gone in cached.keys() {
            forget.execute(params![gone])?;
        }
    }
    tx.commit()?;
    Ok(tracks)
}

/// Lyrics for a song: a `.lrc` file with the same name next to it (usually synced), or else lyrics
/// embedded in its tags. The text as-is; `lyrics::parse` makes sense of it.
pub fn lyrics(path: &Path) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(path.with_extension("lrc")) {
        if !text.trim().is_empty() { return Some(text); }
    }
    let file = Probe::open(path).ok()?.read().ok()?;
    file.tags()
        .iter()
        .find_map(|tag| tag.get_string(&ItemKey::Lyrics))
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

fn read_track(path: &Path, added: i64) -> Track {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Unknown").to_string();
    let folder = path
        .parent()
        .and_then(|d| d.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown album")
        .to_string();
    let mut t = Track {
        path: path.to_path_buf(),
        title: stem,
        artist: "Unknown artist".into(),
        album: folder,
        number: 0,
        disc: 0,
        year: 0,
        added,
        duration: Duration::ZERO,
    };
    let read = Probe::open(path).and_then(|p| p.read());
    let understood = read.is_ok();
    if let Ok(file) = read {
        t.duration = file.properties().duration();
        if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
            if let Some(v) = tag.title() { t.title = v.to_string(); }
            if let Some(v) = tag.artist() { t.artist = v.to_string(); }
            if let Some(v) = tag.album() { t.album = v.to_string(); }
            if let Some(v) = tag.track() { t.number = v; }
            if let Some(v) = tag.disk() { t.disc = v; }
            if let Some(v) = tag.year() { t.year = v; }
        }
    }
    // formats the tag reader doesn't know (MKV, AVI, WMA...) still need a length, and a title if
    // the file has one: ffprobe (part of ffmpeg) knows, if it's installed
    if !understood || t.duration.is_zero() {
        if let Some(info) = probe(path) {
            if t.duration.is_zero() { t.duration = info.duration; }
            if !understood {
                if let Some(v) = info.title { t.title = v; }
                if let Some(v) = info.artist { t.artist = v; }
                if let Some(v) = info.album { t.album = v; }
            }
        }
    }
    t
}

/// What ffprobe says about a file: its length, and its title, artist and album if it has them.
struct Probed {
    duration: Duration,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
}

fn probe(path: &Path) -> Option<Probed> {
    let mut cmd = std::process::Command::new("ffprobe");
    cmd.args(["-v", "error", "-show_entries", "format=duration:format_tags=title,artist,album", "-of", "default=noprint_wrappers=1"]).arg(path);
    crate::no_window(&mut cmd);
    let out = cmd.output().ok().filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut info = Probed { duration: Duration::ZERO, title: None, artist: None, album: None };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        if value.is_empty() || value == "N/A" { continue; }
        match key.trim().to_ascii_lowercase().as_str() {
            "duration" => info.duration = value.parse::<f64>().map(|s| Duration::from_secs_f64(s.max(0.0))).unwrap_or_default(),
            "tag:title" => info.title = Some(value.to_string()),
            "tag:artist" => info.artist = Some(value.to_string()),
            "tag:album" => info.album = Some(value.to_string()),
            _ => {}
        }
    }
    Some(info)
}

/// A frame from a video, for its cover when it has no art of its own. Made once with ffmpeg (if
/// it's installed), then kept in %LOCALAPPDATA%\berry\thumbnails.
fn video_frame(path: &Path) -> Option<Vec<u8>> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    std::fs::metadata(path).ok()?.modified().ok()?.hash(&mut h);
    let dir = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("berry").join("thumbnails");
    let file = dir.join(format!("{:016x}.jpg", h.finish()));
    if let Ok(bytes) = std::fs::read(&file) { return Some(bytes); }
    let _ = std::fs::create_dir_all(&dir);
    // a little way in (opening titles are often black), or the very start for short clips
    for at in ["10", "0"] {
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args(["-loglevel", "error", "-nostdin", "-y", "-ss", at, "-i"]).arg(path).args(["-frames:v", "1", "-vf", "scale=640:-2", "-q:v", "4"]).arg(&file);
        crate::no_window(&mut cmd);
        if cmd.status().is_ok_and(|s| s.success()) {
            if let Ok(bytes) = std::fs::read(&file) { if !bytes.is_empty() { return Some(bytes); } }
        }
    }
    None
}

/// Cover art for a track: embedded picture first, then a cover/folder image next to the file.
pub fn cover(path: &Path) -> Option<Vec<u8>> {
    if let Ok(file) = Probe::open(path).and_then(|p| p.read()) {
        for tag in file.tags() {
            if let Some(pic) = tag.pictures().first() {
                return Some(pic.data().to_vec());
            }
        }
    }
    for ext in ["jpg", "png", "webp"] {
        let p = path.with_extension(ext);
        if p.is_file() {
            return std::fs::read(p).ok();
        }
    }
    let dir = path.parent()?;
    for name in ["cover.jpg", "cover.png", "folder.jpg", "folder.png", "front.jpg", "front.png"] {
        let p = dir.join(name);
        if p.is_file() {
            return std::fs::read(p).ok();
        }
    }
    if is_video(path) {
        return video_frame(path);
    }
    None
}
