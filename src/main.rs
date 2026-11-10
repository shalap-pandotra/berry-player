mod icon;
mod library;
mod sixel;
mod ui;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use library::{Album, Track};
use ratatui::{layout::Rect, widgets::ListState, DefaultTerminal};
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, Sink};
use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufReader, Read, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

// Pixel size of one terminal cell, as Windows Terminal reports it (CSI 16 t). Change if your font size differs.
pub const CELL_W: u32 = 10;
pub const CELL_H: u32 = 20;

/// How long a toast stays on screen.
const TOAST_FOR: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, PartialEq)]
pub enum Tab { Queue, Albums, Artists, Search }

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Queue, Tab::Albums, Tab::Artists, Tab::Search];
    pub fn name(self) -> &'static str {
        match self { Tab::Queue => "queue", Tab::Albums => "albums", Tab::Artists => "artists", Tab::Search => "search" }
    }
    fn index(self) -> usize { self as usize }
    /// How many columns this tab has to move between with h/l.
    fn columns(self) -> usize {
        match self { Tab::Albums => 2, Tab::Artists => 3, _ => 1 }
    }
}

pub struct Artist {
    pub name: String,
    pub albums: Vec<usize>, // indices into App::albums
}

/// The two help panels: `?` for keys, `:help` for commands.
#[derive(Clone, Copy, PartialEq)]
pub enum Help { Keys, Commands }

/// What happens when the queue runs out (or, for One, when a song ends).
#[derive(Clone, Copy, PartialEq)]
pub enum Repeat { Off, All, One }

/// Commands the `:` line knows, for Tab completion and the hints shown while typing.
pub const COMMANDS: &[&str] = &[
    "clear", "folder", "help", "icons", "quit", "repeat", "rescan", "seek", "shuffle", "volume",
];

/// The lists that can be sorted from the `o` menu.
#[derive(Clone, Copy, PartialEq)]
pub enum SortTarget { Albums, Artists, ArtistAlbums }

impl SortTarget {
    pub fn options(self) -> &'static [&'static str] {
        match self {
            SortTarget::Albums => &[
                "title, a to z", "title, z to a", "artist, a to z", "artist, z to a",
                "newest year first", "oldest year first", "recently added", "added longest ago",
            ],
            SortTarget::Artists => &["name, a to z", "name, z to a", "most albums", "fewest albums"],
            SortTarget::ArtistAlbums => &["oldest year first", "newest year first", "title, a to z", "title, z to a"],
        }
    }
    fn index(self) -> usize { self as usize }
    fn setting(self) -> &'static str {
        match self { SortTarget::Albums => "sort.albums", SortTarget::Artists => "sort.artists", SortTarget::ArtistAlbums => "sort.artist_albums" }
    }
    pub fn noun(self) -> &'static str {
        match self { SortTarget::Artists => "artists", _ => "albums" }
    }
}

/// The open sort menu.
pub struct SortMenu {
    pub target: SortTarget,
    pub state: ListState,
}

/// Names sort the way record shops file them: "The Cure" under C.
fn sort_name(s: &str) -> String {
    let l = s.to_lowercase();
    l.strip_prefix("the ").map(str::to_string).unwrap_or(l)
}

/// One search result.
#[derive(Clone, Copy)]
pub enum Hit { Artist(usize), Album(usize), Song(usize, usize) }

pub struct Toast {
    pub text: String,
    shown: Instant,
}

pub struct App {
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub tab: Tab,
    pub before_search: Tab, // the screen search opened over, shown until something's typed
    pub last_tab: Tab,      // where ` jumps back to
    pub cols: [usize; 4], // the focused column in each tab
    // sorting: the chosen option for each SortTarget, and the resulting display orders
    pub sorts: [usize; 3],
    pub album_order: Vec<usize>,  // albums tab: row -> index into albums
    pub artist_order: Vec<usize>, // artists tab: row -> index into artists
    pub menu: Option<SortMenu>,
    pub sort_anchors: [Rect; 3], // where each sortable list was drawn, so its menu opens in its corner
    // the ribbon on the highlighted row: which list and row it's on, and since when
    pub marquee_key: (u8, usize),
    pub marquee_since: Instant,
    // queue
    pub queue: Vec<Track>,
    pub current: Option<usize>,
    pub queue_state: ListState,
    // albums tab
    pub album_state: ListState,
    pub album_track_state: ListState,
    // artists tab
    pub artist_state: ListState,
    pub artist_album_state: ListState,
    pub artist_track_state: ListState,
    // search tab
    pub query: String,
    pub editing: bool,
    pub hits: Vec<Hit>,
    pub hit_state: ListState,
    pub help: Option<Help>,
    // audio
    _stream: OutputStream,
    pub sink: Sink,
    device_name: String,
    last_device_check: Instant,
    pub volume: f32,
    pub nerd_icons: bool, // Nerd Font icons for the playback controls (:icons nerd, or BERRY_ICONS=nerd)
    pub repeat: Repeat,
    pub unshuffled: Option<Vec<Track>>, // the queue's order before :shuffle, so a second :shuffle can undo it
    root: PathBuf, // the music folder being shown
    // the : command line: what's being typed (None when closed), and earlier commands for Up/Down
    pub command: Option<String>,
    history: Vec<String>,
    history_pos: Option<usize>,
    // cover art
    pub cover_area: Rect,
    cover: Option<Vec<u8>>,
    art_cache: HashMap<(u16, u16), sixel::Art>, // the current cover, encoded once per box size
    art_area: Rect,                              // where the cover was last laid out
    art_dirty: bool,
    // with nothing playing, the cover box previews the album under the cursor
    preview: Option<usize>,                    // the album being previewed
    preview_pending: Option<(Option<usize>, Instant)>, // the album the cursor stopped on, and since when
    needs_clear: bool,
    // toasts
    pub toasts: Vec<Toast>,
    pub toast_spots: Vec<Rect>, // where each toast's icon goes, filled in while drawing
    painted_spots: Vec<Rect>,   // where icons were last painted
    toast_icon: String,         // the icon as Sixel, headphones in the terminal's text colour
    stderr: Option<gag::BufferRedirect>,
    quit: bool,
}

