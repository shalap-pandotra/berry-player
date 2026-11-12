//! Playlists, kept as plain `.m3u` files so other players can read them too. berry's own live in a
//! `playlists` folder inside the music folder, with each song written relative to the playlist
//! file, so copying the music folder somewhere else keeps them working. Any other `.m3u` in the
//! music folder shows up too, and is edited in place.

use crate::library::Track;
use crate::{App, Picker, Prompt, PromptKind, SortTarget, Tab};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
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

/// The playlists tab, the picker and the prompts: berry's side of playlists.
impl App {

    pub(crate) fn selected_playlist(&self) -> Option<usize> {
        self.playlist_order.get(self.playlist_state.selected()?).copied()
    }

    /// Read the music folder's playlists again, and re-index the library they point into.
    pub(crate) fn reload_playlists(&mut self) {
        self.song_index.clear();
        for (a, album) in self.albums.iter().enumerate() {
            for (t, track) in album.tracks.iter().enumerate() {
                self.song_index.insert(path_key(&track.path), (a, t));
            }
        }
        self.playlists = load_all(&self.root);
        self.apply_sorts();
        let n = self.playlist_order.len();
        self.playlist_state.select(if n == 0 { None } else { Some(self.playlist_state.selected().unwrap_or(0).min(n - 1)) });
        self.playlist_song_state.select(Some(0));
    }

    /// Whether a file is a song in the library (not a YouTube song, not missing).
    pub(crate) fn in_library(&self, path: &Path) -> bool {
        self.song_index.contains_key(&path_key(path))
    }

    /// A playlist entry as a song: the library's copy, or a stand-in named after the file if it's
    /// gone missing (shown greyed out, skipped when played).
    pub(crate) fn track_for(&self, path: &Path) -> Track {
        match self.song_index.get(&path_key(path)) {
            Some(&(a, t)) => self.albums[a].tracks[t].clone(),
            None => Track {
                path: path.to_path_buf(),
                title: path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
                artist: String::new(),
                album: String::new(),
                number: 0,
                disc: 0,
                year: 0,
                added: 0,
                duration: Duration::ZERO,
            },
        }
    }

    pub(crate) fn playlist_tracks(&self, i: usize) -> Vec<Track> {
        self.playlists[i].songs.iter().map(|p| self.track_for(p)).collect()
    }

    /// `s`: put what's under the cursor into a playlist, through the picker.
    pub(crate) fn open_picker(&mut self) {
        let songs: Vec<Track> = if self.tab == Tab::Queue {
            self.queue_state.selected().and_then(|i| self.queue.get(i)).cloned().into_iter().collect()
        } else {
            match self.selection() {
                Some((tracks, start, true)) => tracks.get(start).cloned().into_iter().collect(),
                Some((tracks, _, false)) => tracks,
                None => Vec::new(),
            }
        };
        let songs: Vec<PathBuf> = songs.into_iter().map(|t| t.path).filter(|p| self.in_library(p)).collect();
        if songs.is_empty() {
            return self.toast("nothing here that can go in a playlist");
        }
        self.picker = Some(Picker { songs, query: String::new(), state: ListState::default().with_selected(Some(0)) });
    }

    /// The playlists the picker is showing for what's been typed, best matches first.
    pub(crate) fn picker_matches(&self) -> Vec<usize> {
        self.picker_matches_for(self.picker.as_ref().map_or("", |p| p.query.as_str()))
    }

    pub(crate) fn picker_key(&mut self, k: KeyEvent) {
        let rows = 1 + self.picker_matches().len();
        let Some(picker) = self.picker.as_mut() else { return };
        let cur = picker.state.selected().unwrap_or(0);
        match k.code {
            KeyCode::Esc => self.picker = None,
            KeyCode::Down | KeyCode::Tab => picker.state.select(Some((cur + 1).min(rows - 1))),
            KeyCode::Up | KeyCode::BackTab => picker.state.select(Some(cur.saturating_sub(1))),
            KeyCode::Backspace => {
                picker.query.pop();
                picker.state.select(Some(0));
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                picker.query.push(c);
                // jump to the best match as you type, so Enter adds to it
                picker.state.select(Some(if rows > 1 { 1 } else { 0 }));
            }
            KeyCode::Enter => {
                let Some(picker) = self.picker.take() else { return };
                match cur.checked_sub(1).and_then(|i| self.picker_matches_for(&picker.query).get(i).copied()) {
                    Some(i) => self.add_to_playlist(i, &picker.songs),
                    None => self.prompt = Some(Prompt { kind: PromptKind::NewPlaylist(picker.songs), text: picker.query }),
                }
            }
            _ => {}
        }
    }

