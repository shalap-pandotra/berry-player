//! The playlists tab, the picker and the prompts: berry's side of playlists. The playlists
//! themselves (reading and writing the .m3u files) live in berry-core.

use crate::library::Track;
use crate::playlist::{load_all, path_key, Playlist};
use crate::{App, Picker, Prompt, PromptKind, SortTarget, Tab};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