impl App {
    /// The screen actually on display: search shows the screen it opened over until there's a query.
    pub fn shown_tab(&self) -> Tab {
        if self.tab == Tab::Search && self.query.is_empty() { self.before_search } else { self.tab }
    }

    pub fn col(&self) -> usize { self.cols[self.shown_tab().index()] }

    pub fn playing(&self) -> Option<&Track> {
        self.current.and_then(|i| self.queue.get(i))
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast { text: text.into(), shown: Instant::now() });
    }

    /// Anything the audio libraries print would scribble over the interface,
    /// so error output is captured and shown as toasts instead.
    fn collect_stderr(&mut self) {
        let mut buf = String::new();
        if let Some(r) = self.stderr.as_mut() { let _ = r.read_to_string(&mut buf); }
        for line in buf.lines().map(str::trim).filter(|l| !l.is_empty()) {
            // a vanished device is handled by switching outputs, which gets its own toast
            if line.contains("no longer available") { continue; }
            self.toast(line.to_string());
        }
        self.toasts.retain(|t| t.shown.elapsed() < TOAST_FOR);
    }

    // ---------- selection helpers ----------

    /// The album under the cursor in the albums tab, as an index into self.albums.
    pub fn selected_album(&self) -> Option<usize> {
        self.album_order.get(self.album_state.selected()?).copied()
    }

    /// The artist under the cursor in the artists tab, as an index into self.artists.
    pub fn selected_artist(&self) -> Option<usize> {
        self.artist_order.get(self.artist_state.selected()?).copied()
    }

    /// The album picked in the artists tab, as an index into self.albums.
    pub fn artist_album(&self) -> Option<usize> {
        let a = self.selected_artist()?;
        self.artists.get(a)?.albums.get(self.artist_album_state.selected()?).copied()
    }

    // ---------- sorting ----------

    fn load_sorts(&mut self) {
        for t in [SortTarget::Albums, SortTarget::Artists, SortTarget::ArtistAlbums] {
            if let Some(n) = library::setting(t.setting()).and_then(|v| v.parse::<usize>().ok()) {
                if n < t.options().len() { self.sorts[t.index()] = n; }
            }
        }
    }

    /// Rebuild the display orders from the chosen sorts. Untagged years always go last.
    fn apply_sorts(&mut self) {
        use std::cmp::Reverse;
        let albums = &self.albums;
        let mut order: Vec<usize> = (0..albums.len()).collect();
        let title = |i: &usize| albums[*i].title.to_lowercase();
        let by_artist = |i: &usize| (sort_name(&albums[*i].artist), albums[*i].year, albums[*i].title.to_lowercase());
        match self.sorts[0] {
            0 => order.sort_by_key(title),
            1 => order.sort_by_key(|i| Reverse(title(i))),
            2 => order.sort_by_key(by_artist),
            3 => order.sort_by(|a, b| sort_name(&albums[*b].artist).cmp(&sort_name(&albums[*a].artist)).then(by_artist(a).cmp(&by_artist(b)))),
            4 => order.sort_by_key(|i| (albums[*i].year == 0, Reverse(albums[*i].year))),
            5 => order.sort_by_key(|i| (albums[*i].year == 0, albums[*i].year)),
            6 => order.sort_by_key(|i| Reverse(albums[*i].added)),
            _ => order.sort_by_key(|i| albums[*i].added),
        }
        self.album_order = order;

        let artists = &self.artists;
        let mut order: Vec<usize> = (0..artists.len()).collect();
        match self.sorts[1] {
            0 => order.sort_by_key(|i| sort_name(&artists[*i].name)),
            1 => order.sort_by_key(|i| Reverse(sort_name(&artists[*i].name))),
            2 => order.sort_by_key(|i| (Reverse(artists[*i].albums.len()), sort_name(&artists[*i].name))),
            _ => order.sort_by_key(|i| (artists[*i].albums.len(), sort_name(&artists[*i].name))),
        }
        self.artist_order = order;

        let choice = self.sorts[2];
        for artist in &mut self.artists {
            match choice {
                0 => artist.albums.sort_by_key(|i| (albums[*i].year == 0, albums[*i].year)),
                1 => artist.albums.sort_by_key(|i| (albums[*i].year == 0, Reverse(albums[*i].year))),
                2 => artist.albums.sort_by_key(|i| albums[*i].title.to_lowercase()),
                _ => artist.albums.sort_by_key(|i| Reverse(albums[*i].title.to_lowercase())),
            }
        }
    }

    fn open_sort_menu(&mut self) {
        let target = match (self.shown_tab(), self.col()) {
            (Tab::Albums, _) => SortTarget::Albums,
            (Tab::Artists, 0) => SortTarget::Artists,
            (Tab::Artists, _) => SortTarget::ArtistAlbums,
            _ => {
                self.toast("nothing to sort here");
                return;
            }
        };
        let state = ListState::default().with_selected(Some(self.sorts[target.index()]));
        self.menu = Some(SortMenu { target, state });
    }

    /// Apply and remember a sort, keeping the cursor on the same album or artist.
    fn set_sort(&mut self, target: SortTarget, choice: usize) {
        let (album, artist, artist_album) = (self.selected_album(), self.selected_artist(), self.artist_album());
        self.sorts[target.index()] = choice;
        library::save_setting(target.setting(), &choice.to_string());
        self.apply_sorts();
        if let Some(a) = album { self.album_state.select(self.album_order.iter().position(|&x| x == a)); }
        if let Some(a) = artist {
            self.artist_state.select(self.artist_order.iter().position(|&x| x == a));
            if let Some(al) = artist_album { self.artist_album_state.select(self.artists[a].albums.iter().position(|&x| x == al)); }
        }
    }

    fn artist_tracks(&self, artist: usize) -> Vec<Track> {
        self.artists[artist].albums.iter().flat_map(|&a| self.albums[a].tracks.clone()).collect()
    }

    /// What the cursor is on right now: the tracks it stands for, where to start,
    /// and whether it's a single song (so `a` adds just that song).
    fn selection(&self) -> Option<(Vec<Track>, usize, bool)> {
        match (self.tab, self.col()) {
            (Tab::Albums, 0) => Some((self.albums.get(self.selected_album()?)?.tracks.clone(), 0, false)),
            (Tab::Albums, _) => {
                let tracks = self.albums.get(self.selected_album()?)?.tracks.clone();
                Some((tracks, self.album_track_state.selected().unwrap_or(0), true))
            }
            (Tab::Artists, 0) => Some((self.artist_tracks(self.selected_artist()?), 0, false)),
            (Tab::Artists, 1) => Some((self.albums[self.artist_album()?].tracks.clone(), 0, false)),
            (Tab::Artists, _) => {
                let tracks = self.albums[self.artist_album()?].tracks.clone();
                Some((tracks, self.artist_track_state.selected().unwrap_or(0), true))
            }
            (Tab::Search, _) => match *self.hits.get(self.hit_state.selected()?)? {
                Hit::Artist(a) => Some((self.artist_tracks(a), 0, false)),
                Hit::Album(a) => Some((self.albums[a].tracks.clone(), 0, false)),
                Hit::Song(a, t) => Some((self.albums[a].tracks.clone(), t, true)),
            },
            (Tab::Queue, _) => None,
        }
    }

    // ---------- playback ----------

    fn open(track: &Track) -> Option<Decoder<BufReader<File>>> {
        File::open(&track.path).ok().and_then(|f| Decoder::new(BufReader::new(f)).ok())
    }

    /// Start the queue at position i (skipping files that fail to open).
    fn play_at(&mut self, mut i: usize) {
        self.sink.clear();
        while i < self.queue.len() {
            if let Some(source) = Self::open(&self.queue[i]) {
                self.sink.append(source);
                self.sink.play();
                self.current = Some(i);
                self.queue_state.select(Some(i));
                self.load_cover();
                return;
            }
            i += 1;
        }
        self.current = None;
        self.load_cover();
    }

    fn play_now(&mut self) {
        if self.tab == Tab::Queue {
            if let Some(i) = self.queue_state.selected() { self.play_at(i); }
            return;
        }
        // in search, only songs play: albums and artists open in their own tabs
        if self.tab == Tab::Search {
            match self.hit_state.selected().and_then(|i| self.hits.get(i)).copied() {
                Some(Hit::Album(a)) => {
                    self.switch_tab(Tab::Albums);
                    self.cols[Tab::Albums.index()] = 0;
                    self.album_state.select(self.album_order.iter().position(|&x| x == a));
                    self.album_track_state.select(Some(0));
                    return;
                }
                Some(Hit::Artist(a)) => {
                    self.switch_tab(Tab::Artists);
                    self.cols[Tab::Artists.index()] = 0;
                    self.artist_state.select(self.artist_order.iter().position(|&x| x == a));
                    self.artist_album_state.select(Some(0));
                    self.artist_track_state.select(Some(0));
                    return;
                }
                _ => {} 
            }
        }
        if let Some((tracks, start, _)) = self.selection() {
            self.queue = tracks;
            self.unshuffled = None; // a brand new queue: nothing to unshuffle back to
            self.play_at(start);
        }
    }

    /// `a` adds what's under the cursor (one song if it's on a song); `A` (whole = true) always adds
    /// the whole album or artist around it.
    fn add_to_queue(&mut self, whole: bool) {
        let Some((tracks, start, single)) = self.selection() else { return };
        let added: Vec<Track> = if single && !whole { vec![tracks[start].clone()] } else { tracks };
        let first_new = self.queue.len();
        let n = added.len();
        self.queue.extend(added);
        self.toast(if n == 1 { "added 1 song to the queue".into() } else { format!("added {n} songs to the queue") });
        if self.current.is_none() { self.play_at(first_new); }
    }

    fn skip(&mut self, delta: isize) {
        if let Some(i) = self.current {
            let len = self.queue.len() as isize;
            let mut next = i as isize + delta;
            if self.repeat == Repeat::All { next = next.rem_euclid(len.max(1)); } // wrap around
            if next >= 0 && next < len { self.play_at(next as usize); }
        }
    }

    fn seek(&mut self, secs: i64) {
        if self.current.is_none() { return; }
        let pos = self.sink.get_pos().as_secs() as i64 + secs;
        let _ = self.sink.try_seek(Duration::from_secs(pos.max(0) as u64));
    }

    fn set_volume(&mut self, delta: f32) {
        self.volume = (self.volume + delta).clamp(0.0, 1.5);
        self.sink.set_volume(self.volume);
    }

    // ---------- queue editing ----------

    fn queue_move(&mut self, delta: isize) {
        let Some(i) = self.queue_state.selected() else { return };
        let j = i as isize + delta;
        if j < 0 || j as usize >= self.queue.len() { return; }
        let j = j as usize;
        self.queue.swap(i, j);
        self.current = self.current.map(|c| if c == i { j } else if c == j { i } else { c });
        self.queue_state.select(Some(j));
    }

    // ---------- the : command line ----------

    fn command_key(&mut self, k: KeyEvent) {
        let Some(line) = self.command.as_mut() else { return };
        match k.code {
            KeyCode::Esc => self.command = None,
            KeyCode::Enter => {
                let line = std::mem::take(line);
                self.command = None;
                if !line.trim().is_empty() && self.history.last() != Some(&line) { self.history.push(line.clone()); }
                self.history_pos = None;
                self.run_command(&line);
            }
            KeyCode::Backspace => {
                if line.pop().is_none() { self.command = None; } // backspacing past the start closes it
            }
            KeyCode::Tab => *line = complete(line),
            KeyCode::Up if !self.history.is_empty() => {
                let pos = self.history_pos.map_or(self.history.len() - 1, |p| p.saturating_sub(1));
                self.history_pos = Some(pos);
                *line = self.history[pos].clone();
            }
            KeyCode::Down => {
                if let Some(p) = self.history_pos {
                    if p + 1 < self.history.len() {
                        self.history_pos = Some(p + 1);
                        *line = self.history[p + 1].clone();
                    } else {
                        self.history_pos = None;
                        line.clear();
                    }
                }
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => line.push(c),
            _ => {}
        }
    }

    fn run_command(&mut self, line: &str) {
        let line = line.trim();
        let (name, arg) = match line.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (line, ""),
        };
        // a unique start is enough: :vol, :shuf, :rep all work
        let matches: Vec<&str> = COMMANDS.iter().copied().filter(|c| c.starts_with(name)).collect();
        let name = match (name, matches.as_slice()) {
            ("q", _) => "quit",
            (_, [only]) => only,
            _ => name,
        };
        match name {
            "" => {}
            "quit" => self.quit = true,
            "help" => self.help = Some(Help::Commands),
            "clear" => self.clear_queue(),
            "shuffle" => self.shuffle(),
            "rescan" => {
                let root = self.root.clone();
                self.load_folder(root);
            }
            "folder" => {
                if arg.is_empty() {
                    self.toast(format!("library: {}", self.root.display()));
                } else {
                    self.load_folder(PathBuf::from(arg.trim_matches('"')));
                }
            }
            "volume" => match arg.trim_end_matches('%').parse::<f32>() {
                Ok(v) => {
                    self.volume = (v / 100.0).clamp(0.0, 1.5);
                    self.sink.set_volume(self.volume);
                    self.toast(format!("volume {}%", (self.volume * 100.0).round()));
                }
                Err(_) => self.toast("usage: :volume 0 to 150"),
            },
            "seek" => match parse_time(arg) {
                Some(t) if self.current.is_some() => { let _ = self.sink.try_seek(t); }
                Some(_) => self.toast("nothing's playing"),
                None => self.toast("usage: :seek 1:30 (or seconds)"),
            },
            "repeat" => {
                self.repeat = match arg {
                    "off" => Repeat::Off,
                    "all" => Repeat::All,
                    "one" => Repeat::One,
                    "" => match self.repeat { Repeat::Off => Repeat::All, Repeat::All => Repeat::One, Repeat::One => Repeat::Off },
                    _ => {
                        self.toast("usage: :repeat off, all, or one");
                        return;
                    }
                };
                let label = match self.repeat { Repeat::Off => "off", Repeat::All => "the whole queue", Repeat::One => "this song" };
                self.toast(format!("repeat: {label}"));
            }
            "icons" => {
                match arg {
                    "nerd" => self.nerd_icons = true,
                    "plain" => self.nerd_icons = false,
                    _ => {
                        self.toast("usage: :icons nerd or :icons plain");
                        return;
                    }
                }
                library::save_setting("icons", arg);
                self.toast(format!("{arg} icons"));
            }
            other => self.toast(format!("unknown command: {other} (try :help)")),
        }
    }

    /// Shuffle the queue (the song that's playing moves to the top and keeps playing), or, if it's
    /// already shuffled, put the original order back.
    fn shuffle(&mut self) {
        if let Some(original) = self.unshuffled.take() {
            self.unshuffle(original);
            return;
        }
        if self.queue.len() < 2 { return; }
        self.unshuffled = Some(self.queue.clone());
        let start = match self.current {
            Some(c) => { self.queue.swap(0, c); self.current = Some(0); 1 }
            None => 0,
        };
        // a small xorshift generator seeded from the clock is plenty for shuffling songs
        let mut x = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
        for i in (start + 1..self.queue.len()).rev() {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let j = start + (x % (i - start + 1) as u64) as usize;
            self.queue.swap(i, j);
        }
        self.queue_state.select(Some(0));
        let n = self.queue.len();
        self.toast(format!("shuffled {n} songs"));
    }

    /// Back to the order from before :shuffle. Songs added since go at the end, songs removed stay
    /// gone, and the current song keeps playing from its old spot.
    fn unshuffle(&mut self, original: Vec<Track>) {
        let playing = self.playing().map(|t| t.path.clone());
        let mut rest = std::mem::take(&mut self.queue);
        let mut restored = Vec::with_capacity(rest.len());
        for t in original {
            if let Some(pos) = rest.iter().position(|r| r.path == t.path) {
                restored.push(rest.remove(pos));
            }
        }
        restored.extend(rest);
        self.queue = restored;
        self.current = playing.and_then(|p| self.queue.iter().position(|t| t.path == p));
        self.queue_state.select(self.current.or(if self.queue.is_empty() { None } else { Some(0) }));
        self.toast("back to the original order");
    }

    /// Switch to (or rescan) a music folder, and remember it for next time.
    fn load_folder(&mut self, dir: PathBuf) {
        if !dir.is_dir() {
            self.toast(format!("can't find the folder {}", dir.display()));
            return;
        }
        let albums = library::scan(&dir);
        if albums.is_empty() {
            self.toast(format!("no music found in {}", dir.display()));
            return;
        }
        self.artists = build_artists(&albums);
        self.albums = albums;
        self.apply_sorts();
        for state in [&mut self.album_state, &mut self.album_track_state, &mut self.artist_state, &mut self.artist_album_state, &mut self.artist_track_state] {
            state.select(Some(0));
        }
        self.search(); // results pointed into the old library
        self.preview = None;
        self.preview_pending = None;
        library::save_setting("folder", &dir.to_string_lossy());
        let n = self.albums.len();
        self.toast(format!("{n} album{} in {}", if n == 1 { "" } else { "s" }, dir.display()));
        self.root = dir;
    }

    /// Clear the queue, keeping the song that's playing so the music doesn't cut out.
    fn clear_queue(&mut self) {
        self.unshuffled = None;
        match self.playing().cloned() {
            Some(t) => {
                self.queue = vec![t];
                self.current = Some(0);
                self.queue_state.select(Some(0));
            }
            None => {
                self.queue.clear();
                self.queue_state.select(None);
            }
        }
        self.toast("cleared the queue");
    }

    fn queue_remove(&mut self) {
        let Some(i) = self.queue_state.selected() else { return };
        if i >= self.queue.len() { return; }
        self.queue.remove(i);
        match self.current {
            Some(c) if c == i => {
                // removed the song that's playing: carry on with whatever slid into its place
                if i < self.queue.len() { self.play_at(i) } else { self.sink.clear(); self.current = None; self.load_cover(); }
            }
            Some(c) if c > i => self.current = Some(c - 1),
            _ => {}
        }
        if self.queue.is_empty() { self.queue_state.select(None) }
        else { self.queue_state.select(Some(i.min(self.queue.len() - 1))) }
    }

    // ---------- search ----------

    fn search(&mut self) {
        let q = self.query.to_lowercase();
        self.hits.clear();
        if !q.is_empty() {
            for (i, a) in self.artists.iter().enumerate() {
                if a.name.to_lowercase().contains(&q) { self.hits.push(Hit::Artist(i)); }
            }
            for (i, a) in self.albums.iter().enumerate() {
                if a.title.to_lowercase().contains(&q) { self.hits.push(Hit::Album(i)); }
            }
            for (ai, a) in self.albums.iter().enumerate() {
                for (ti, t) in a.tracks.iter().enumerate() {
                    if t.title.to_lowercase().contains(&q) { self.hits.push(Hit::Song(ai, ti)); }
                }
            }
            self.hits.truncate(300);
        }
        self.hit_state.select(if self.hits.is_empty() { None } else { Some(0) });
    }

    // ---------- cover art ----------

    fn load_cover(&mut self) {
        self.cover = self.playing().and_then(|t| library::cover(&t.path));
        self.preview = None;
        self.preview_pending = None;
        self.show_new_cover();
    }

    /// Swap in self.cover. The box stays where it is, so painting blanks it and draws over it,
    /// no full-screen clear needed.
    fn show_new_cover(&mut self) {
        self.art_cache.clear();
        self.art_area = Rect::default();
        self.art_dirty = true;
    }

    /// With nothing playing, the album the cursor is on (it gets previewed in the cover box).
    fn browsing_album(&self) -> Option<usize> {
        if self.current.is_some() { return None; }
        match self.shown_tab() {
            Tab::Albums => self.selected_album(),
            Tab::Artists => self.artist_album(),
            Tab::Search => match *self.hits.get(self.hit_state.selected()?)? {
                Hit::Album(a) | Hit::Song(a, _) => Some(a),
                Hit::Artist(a) => self.artists[a].albums.first().copied(),
            },
            Tab::Queue => None,
        }
    }

    /// Show the cover of the album under the cursor once the cursor has rested there a moment,
    /// so holding j through a long list doesn't decode every cover on the way.
    fn update_preview(&mut self) {
        if self.current.is_some() { return; }
        let wanted = self.browsing_album();
        if wanted == self.preview {
            self.preview_pending = None;
            return;
        }
        match self.preview_pending {
            Some((p, since)) if p == wanted => {
                if since.elapsed() >= Duration::from_millis(150) {
                    self.preview = wanted;
                    self.preview_pending = None;
                    self.cover = wanted.and_then(|a| self.albums[a].tracks.first()).and_then(|t| library::cover(&t.path));
                    self.show_new_cover();
                }
            }
            _ => self.preview_pending = Some((wanted, Instant::now())),
        }
    }

    /// Called after every draw. When the cover box moves or resizes (switching between the queue
    /// and the other tabs, or resizing the window), wipe the old copy and paint it in the new spot.
    /// Each size is encoded once per song, so going back and forth between tabs doesn't re-encode.
    fn update_art(&mut self) {
        let area = self.cover_area;
        if area == self.art_area { return; }
        if self.art_area.width > 0 { self.needs_clear = true; }
        self.art_area = area;
        let key = (area.width, area.height);
        if area.width > 0 && !self.art_cache.contains_key(&key) {
            if let Some(art) = self.cover.as_ref().and_then(|b| sixel::encode(b, key.0 as u32 * CELL_W, key.1 as u32 * CELL_H)) {
                self.art_cache.insert(key, art);
            }
        }
        self.art_dirty = true;
    }

    fn art(&self) -> Option<&sixel::Art> {
        self.art_cache.get(&(self.cover_area.width, self.cover_area.height))
    }

    // ---------- keys ----------

    fn move_selection(&mut self, delta: isize) {
        let (state, n): (&mut ListState, usize) = match (self.tab, self.col()) {
            (Tab::Queue, _) => (&mut self.queue_state, self.queue.len()),
            (Tab::Albums, 0) => (&mut self.album_state, self.albums.len()),
            (Tab::Albums, _) => {
                let n = self.selected_album().map_or(0, |a| self.albums[a].tracks.len());
                (&mut self.album_track_state, n)
            }
            (Tab::Artists, 0) => (&mut self.artist_state, self.artists.len()),
            (Tab::Artists, 1) => {
                let n = self.selected_artist().map_or(0, |a| self.artists[a].albums.len());
                (&mut self.artist_album_state, n)
            }
            (Tab::Artists, _) => {
                let n = self.artist_album().map_or(0, |a| self.albums[a].tracks.len());
                (&mut self.artist_track_state, n)
            }
            (Tab::Search, _) => (&mut self.hit_state, self.hits.len()),
        };
        if n == 0 { return; }
        let cur = state.selected().unwrap_or(0) as isize;
        state.select(Some((cur + delta).clamp(0, n as isize - 1) as usize));
        // moving a parent list resets the lists to its right
        match (self.tab, self.col()) {
            (Tab::Albums, 0) => self.album_track_state.select(Some(0)),
            (Tab::Artists, 0) => { self.artist_album_state.select(Some(0)); self.artist_track_state.select(Some(0)); }
            (Tab::Artists, 1) => self.artist_track_state.select(Some(0)),
            _ => {}
        }
    }

    fn switch_tab(&mut self, tab: Tab) {
        if tab == Tab::Search && self.tab != Tab::Search { self.before_search = self.tab; }
        if tab != self.tab { self.last_tab = self.tab; }
        self.tab = tab;
        // going to search starts typing straight away
        self.editing = tab == Tab::Search && self.query.is_empty();
    }

    fn key(&mut self, k: KeyEvent) {
        if self.command.is_some() {
            self.command_key(k);
            return;
        }
        let tabs = Tab::ALL.len();
        if self.editing {
            match k.code {
                // tab keys switch tabs even mid-typing, so they can cycle through all four
                KeyCode::Tab => self.switch_tab(Tab::ALL[(self.tab.index() + 1) % tabs]),
                KeyCode::BackTab => self.switch_tab(Tab::ALL[(self.tab.index() + tabs - 1) % tabs]),
                KeyCode::Esc | KeyCode::Enter | KeyCode::Down => {
                    self.editing = false;
                    // nothing typed: back to where you were
                    if self.query.is_empty() { self.switch_tab(self.before_search); }
                }
                KeyCode::Backspace => { self.query.pop(); self.search(); }
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => { self.query.push(c); self.search(); }
                _ => {}
            }
            return;
        }
        if self.help.is_some() {
            if matches!(k.code, KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q')) { self.help = None; }
            return;
        }
        if let Some(menu) = self.menu.as_mut() {
            let n = menu.target.options().len();
            let cur = menu.state.selected().unwrap_or(0);
            match k.code {
                KeyCode::Char('j') | KeyCode::Down => menu.state.select(Some((cur + 1).min(n - 1))),
                KeyCode::Char('k') | KeyCode::Up => menu.state.select(Some(cur.saturating_sub(1))),
                KeyCode::Enter => {
                    let target = menu.target;
                    self.menu = None;
                    self.set_sort(target, cur);
                }
                KeyCode::Esc | KeyCode::Char('o') | KeyCode::Char('q') => self.menu = None,
                _ => {}
            }
            return;
        }
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char(':') => self.command = Some(String::new()),
            KeyCode::Char('`') => self.switch_tab(self.last_tab),
            KeyCode::Char('?') => self.help = Some(Help::Keys),
            KeyCode::Char('o') => self.open_sort_menu(),
            KeyCode::Char(c @ '1'..='4') => self.switch_tab(Tab::ALL[c as usize - '1' as usize]),
            KeyCode::Tab => self.switch_tab(Tab::ALL[(self.tab.index() + 1) % tabs]),
            KeyCode::BackTab => self.switch_tab(Tab::ALL[(self.tab.index() + tabs - 1) % tabs]),
            KeyCode::Char('/') => { self.switch_tab(Tab::Search); self.editing = true; }
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('h') | KeyCode::Left => { let c = &mut self.cols[self.tab.index()]; *c = c.saturating_sub(1); }
            KeyCode::Char('l') | KeyCode::Right => {
                let max = self.tab.columns() - 1;
                let c = &mut self.cols[self.tab.index()];
                *c = (*c + 1).min(max);
            }
            KeyCode::Enter => self.play_now(),
            KeyCode::Char('a') => self.add_to_queue(false),
            KeyCode::Char('A') => self.add_to_queue(true),
            KeyCode::Char('D') => self.clear_queue(),
            KeyCode::Char(' ') => if self.sink.is_paused() { self.sink.play() } else { self.sink.pause() },
            KeyCode::Char('n') => self.skip(1),
            KeyCode::Char('p') => self.skip(-1),
            KeyCode::Char('.') => self.seek(5),
            KeyCode::Char(',') => self.seek(-5),
            KeyCode::Char('=') | KeyCode::Char('+') => self.set_volume(0.05),
            KeyCode::Char('-') => self.set_volume(-0.05),
            KeyCode::Char('J') if self.tab == Tab::Queue => self.queue_move(1),
            KeyCode::Char('K') if self.tab == Tab::Queue => self.queue_move(-1),
            KeyCode::Char('d') if self.tab == Tab::Queue => self.queue_remove(),
            _ => {}
        }
    }

    // ---------- background work ----------

    /// If Windows switched its default output (say, to bluetooth headphones), move playback over to it,
    /// picking the song up at the same spot.
    fn follow_default_device(&mut self) {
        if self.last_device_check.elapsed() < Duration::from_secs(2) { return; }
        self.last_device_check = Instant::now();
        let name = default_device_name();
        if name.is_empty() || name == self.device_name { return; }
        let Ok((stream, handle)) = OutputStream::try_default() else { return }; // try again next check
        let Ok(sink) = Sink::try_new(&handle) else { return };
        self.device_name = name;
        sink.set_volume(self.volume);
        if let Some(source) = self.playing().and_then(Self::open) {
            sink.append(source);
            let _ = sink.try_seek(self.sink.get_pos());
            if self.sink.is_paused() { sink.pause(); }
        }
        self.sink.stop();
        self.sink = sink;
        self._stream = stream;
        let msg = format!("now playing on {}", self.device_name);
        self.toast(msg);
    }

    fn tick(&mut self) {
        self.follow_default_device();
        self.collect_stderr();
        self.update_preview();
        if let Some(i) = self.current {
            if self.sink.empty() && !self.sink.is_paused() {
                if self.repeat == Repeat::One {
                    self.play_at(i);
                } else if i + 1 < self.queue.len() {
                    self.play_at(i + 1);
                } else if self.repeat == Repeat::All {
                    self.play_at(0);
                } else {
                    self.current = None;
                    self.load_cover();
                }
            }
        }
    }
}