    pub(crate) fn picker_matches_for(&self, query: &str) -> Vec<usize> {
        let q = query.to_lowercase();
        let mut found: Vec<usize> = (0..self.playlists.len()).filter(|&i| self.playlists[i].name.to_lowercase().contains(&q)).collect();
        found.sort_by_key(|&i| (!self.playlists[i].name.to_lowercase().starts_with(&q), self.playlists[i].name.to_lowercase()));
        found
    }

    pub(crate) fn add_to_playlist(&mut self, i: usize, songs: &[PathBuf]) {
        let list = &mut self.playlists[i];
        let before = list.songs.len();
        for s in songs {
            if !list.songs.contains(s) { list.songs.push(s.clone()); }
        }
        let (added, name) = (list.songs.len() - before, list.name.clone());
        if added == 0 {
            return self.toast(format!("already in {name}"));
        }
        if let Err(e) = list.save() {
            return self.toast(format!("couldn't save {name}: {e}"));
        }
        self.apply_sorts();
        self.toast(format!("added {added} song{} to {name}", if added == 1 { "" } else { "s" }));
    }

    pub(crate) fn prompt_key(&mut self, k: KeyEvent) {
        let Some(prompt) = self.prompt.as_mut() else { return };
        // deleting is a yes/no: y goes ahead, anything else backs out
        if let PromptKind::Delete(i) = prompt.kind {
            self.prompt = None;
            if k.code == KeyCode::Char('y') { self.delete_playlist(i); }
            return;
        }
        match k.code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Backspace => { prompt.text.pop(); }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => prompt.text.push(c),
            KeyCode::Enter => {
                let Some(Prompt { kind, text }) = self.prompt.take() else { return };
                match kind {
                    PromptKind::NewPlaylist(songs) => self.new_playlist(&text, &songs),
                    PromptKind::Rename(i) => match self.playlists[i].rename(&text) {
                        Ok(()) => {
                            self.set_sort(SortTarget::Playlists, self.sorts[3]); // re-sort, cursor stays on it
                            self.toast(format!("renamed to {}", self.playlists[i].name));
                        }
                        Err(e) => self.toast(e),
                    },
                    PromptKind::Delete(_) => {}
                }
            }
            _ => {}
        }
    }

    /// Make a playlist (from the picker, or :save) with these songs in it.
    pub(crate) fn new_playlist(&mut self, name: &str, songs: &[PathBuf]) {
        match Playlist::create(&self.root, name) {
            Ok(list) => {
                self.playlists.push(list);
                let i = self.playlists.len() - 1;
                self.add_to_playlist(i, songs);
                self.apply_sorts();
                self.playlist_state.select(self.playlist_order.iter().position(|&x| x == i));
            }
            Err(e) => self.toast(e),
        }
    }

    pub(crate) fn delete_playlist(&mut self, i: usize) {
        let name = self.playlists[i].name.clone();
        if let Err(e) = self.playlists[i].delete() {
            return self.toast(format!("couldn't delete {name}: {e}"));
        }
        self.playlists.remove(i);
        self.apply_sorts();
        let n = self.playlist_order.len();
        self.playlist_state.select(if n == 0 { None } else { Some(self.playlist_state.selected().unwrap_or(0).min(n - 1)) });
        self.toast(format!("deleted {name}"));
    }

    /// Inside a playlist: move the song under the cursor (J/K) or take it out (d), saving as you go.
    pub(crate) fn playlist_edit(&mut self, delta: Option<isize>) {
        let (Some(i), Some(s)) = (self.selected_playlist(), self.playlist_song_state.selected()) else { return };
        let list = &mut self.playlists[i];
        if s >= list.songs.len() { return; }
        match delta {
            Some(d) => {
                let to = s as isize + d;
                if to < 0 || to >= list.songs.len() as isize { return; }
                list.songs.swap(s, to as usize);
                self.playlist_song_state.select(Some(to as usize));
            }
            None => {
                list.songs.remove(s);
                let n = list.songs.len();
                self.playlist_song_state.select(if n == 0 { None } else { Some(s.min(n - 1)) });
            }
        }
        if let Err(e) = self.playlists[i].save() {
            let name = self.playlists[i].name.clone();
            self.toast(format!("couldn't save {name}: {e}"));
        }
    }

    /// While filtering the playlists list: type to narrow it, Enter to keep the filter, Esc to drop it.
    pub(crate) fn filter_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => {
                self.filtering = false;
                self.playlist_filter.clear();
            }
            KeyCode::Enter | KeyCode::Down => self.filtering = false,
            KeyCode::Backspace => { self.playlist_filter.pop(); }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => self.playlist_filter.push(c),
            _ => return,
        }
        self.apply_sorts();
        self.playlist_state.select(if self.playlist_order.is_empty() { None } else { Some(0) });
        self.playlist_song_state.select(Some(0));
    }

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
