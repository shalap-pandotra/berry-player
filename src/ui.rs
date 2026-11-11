use crate::{App, Help, Hit, Repeat, SortTarget, Tab, COMMANDS};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap},
    Frame,
};
use std::time::{Duration, Instant};

fn fmt(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

fn bar(ratio: f64, width: u16) -> String {
    let filled = width as f64 * ratio.clamp(0.0, 1.0);
    let full = filled.floor() as usize;
    let mut s = "█".repeat(full);
    if full < width as usize {
        const TIPS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
        s.push_str(TIPS[((filled - full as f64) * 8.0) as usize]);
    }
    s
}

fn dim() -> Style { Style::new().add_modifier(Modifier::DIM) }
fn bold() -> Style { Style::new().add_modifier(Modifier::BOLD) }

/// A bordered list. The focused one highlights its row in reverse; unfocused ones just embolden it.
fn list(f: &mut Frame, area: Rect, title: &str, items: Vec<ListItem<'static>>, state: &mut ListState, focused: bool) {
    let highlight = if focused { Style::new().add_modifier(Modifier::REVERSED) } else { bold() };
    let title_style = if focused { bold() } else { dim() };
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(format!(" {title} ")).title_style(title_style))
            .highlight_style(highlight),
        area,
        state,
    );
}

pub fn draw(f: &mut Frame, app: &mut App) {
    draw_screen(f, app);
    draw_command_line(f, app);
}

/// The `:` prompt, along the bottom edge while it's open, with the commands that match so far.
fn draw_command_line(f: &mut Frame, app: &App) {
    let Some(line) = &app.command else { return };
    let area = f.area();
    let bar = Rect { x: area.x, y: area.bottom().saturating_sub(1), width: area.width, height: 1 };
    let mut spans = vec![Span::styled(":", bold()), Span::raw(line.clone()), Span::styled("▏", bold())];
    if !line.contains(' ') {
        let hints: Vec<&str> = COMMANDS.iter().copied().filter(|c| c.starts_with(line.as_str())).collect();
        if !hints.is_empty() && hints.len() < COMMANDS.len() {
            spans.push(Span::styled(format!("   {}", hints.join("  ")), dim()));
        }
    }
    f.render_widget(Clear, bar);
    f.render_widget(Paragraph::new(Line::from(spans)), bar);
}

fn draw_screen(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // YouTube mode's search view: same shape as the browsing tabs, with its own list
    #[cfg(feature = "youtube")]
    if app.yt.active && app.yt.on_search {
        let [tabs, body, bar] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(BAR_ROWS + 2)]).areas(area);
        draw_tabs(f, app, tabs);
        crate::youtube::draw(f, app, body);
        draw_now_bar(f, app, bar);
        draw_toasts(f, app, body);
        if let Some(which) = app.help { draw_help(f, body, which); }
        return;
    }
    let area = f.area();
    if app.shown_tab() == Tab::Queue {
        // home
        let right_w = ((area.width as u32 * 2 / 5) as u16).clamp(28, 64).min(area.width.saturating_sub(30));
        let [left, right] = Layout::horizontal([Constraint::Min(30), Constraint::Length(right_w)]).areas(area);
        let [tabs, _, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(3)]).areas(left);
        draw_tabs(f, app, tabs);
        draw_queue(f, app, body);
        draw_now_playing(f, app, right);
        draw_toasts(f, app, body);
        if let Some(which) = app.help { draw_help(f, body, which); }
        draw_sort_menu(f, app, body);
    } else {
        // browsing: the lists get the full width, with a compact now playing bar along the bottom
        let [tabs, body, bar] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(BAR_ROWS + 2)]).areas(area);
        draw_tabs(f, app, tabs);
        match app.shown_tab() {
            Tab::Albums => draw_albums(f, app, body),
            Tab::Artists => draw_artists(f, app, body),
            _ => draw_search(f, app, body),
        }
        draw_now_bar(f, app, bar);
        draw_toasts(f, app, body);
        if let Some(which) = app.help { draw_help(f, body, which); }
        draw_sort_menu(f, app, body);
    }
}

/// Height of the small cover in the bottom bar, in rows (it's twice as many columns wide, so it's square).
const BAR_ROWS: u16 = 10;