/// Tab completion for the command line: finish a command name (or `repeat`/`icons` options) as far
/// as it's unambiguous.
fn complete(line: &str) -> String {
    let options: Vec<&str>;
    let (head, word) = match line.split_once(' ') {
        Some((cmd, arg)) => {
            options = match cmd {
                "repeat" => vec!["off", "all", "one"],
                "icons" => vec!["nerd", "plain"],
                _ => return line.to_string(),
            };
            (format!("{cmd} "), arg)
        }
        None => {
            options = COMMANDS.to_vec();
            (String::new(), line)
        }
    };
    let matches: Vec<&str> = options.into_iter().filter(|o| o.starts_with(word)).collect();
    match matches.as_slice() {
        [] => line.to_string(),
        [only] => format!("{head}{only}{}", if head.is_empty() && matches!(*only, "volume" | "seek" | "folder" | "repeat" | "icons") { " " } else { "" }),
        many => {
            // as far as all the matches agree
            let first = many[0];
            let common = (word.len()..=first.len()).rev().find(|&n| many.iter().all(|m| m.starts_with(&first[..n]))).unwrap_or(word.len());
            format!("{head}{}", &first[..common])
        }
    }
}

/// "1:30", "90", or "1:02:03" as a duration.
fn parse_time(s: &str) -> Option<Duration> {
    let mut secs = 0u64;
    for part in s.split(':') {
        secs = secs * 60 + part.trim().parse::<u64>().ok()?;
    }
    (!s.is_empty()).then(|| Duration::from_secs(secs))
}

