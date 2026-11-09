use lofty::prelude::*;
use lofty::probe::Probe;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use walkdir::WalkDir;

const AUDIO: &[&str] = &["mp3", "flac", "ogg", "m4a", "aac", "wav", "aiff", "aif"];

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
                .is_some_and(|x| AUDIO.contains(&x.to_ascii_lowercase().as_str()))
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
    if let Ok(file) = Probe::open(path).and_then(|p| p.read()) {
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
    t
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
    let dir = path.parent()?;
    for name in ["cover.jpg", "cover.png", "folder.jpg", "folder.png", "front.jpg", "front.png"] {
        let p = dir.join(name);
        if p.is_file() {
            return std::fs::read(p).ok();
        }
    }
    None
}