fn draw_now_bar(f: &mut Frame, app: &mut App, area: Rect) {
    let [cover, info] = Layout::horizontal([Constraint::Length(BAR_ROWS * 2 + 2), Constraint::Min(20)]).areas(area);
    let cover_block = Block::bordered().border_style(dim());
    app.cover_area = cover_block.inner(cover);
    f.render_widget(cover_block, cover);

    let block = Block::bordered().title(" now playing ").padding(Padding::horizontal(1));
    let inner = block.inner(info);
    f.render_widget(block, info);
    draw_track_info(f, app, inner);
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    #[cfg(feature = "youtube")]
    if app.yt.active {
        f.render_widget(Paragraph::new(crate::youtube::tab_line(app)), area);
        return;
    }
    let mut spans = vec![Span::raw(" ")];
    for (i, t) in Tab::ALL.iter().enumerate() {
        let label = format!(" [{}] {} ", i + 1, t.name());
        spans.push(if *t == app.tab {
            Span::styled(label, Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD))
        } else {
                Span::styled(label, dim())
            });
        spans.push(Span::raw(" "));
    }
    if app.tab == Tab::Search || !app.query.is_empty() {
        spans.push(Span::styled(" / ", dim()));
        spans.push(Span::raw(app.query.clone()));
        if app.editing { spans.push(Span::styled("▏", bold())); }
    }
    spans.push(Span::styled("    ? help", dim()));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A fixed-width column on the right of a list row (an artist, a duration). Longer text ends in "…".
struct Col {
    text: String,
    style: Style,
    width: usize,
    right: bool, // right-aligned (numbers) or left-aligned (names)
}

fn col(text: String, style: Style, width: usize, right: bool) -> Col {
    Col { text, style, width, right }
}

/// Width for a column of names: as wide as the longest, but never more than a third of the row.
fn names_width<'a>(names: impl Iterator<Item = &'a str>, row_width: u16) -> usize {
    names.map(|n| n.chars().count()).max().unwrap_or(0).min(row_width as usize / 3)
}

/// One list row. `prefix` (a track number, the ♪ marker) stays put; `left` scrolls like a ribbon
/// (on the highlighted row) or ends in "…" (on the others) when it doesn't fit; `cols` are
/// fixed-width columns lined up on the right, like the artist and a song's length.
fn row(prefix: Option<(String, Style)>, left: Vec<(String, Style)>, cols: Vec<Col>, width: u16, scroll: Option<usize>) -> Line<'static> {
    let width = width as usize;
    let prefix_w = prefix.as_ref().map_or(0, |(t, _)| t.chars().count());
    let cols_w: usize = cols.iter().map(|c| c.width + 2).sum();
    let room = width.saturating_sub(cols_w + prefix_w);
    let chars: Vec<(char, Style)> = left.iter().flat_map(|(t, st)| t.chars().map(move |c| (c, *st))).collect();
    let blank = (' ', Style::new());
    let shown: Vec<(char, Style)> = if chars.len() <= room {
        let mut v = chars;
        v.resize(room, blank);
        v
    } else if let Some(ticks) = scroll {
        slide(&chars, room, ticks, blank)
    } else {
        let mut v: Vec<(char, Style)> = chars[..room.saturating_sub(1)].to_vec();
        if room > 0 { v.push(('…', chars[room - 1].1)); }
        v
    };
    // regroup into spans of the same style
    let mut spans: Vec<Span<'static>> = Vec::new();
    if let Some((t, st)) = prefix { spans.push(Span::styled(t, st)); }
    let mut text = String::new();
    let mut style = shown.first().map_or(Style::new(), |c| c.1);
    for (c, st) in shown {
        if st != style {
            spans.push(Span::styled(std::mem::take(&mut text), style));
            style = st;
        }
        text.push(c);
    }
    spans.push(Span::styled(text, style));
    for c in cols {
        let n = c.text.chars().count();
        let fitted = if n > c.width {
            let chars: Vec<char> = c.text.chars().collect();
            match scroll {
                // the highlighted row scrolls long names too, in step with its title
                Some(ticks) => slide(&chars, c.width, ticks, ' ').into_iter().collect(),
                None => {
                    let mut t: String = chars[..c.width.saturating_sub(1)].iter().collect();
                    if c.width > 0 { t.push('…'); }
                    t
                }
            }
        } else if c.right {
            format!("{}{}", " ".repeat(c.width - n), c.text)
        } else {
            format!("{}{}", c.text, " ".repeat(c.width - n))
        };
        spans.push(Span::raw("  "));
        spans.push(Span::styled(fitted, c.style));
    }
    Line::from(spans)
}

