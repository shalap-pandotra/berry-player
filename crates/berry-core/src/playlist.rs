//! Playlists, kept as plain `.m3u` files so other players can read them too. berry's own live in a
//! `playlists` folder inside the music folder, with each song written relative to the playlist
//! file, so copying the music folder somewhere else keeps them working. Any other `.m3u` in the
//! music folder shows up too, and is edited in place.

use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

pub struct Playlist {
    pub name: String,
    pub file: PathBuf,
    pub songs: Vec<PathBuf>, // full paths
    pub modified: i64,       // when the file last changed, for "recently changed" sorting
}

/// Where berry keeps the playlists it makes.
pub fn folder(root: &Path) -> PathBuf {
    root.join("playlists")
}

/// Every playlist in the music folder, wherever it sits.
pub fn load_all(root: &Path) -> Vec<Playlist> {
    WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("m3u") || x.eq_ignore_ascii_case("m3u8")))
        .filter_map(|e| read(e.path()))
        .collect()
}

fn read(file: &Path) -> Option<Playlist> {
    let bytes = std::fs::read(file).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let dir = file.parent()?;
    let songs = text
        .lines()
        .map(|l| l.trim().trim_start_matches('\u{feff}'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter(|l| !l.contains("://")) // web streams: not something berry plays
        .map(|l| normalise(&dir.join(l.replace(['\\', '/'], std::path::MAIN_SEPARATOR_STR))))
        .collect();
    Some(Playlist {
        name: file.file_stem()?.to_string_lossy().into_owned(),
        file: file.to_path_buf(),
        songs,
        modified: modified(file),
    })
}

fn modified(file: &Path) -> i64 {
    std::fs::metadata(file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// Tidy a path without touching the disk: drop `.` and resolve `..`.
pub fn normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => { out.pop(); }
            other => out.push(other),
        }
    }
    out
}

/// `to` written relative to the folder `from`, if they share a drive (otherwise the full path).
fn relative(to: &Path, from: &Path) -> PathBuf {
    let (a, b): (Vec<_>, Vec<_>) = (to.components().collect(), from.components().collect());
    let same_root = a.first().map(|c| c.as_os_str().to_ascii_lowercase()) == b.first().map(|c| c.as_os_str().to_ascii_lowercase());
    if !same_root { return to.to_path_buf(); }
    let shared = a.iter().zip(&b).take_while(|(x, y)| x.as_os_str().eq_ignore_ascii_case(y.as_os_str())).count();
    let mut out = PathBuf::new();
    for _ in shared..b.len() { out.push(".."); }
    for c in &a[shared..] { out.push(c); }
    out
}

/// Characters Windows won't allow in a file name.
fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')).collect::<String>().trim().trim_end_matches('.').to_string()
}

impl Playlist {
    /// Write the playlist back to its file.
    pub fn save(&mut self) -> std::io::Result<()> {
        let dir = self.file.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut text = String::from("#EXTM3U\n");
        for song in &self.songs {
            text.push_str(&relative(song, &dir).to_string_lossy());
            text.push('\n');
        }
        std::fs::write(&self.file, text)?;
        self.modified = modified(&self.file);
        Ok(())
    }

    /// A new, empty playlist in berry's playlists folder.
    pub fn create(root: &Path, name: &str) -> Result<Playlist, String> {
        let name = clean_name(name);
        if name.is_empty() { return Err("a playlist needs a name".into()); }
        let dir = folder(root);
        std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
        let file = dir.join(format!("{name}.m3u"));
        if file.exists() { return Err(format!("there's already a playlist called {name}")); }
        let mut p = Playlist { name, file, songs: Vec::new(), modified: 0 };
        p.save().map_err(|e| format!("couldn't save the playlist: {e}"))?;
        Ok(p)
    }

    /// Rename the playlist, and its file with it.
    pub fn rename(&mut self, name: &str) -> Result<(), String> {
        let name = clean_name(name);
        if name.is_empty() { return Err("a playlist needs a name".into()); }
        let ext = self.file.extension().map_or("m3u".into(), |e| e.to_string_lossy().into_owned());
        let file = self.file.with_file_name(format!("{name}.{ext}"));
        // case-only renames (mix -> Mix) are the same file on Windows, so allow those
        if file.exists() && !file.to_string_lossy().eq_ignore_ascii_case(&self.file.to_string_lossy()) {
            return Err(format!("there's already a playlist called {name}"));
        }
        std::fs::rename(&self.file, &file).map_err(|e| format!("couldn't rename it: {e}"))?;
        self.file = file;
        self.name = name;
        Ok(())
    }

    pub fn delete(&self) -> std::io::Result<()> {
        std::fs::remove_file(&self.file)
    }
}

/// A path as a lookup key: Windows doesn't care about case, so neither does this.
pub fn path_key(p: &Path) -> String {
    normalise(p).to_string_lossy().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let root = std::env::temp_dir().join("berry-playlist-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Artist/Album")).unwrap();
        let song = root.join("Artist/Album/01 Song.flac");
        std::fs::write(&song, b"").unwrap();
        // made, saved with a relative path, read back as the same full path
        let mut p = Playlist::create(&root, "road: trip?").unwrap();
        assert_eq!(p.name, "road trip");
        p.songs.push(song.clone());
        p.save().unwrap();
        let text = std::fs::read_to_string(&p.file).unwrap();
        assert!(text.contains(&format!("..{0}Artist{0}Album{0}01 Song.flac", std::path::MAIN_SEPARATOR)), "{text}");
        let found = load_all(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].songs, vec![song.clone()]);
        // a playlist from elsewhere, with comments and the other slash style, sitting in the library
        std::fs::write(root.join("Artist/other.m3u8"), "#EXTM3U\n#EXTINF:123,Artist - Song\nAlbum/01 Song.flac\nhttp://radio.example/stream\n").unwrap();
        let mut found = load_all(&root);
        found.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(found[0].name, "other");
        assert_eq!(found[0].songs, vec![song.clone()]);
        // renaming, clashes, and deleting
        assert!(p.rename("other").is_ok()); // different folder, so no clash
        assert!(Playlist::create(&root, "other").is_err());
        p.delete().unwrap();
        assert_eq!(load_all(&root).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