fn default_device_name() -> String {
    rodio::cpal::default_host()
        .default_output_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default()
}

/// Group albums under their artist, alphabetically.
fn build_artists(albums: &[Album]) -> Vec<Artist> {
    let mut artists: Vec<Artist> = Vec::new();
    for (i, a) in albums.iter().enumerate() {
        match artists.iter_mut().find(|x| x.name == a.artist) {
            Some(x) => x.albums.push(i),
            None => artists.push(Artist { name: a.artist.clone(), albums: vec![i] }),
        }
    }
    artists.sort_by_key(|a| a.name.to_lowercase());
    artists
}

fn paint_art(app: &App) -> io::Result<()> {
    let area = app.cover_area;
    if area.width == 0 || area.height == 0 { return Ok(()); }
    let mut out = io::stdout().lock();
    write!(out, "\x1b[?25l\x1b7\x1b[0m")?;
    // blank the box first (erasing characters also erases any image under them), so a cover of a
    // different shape, or no cover at all, never leaves bits of the old one behind
    for row in 0..area.height {
        write!(out, "\x1b[{};{}H\x1b[{}X", area.y + row + 1, area.x + 1, area.width)?;
    }
    if let Some(art) = app.art() {
        let dx = (area.width as u32 * CELL_W).saturating_sub(art.width) / 2 / CELL_W;
        let dy = (area.height as u32 * CELL_H).saturating_sub(art.height) / 2 / CELL_H;
        write!(out, "\x1b[{};{}H{}", area.y as u32 + dy + 1, area.x as u32 + dx + 1, art.data)?;
    }
    write!(out, "\x1b8")?;
    out.flush()
}