/// The ribbon: the text, a gap, then the text again, sliding left through a `room`-wide window,
/// resting a moment at the start of each pass.
fn slide<T: Copy>(items: &[T], room: usize, ticks: usize, blank: T) -> Vec<T> {
    const GAP: usize = 4;
    const REST: usize = 6;
    let cycle = items.len() + GAP;
    let offset = (ticks % (cycle + REST)).saturating_sub(REST);
    (0..room).map(|i| items.get((offset + i) % cycle).copied().unwrap_or(blank)).collect()
}

/// How far the highlighted row's ribbon has scrolled
fn ticks(app: &mut App, list: u8, selected: Option<usize>) -> usize {
    let key = (list, selected.unwrap_or(usize::MAX));
    if app.marquee_key != key {
        app.marquee_key = key;
        app.marquee_since = Instant::now();
    }
    (app.marquee_since.elapsed().as_millis() / 200) as usize
}

/// The scroll position for row `i` of a list: only the highlighted row of the focused list moves.
fn scroll_for(i: usize, selected: Option<usize>, ticks: Option<usize>) -> Option<usize> {
    if Some(i) == selected { ticks } else { None }
}

fn draw_queue(f: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.saturating_sub(2);
    let sel = app.queue_state.selected();
    let t = Some(ticks(app, 0, sel));
    let artist_w = names_width(app.queue.iter().map(|tr| tr.artist.as_str()), width);
    let dur_w = app.queue.iter().map(|tr| fmt(tr.duration).len()).max().unwrap_or(0);
    let items: Vec<ListItem> = app
        .queue
        .iter()
        .enumerate()
        .map(|(i, tr)| {
            let mark = if Some(i) == app.current { "♪ " } else { "  " };
            ListItem::new(row(
                Some((mark.to_string(), Style::new())),
                vec![(tr.title.clone(), Style::new())],
                vec![col(tr.artist.clone(), dim(), artist_w, false), col(fmt(tr.duration), dim(), dur_w, true)],
                width,
                scroll_for(i, sel, t),
            ))
        })
        .collect();
    let title = if app.queue.is_empty() {
        "queue · empty, press enter or a on something".to_string()
    } else {
        format!("queue · {} songs · J/K move · d remove", app.queue.len())
    };
    list(f, area, &title, items, &mut app.queue_state, true);
}

fn album_items(app: &App, indices: &[usize], with_artist: bool, width: u16, sel: Option<usize>, t: Option<usize>) -> Vec<ListItem<'static>> {
    let artist_w = if with_artist { names_width(indices.iter().map(|&i| app.albums[i].artist.as_str()), width) } else { 0 };
    indices
        .iter()
        .enumerate()
        .map(|(row_i, &i)| {
            let a = &app.albums[i];
            let cols = if with_artist { vec![col(a.artist.clone(), dim(), artist_w, false)] } else { vec![] };
            ListItem::new(row(None, vec![(a.title.clone(), Style::new())], cols, width, scroll_for(row_i, sel, t)))
        })
        .collect()
}

