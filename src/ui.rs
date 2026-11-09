use crate::{App, Hit, SortTarget, Tab};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap},
    Frame,
};
use std::time::Duration;

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
        if app.help { draw_help(f, body); }
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
        if app.help { draw_help(f, body); }
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

fn draw_queue(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .queue
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mark = if Some(i) == app.current { "♪ " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::raw(format!("{mark}{}", t.title)),
                Span::styled(format!("  {}  {}", t.artist, fmt(t.duration)), dim()),
            ]))
        })
        .collect();
    let title = if app.queue.is_empty() {
        "queue · empty, press enter or a on something".to_string()
    } else {
        format!("queue · {} songs · J/K move · d remove", app.queue.len())
    };
    list(f, area, &title, items, &mut app.queue_state, true);
}

fn album_items(app: &App, indices: impl Iterator<Item = usize>, with_artist: bool) -> Vec<ListItem<'static>> {
    indices
        .map(|i| {
            let a = &app.albums[i];
            let mut spans = vec![Span::raw(a.title.clone())];
            if with_artist { spans.push(Span::styled(format!("  {}", a.artist), dim())); }
            ListItem::new(Line::from(spans))
        })
        .collect()
}

fn track_items(app: &App, album: Option<usize>) -> Vec<ListItem<'static>> {
    let playing = app.playing().map(|t| t.path.clone());
    album
        .map(|a| {
            app.albums[a]
                .tracks
                .iter()
                .map(|t| {
                    let mark = if Some(&t.path) == playing.as_ref() { "♪ " } else { "  " };
                    ListItem::new(Line::from(vec![
                        Span::raw(format!(
                            "{mark}{}  {}",
                            if t.number > 0 { format!("{:>2}", t.number) } else { "  ".to_string() },
                            t.title
                        )),
                        Span::styled(format!("  {}", fmt(t.duration)), dim()),
                    ]))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn draw_albums(f: &mut Frame, app: &mut App, area: Rect) {
    let [l, r] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area);
    let col = app.col();
    let albums = album_items(app, app.album_order.iter().copied(), true);
    let title = format!("albums · {}", sort_label(app, SortTarget::Albums));
    app.sort_anchors[SortTarget::Albums as usize] = l;
    list(f, l, &title, albums, &mut app.album_state, col == 0);
    let tracks = track_items(app, app.selected_album());
    list(f, r, "tracks", tracks, &mut app.album_track_state, col == 1);
}

fn draw_artists(f: &mut Frame, app: &mut App, area: Rect) {
    let [a, b, c] = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(35), Constraint::Percentage(35)])
        .areas(area);
    let col = app.col();
    let artists: Vec<ListItem> = app.artist_order.iter().map(|&i| ListItem::new(app.artists[i].name.clone())).collect();
    let title = format!("artists · {}", sort_label(app, SortTarget::Artists));
    app.sort_anchors[SortTarget::Artists as usize] = a;
    list(f, a, &title, artists, &mut app.artist_state, col == 0);
    let albums = app
        .selected_artist()
        .map(|s| album_items(app, app.artists[s].albums.clone().into_iter(), false))
        .unwrap_or_default();
    let title = format!("albums · {}", sort_label(app, SortTarget::ArtistAlbums));
    app.sort_anchors[SortTarget::ArtistAlbums as usize] = b;
    list(f, b, &title, albums, &mut app.artist_album_state, col == 1);
    let tracks = track_items(app, app.artist_album());
    list(f, c, "tracks", tracks, &mut app.artist_track_state, col == 2);
}

fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    if app.query.is_empty() {
        let hint = Paragraph::new("type to search artists, albums and songs.\nenter plays, a adds to the queue.")
            .style(dim())
            .block(Block::bordered().title(" search ").padding(Padding::uniform(1)));
        f.render_widget(hint, area);
        return;
    }
    let items: Vec<ListItem> = app
        .hits
        .iter()
        .map(|h| {
            let (kind, main, extra) = match *h {
                Hit::Artist(a) => ("artist", app.artists[a].name.clone(), String::new()),
                Hit::Album(a) => ("album ", app.albums[a].title.clone(), app.albums[a].artist.clone()),
                Hit::Song(a, t) => {
                    let tr = &app.albums[a].tracks[t];
                    ("song  ", tr.title.clone(), format!("{} · {}", tr.artist, tr.album))
                }
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{kind}  "), dim()),
                Span::raw(main),
                Span::styled(format!("  {extra}"), dim()),
            ]))
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

fn draw_help(f: &mut Frame, area: Rect) {
    // two columns, so it fits even in the short browsing layout
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
        ("", ""),
        ("q", "quit"),
        ("? esc", "close this"),
    ];
    let lines = |rows: &[(&str, &str)]| -> Vec<Line<'static>> {
        rows.iter()
            .map(|(k, v)| {
                if k.is_empty() {
                    Line::styled(v.to_string(), bold())
                } else {
                    Line::from(vec![Span::raw(format!("{k:<7}")), Span::styled(v.to_string(), dim())])
                }
            })
            .collect()
    };
    let w = 80.min(area.width.saturating_sub(2));
    let h = (left.len().max(right.len()) as u16 + 2).min(area.height);
    let rect = Rect { x: area.x + (area.width.saturating_sub(w)) / 2, y: area.y + (area.height.saturating_sub(h)) / 2, width: w, height: h };
    f.render_widget(Clear, rect);
    let block = Block::bordered().title(" keys ").padding(Padding::horizontal(1));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [l, r] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(inner);
    f.render_widget(Paragraph::new(lines(left)), l);
    f.render_widget(Paragraph::new(lines(right)), r);
}