fn paint_toast_icons(app: &App) -> io::Result<()> {
    if app.toast_spots.is_empty() { return Ok(()); }
    let mut out = io::stdout().lock();
    write!(out, "\x1b[?25l\x1b7")?;
    for spot in &app.toast_spots {
        write!(out, "\x1b[{};{}H{}", spot.y + 1, spot.x + 1, app.toast_icon)?;
    }
    write!(out, "\x1b8")?;
    out.flush()
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    while !app.quit {
        write!(io::stdout(), "\x1b[?2026h")?;
        io::stdout().flush()?;
        terminal.draw(|f| ui::draw(f, app))?;
        app.update_art();
        // a toast icon went away or moved: it could be stranded on cells that didn't change and
        // so won't be redrawn, and only a full redraw is sure to remove an image
        if app.painted_spots.iter().any(|s| !app.toast_spots.contains(s)) {
            app.needs_clear = true;
        }
        if app.needs_clear {
            terminal.clear()?;
            terminal.draw(|f| ui::draw(f, app))?;
            app.needs_clear = false;
            app.art_dirty = true;
        }
        if app.art_dirty {
            paint_art(app)?;
            app.art_dirty = false;
        }
        // toast icons are see-through, so anything redrawn underneath would cut into them:
        // repaint them every frame while toasts are up (they're tiny)
        paint_toast_icons(app)?;
        app.painted_spots = app.toast_spots.clone();
        write!(io::stdout(), "\x1b[?2026l")?;
        io::stdout().flush()?;
        // (every 200ms otherwise, which also keeps a long title's ribbon moving)
        let wait = if app.preview_pending.is_some() { 50 } else { 200 };
        if event::poll(Duration::from_millis(wait))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.key(k),
                Event::Resize(..) | Event::FocusGained => app.art_dirty = true,
                _ => {}
            }
        }
        app.tick();
    }
    Ok(())
}