fn track_items(app: &App, album: Option<usize>, width: u16, sel: Option<usize>, t: Option<usize>) -> Vec<ListItem<'static>> {
    let playing = app.playing().map(|tr| tr.path.clone());
    album
        .map(|a| {
            let dur_w = app.albums[a].tracks.iter().map(|tr| fmt(tr.duration).len()).max().unwrap_or(0);
            app.albums[a]
                .tracks
                .iter()
                .enumerate()
                .map(|(i, tr)| {
                    let mark = if Some(&tr.path) == playing.as_ref() { "♪ " } else { "  " };
                    // untagged songs have no track number: leave the space blank rather than show 0
                    let number = if tr.number > 0 { format!("{:>2}", tr.number) } else { "  ".to_string() };
                    ListItem::new(row(
                        Some((format!("{mark}{number}  "), Style::new())),
                        vec![(tr.title.clone(), Style::new())],
                        vec![col(fmt(tr.duration), dim(), dur_w, true)],
                        width,
                        scroll_for(i, sel, t),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn draw_albums(f: &mut Frame, app: &mut App, area: Rect) {
    let [l, r] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area);
    let col = app.col();
    let sel = app.album_state.selected();
    let t = (col == 0).then(|| ticks(app, 1, sel));
    let order = app.album_order.clone();
    let albums = album_items(app, &order, true, l.width.saturating_sub(2), sel, t);
    let title = format!("albums · {}", sort_label(app, SortTarget::Albums));
    app.sort_anchors[SortTarget::Albums as usize] = l;
    list(f, l, &title, albums, &mut app.album_state, col == 0);
    let sel = app.album_track_state.selected();
    let t = (col == 1).then(|| ticks(app, 2, sel));
    let tracks = track_items(app, app.selected_album(), r.width.saturating_sub(2), sel, t);
    list(f, r, "tracks", tracks, &mut app.album_track_state, col == 1);
}

fn draw_artists(f: &mut Frame, app: &mut App, area: Rect) {
    let [a, b, c] = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(35), Constraint::Percentage(35)])
        .areas(area);
    let col = app.col();
    let sel = app.artist_state.selected();
    let t = (col == 0).then(|| ticks(app, 3, sel));
    let width = a.width.saturating_sub(2);
    let artists: Vec<ListItem> = app
        .artist_order
        .iter()
        .enumerate()
        .map(|(row_i, &i)| ListItem::new(row(None, vec![(app.artists[i].name.clone(), Style::new())], vec![], width, scroll_for(row_i, sel, t))))
        .collect();
    let title = format!("artists · {}", sort_label(app, SortTarget::Artists));
    app.sort_anchors[SortTarget::Artists as usize] = a;
    list(f, a, &title, artists, &mut app.artist_state, col == 0);
    let sel = app.artist_album_state.selected();
    let t = (col == 1).then(|| ticks(app, 4, sel));
    let albums = app
        .selected_artist()
        .map(|s| album_items(app, &app.artists[s].albums.clone(), false, b.width.saturating_sub(2), sel, t))
        .unwrap_or_default();
    let title = format!("albums · {}", sort_label(app, SortTarget::ArtistAlbums));
    app.sort_anchors[SortTarget::ArtistAlbums as usize] = b;
    list(f, b, &title, albums, &mut app.artist_album_state, col == 1);
    let sel = app.artist_track_state.selected();
    let t = (col == 2).then(|| ticks(app, 5, sel));
    let tracks = track_items(app, app.artist_album(), c.width.saturating_sub(2), sel, t);
    list(f, c, "tracks", tracks, &mut app.artist_track_state, col == 2);
}

fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    if app.query.is_empty() {
        let hint = Paragraph::new("type to search artists, albums and songs.\nenter plays a song or opens an album or artist; a adds to the queue.")
            .style(dim())
            .block(Block::bordered().title(" search ").padding(Padding::uniform(1)));
        f.render_widget(hint, area);
        return;
    }
    let width = area.width.saturating_sub(2);
    let sel = app.hit_state.selected();
    let t = (!app.editing).then(|| ticks(app, 6, sel));
    let items: Vec<ListItem> = app
        .hits
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let (kind, main, extra) = match *h {
                Hit::Artist(a) => ("artist", app.artists[a].name.clone(), String::new()),
                Hit::Album(a) => ("album ", app.albums[a].title.clone(), app.albums[a].artist.clone()),
                Hit::Song(a, s) => {
                    let tr = &app.albums[a].tracks[s];
                    ("song  ", tr.title.clone(), format!("{} · {}", tr.artist, tr.album))
                }
            };
            ListItem::new(row(
                Some((format!("{kind}  "), dim())),
                vec![(main, Style::new()), (format!("  {extra}"), dim())],
                vec![],
                width,
                scroll_for(i, sel, t),
            ))
        })
        .collect();
    let title = format!("search · {} results", app.hits.len());
    list(f, area, &title, items, &mut app.hit_state, !app.editing);
}

fn draw_now_playing(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered().title(" now playing ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    // a square cover: cells are twice as tall as they are wide, so two columns per row
    const INFO_ROWS: u16 = 8;
    let rows = ((inner.width.saturating_sub(4)) / 2).min(inner.height.saturating_sub(INFO_ROWS + 3));
    let cols = rows * 2;
    let frame = Rect {
        x: inner.x + inner.width.saturating_sub(cols + 2) / 2,
        y: inner.y + 1,
        width: cols + 2,
        height: rows + 2,
    };
    if rows >= 4 {
        let cover_block = Block::bordered().border_style(dim());
        app.cover_area = cover_block.inner(frame);
        f.render_widget(cover_block, frame);
    } else {
        app.cover_area = Rect::default();
    }

    let info_top = if rows >= 4 { frame.bottom() + 1 } else { inner.y + 1 };
    let info = Rect {
        x: inner.x + 2,
        y: info_top,
        width: inner.width.saturating_sub(4),
        height: inner.bottom().saturating_sub(info_top),
    };
    draw_track_info(f, app, info);
}

/// Title, artist, album, progress, and controls: shared by the big column and the bottom bar.
fn draw_track_info(f: &mut Frame, app: &App, info: Rect) {
    let [text, gauge, time, controls] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
        .areas(info);

    match app.playing() {
        Some(t) => {
            let pos = app.sink.get_pos();
            let ratio = if t.duration.is_zero() { 0.0 } else { (pos.as_secs_f64() / t.duration.as_secs_f64()).min(1.0) };
            f.render_widget(
                Paragraph::new(vec![
                    Line::styled(t.title.clone(), bold()),
                    Line::raw(t.artist.clone()),
                    Line::styled(t.album.clone(), dim()),
                ])
                    .wrap(Wrap { trim: true }),
                text,
            );
            f.render_widget(Paragraph::new(bar(ratio, gauge.width)), gauge);
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::raw(fmt(pos)),
                    Span::styled(format!(" / {}", fmt(t.duration)), dim()),
                ])),
                time,
            );
        }
        None => f.render_widget(Paragraph::new("nothing playing").style(dim()), text),
    }

    // controls with their keys underneath: plain symbols every font has,
    // or Nerd Font icons when BERRY_ICONS=nerd
    let playing = app.current.is_some() && !app.sink.is_paused();
    let vol = (app.volume * 100.0).round();
    let (icons, keys) = if app.nerd_icons {
        let play = if playing { "\u{f03e4}" } else { "\u{f040a}" };
        (format!("\u{f04ae}   {play}   \u{f04ad}      vol {vol}%"), "p   spc   n      - =")
    } else {
        let play = if playing { "‖" } else { "▶" };
        (format!("|◀   {play}   ▶|     vol {vol}%"), "p   spc  n      - =")
    };
    let icons = match app.repeat {
        Repeat::Off => icons,
        Repeat::All => format!("{icons}   repeat all"),
        Repeat::One => format!("{icons}   repeat one"),
    };
    let icons = if app.unshuffled.is_some() { format!("{icons}   shuffled") } else { icons };
    f.render_widget(Paragraph::new(vec![Line::raw(icons), Line::styled(keys, dim())]), controls);
}

fn draw_toasts(f: &mut Frame, app: &mut App, area: Rect) {
    app.toast_spots.clear();
    let width = (area.width / 2).clamp(24, 48).min(area.width.saturating_sub(2));
    let mut y = area.y + 2;
    for t in app.toasts.iter().rev().take(4) {
        let text_w = width.saturating_sub(4).max(1) as usize; // borders and padding
        let lines = t.text.chars().count().div_ceil(text_w).max(1) as u16;
        let height = lines + 3; // borders, plus a blank row for the bottom of the icon
        if height > area.bottom().saturating_sub(y) { break; }
        let rect = Rect { x: area.right().saturating_sub(width + 1), y, width, height };
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(t.text.as_str())
                .wrap(Wrap { trim: true })
                // the title is one bit of border then a 4-cell gap, right where the icon goes
                .block(Block::bordered().title("─    ").padding(Padding::new(1, 1, 1, 0))),
            rect,
        );
        // the icon's box: 4 cells wide, from the row above the toast down into its first row
        app.toast_spots.push(Rect { x: rect.x + 2, y: rect.y - 1, width: 4, height: 3 });
        y += height + 1;
    }
}

fn sort_label(app: &App, target: SortTarget) -> &'static str {
    target.options()[app.sorts[target as usize]]
}