fn music_dir() -> PathBuf {
    if let Some(arg) = std::env::args().nth(1) {
        return PathBuf::from(arg);
    }
    // the folder last opened with :folder, if it's still there
    if let Some(saved) = library::setting("folder").map(PathBuf::from).filter(|p| p.is_dir()) {
        return saved;
    }
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Music")
}

fn main() -> io::Result<()> {
    let dir = music_dir();
    if !dir.is_dir() {
        println!("can't find the folder {}", dir.display());
        return Ok(());
    }
    println!("scanning {} ...", dir.display());
    let albums = library::scan(&dir);
    if albums.is_empty() {
        println!("no music found. usage: berry [music folder]");
        return Ok(());
    }
    let artists = build_artists(&albums);

    let (stream, handle) = OutputStream::try_default().map_err(io::Error::other)?;
    let sink = Sink::try_new(&handle).map_err(io::Error::other)?;
    let first = || ListState::default().with_selected(Some(0));
    let mut app = App {
        albums,
        artists,
        tab: Tab::Albums,
        before_search: Tab::Albums,
        last_tab: Tab::Albums,
        cols: [0; 4],
        sorts: [0; 3],
        album_order: Vec::new(),
        artist_order: Vec::new(),
        menu: None,
        sort_anchors: [Rect::default(); 3],
        marquee_key: (0, usize::MAX),
        marquee_since: Instant::now(),
        queue: Vec::new(),
        current: None,
        queue_state: ListState::default(),
        album_state: first(),
        album_track_state: first(),
        artist_state: first(),
        artist_album_state: first(),
        artist_track_state: first(),
        query: String::new(),
        editing: false,
        hits: Vec::new(),
        hit_state: ListState::default(),
        help: None,
        _stream: stream,
        sink,
        device_name: default_device_name(),
        last_device_check: Instant::now(),
        volume: 1.0,
        repeat: Repeat::Off,
        unshuffled: None,
        root: dir.clone(),
        command: None,
        history: Vec::new(),
        history_pos: None,
        nerd_icons: library::setting("icons").map_or_else(
            || std::env::var("BERRY_ICONS").is_ok_and(|v| v.eq_ignore_ascii_case("nerd")),
            |v| v == "nerd",
        ),
        cover_area: Rect::default(),
        cover: None,
        art_cache: HashMap::new(),
        art_area: Rect::default(),
        preview: None,
        preview_pending: None,
        art_dirty: false,
        needs_clear: false,
        toasts: Vec::new(),
        toast_spots: Vec::new(),
        painted_spots: Vec::new(),
        toast_icon: icon::sixel(icon::terminal_text_colour().unwrap_or(icon::FALLBACK_INK), 4, 14),
        stderr: if std::env::var_os("BERRY_DEBUG").is_some() { None } else { gag::BufferRedirect::stderr().ok() },
        quit: false,
    };

    app.load_sorts();
    app.apply_sorts();

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}