/// The `o` menu: the sort options for the list under the cursor, the current one marked.
fn draw_sort_menu(f: &mut Frame, app: &mut App, area: Rect) {
    let current = match &app.menu { Some(m) => app.sorts[m.target as usize], None => return };
    let Some(menu) = app.menu.as_mut() else { return };
    let items: Vec<ListItem> = menu
        .target
        .options()
        .iter()
        .enumerate()
        .map(|(i, o)| ListItem::new(format!("{} {o}", if i == current { "●" } else { " " })))
        .collect();
    // tucked into the top-right corner of the list it sorts
    let anchor = app.sort_anchors[menu.target as usize];
    let w = 30.min(area.width.saturating_sub(2));
    let h = (items.len() as u16 + 2).min(area.height);
    let x = anchor.right().saturating_sub(w + 1).max(anchor.x + 1);
    let y = (anchor.y + 1).min(area.bottom().saturating_sub(h));
    let rect = Rect { x, y, width: w, height: h };
    f.render_widget(Clear, rect);
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(format!(" sort {} ", menu.target.noun())).title_style(bold()))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        rect,
        &mut menu.state,
    );
}

fn draw_help(f: &mut Frame, area: Rect, which: Help) {
    let lines = |rows: &[(&str, &str)], key_w: usize| -> Vec<Line<'static>> {
        rows.iter()
            .map(|(k, v)| {
                if k.is_empty() {
                    Line::styled(v.to_string(), bold())
                } else {
                    Line::from(vec![Span::raw(format!("{k:<key_w$}")), Span::styled(v.to_string(), dim())])
                }
            })
            .collect()
    };
    // a centred box of the given size, cleared, with a title; returns the space inside it
    let mut panel = |w: u16, h: u16, title: &str| -> Rect {
        let w = w.min(area.width.saturating_sub(2));
        let h = h.min(area.height);
        let rect = Rect { x: area.x + (area.width.saturating_sub(w)) / 2, y: area.y + (area.height.saturating_sub(h)) / 2, width: w, height: h };
        f.render_widget(Clear, rect);
        let block = Block::bordered().title(format!(" {title} ")).padding(Padding::horizontal(1));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        inner
    };
    match which {
        Help::Keys => {
            let left: &[(&str, &str)] = &[
                ("", "playback"),
                ("enter", "play now"),
                ("a", "add to queue"),
                ("A", "add the whole album / artist"),
                ("space", "pause / resume"),
                ("n  p", "next / previous"),
                (",  .", "seek back / forward 5s"),
                ("-  =", "volume down / up"),
                ("", ""),
                ("", "queue"),
                ("J  K", "move song down / up"),
                ("d", "remove song"),
                ("D", "clear queue (any tab)"),
            ];
            let right: &[(&str, &str)] = &[
                ("", "moving around"),
                ("1-4", "jump to a tab"),
                ("tab", "next tab (shift+tab back)"),
                ("`", "back to the last tab"),
                ("j  k", "down / up"),
                ("h  l", "left / right column"),
                ("/", "search"),
                ("o", "sort this list"),
                (":", "command line (:help lists them)"),
                ("", ""),
                ("q", "quit"),
                ("? esc", "close this"),
            ];
            let inner = panel(84, left.len().max(right.len()) as u16 + 2, "keys");
            let [l, r] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(inner);
            f.render_widget(Paragraph::new(lines(left, 7)), l);
            f.render_widget(Paragraph::new(lines(right, 7)), r);
        }
        Help::Commands => {
            let rows: &[(&str, &str)] = &[
                ("", "type : then a command. tab completes, up / down for earlier ones"),
                ("", ""),
                (":volume 60", "set the volume to 60, from 0 to 150"),
                (":seek 1:30", "jump to 1:30 in the song"),
                (":shuffle", "shuffle the queue; again to put it back"),
                (":repeat", "cycle off / all / one, or :repeat all"),
                (":clear", "clear the queue"),
                (":folder D:\\Music", "switch to another music folder (remembered)"),
                (":rescan", "reread the current folder"),
                (":icons nerd", "Nerd Font control icons (:icons plain to undo)"),
                (":help", "this list"),
                (":q", "quit"),
                ("", ""),
                ("", "a unique start works too: :vol 60, :shuf, :rep one"),
            ];
            let inner = panel(76, rows.len() as u16 + 2, "commands");
            f.render_widget(Paragraph::new(lines(rows, 19)), inner);
        }
    }
}
