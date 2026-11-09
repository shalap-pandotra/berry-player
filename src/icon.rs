//! berry's icon: a 32x32 pixel-art strawberry wearing headphones, drawn as Sixel for the toasts.

const GRID: [&str; 32] = [
    "................................",
    "................................",
    "...........KKKKKKKKKK...........",
    ".........KKKKKKKKKKKKKK.........",
    ".......KKKKKKKKKKKKKKKKKK.......",
    "......KKKKK..........KKKKK......",
    ".....KKKK..............KKKK.....",
    "....KKK..................KKK....",
    "....KKK..................KKK....",
    "...KKK....................KKK...",
    "...KKK...g............g...KKK...",
    "...KKK...gg...gggg...gg...KKK...",
    "...KKK....gggggggggggg....KKK...",
    "..KKKKK..gggggggggggggg..KKKKK..",
    ".KKKKKKKRRRgRRRRRRRRgRRRRKKKKKK.",
    ".KKKKKKRRRRRRRRRRRRRRRRRRKKKKKK.",
    ".KKKKKKRRRRRRRRRRRRRRRRRRKKKKKK.",
    ".KKKKKKRRRRRRRRRRRRRRRRRRKKKKKK.",
    ".KKKKKKRRRRRRRRRRRRRRRRRRKKKKKK.",
    ".KKKKKKRRRRRRRRRRRRRRRRRRKKKKKK.",
    ".KKKKKKRRRRRRRRRRRRRRRRRRKKKKKK.",
    ".KKKKKKKRRRRRRRRRRRRRRRRKKKKKKK.",
    "..KKKKK.RRRRRRRRRRRRRRRR.KKKKK..",
    "........RRRRRRRRRRRRRRRR........",
    ".........RRRRRRRRRRRRRR.........",
    ".........RRRRRRRRRRRRRR.........",
    "..........RRRRRRRRRRRR..........",
    "...........RRRRRRRRRR...........",
    "............RRRRRRRR............",
    ".............RRRRRRR............",
    "...............RR...............",
    "................................",
];

/// Purple headphones, used when the terminal won't say what its text colour is.
pub const FALLBACK_INK: (u8, u8, u8) = (87, 82, 121);

/// The icon as Sixel, with the headphones in `ink` so they stand out on any theme, shifted
/// `left` and `top` pixels into a transparent canvas (images can only start at a cell's top-left
/// corner, so this is how the icon gets placed between cells).
pub fn sixel(ink: (u8, u8, u8), left: usize, top: usize) -> String {
    let (w, h) = (32 + left, 32 + top);
    let colours = [ink, (224, 48, 58), (98, 190, 64)]; // headphones, berry, leaves
    let which = |c: u8| match c {
        b'K' => Some(0),
        b'R' => Some(1),
        b'g' => Some(2),
        _ => None,
    };
    let pct = |v: u8| (v as u32 * 100 + 127) / 255;
    let mut s = format!("\x1bP0;1;0q\"1;1;{w};{h}");
    for (i, c) in colours.iter().enumerate() {
        s += &format!("#{};2;{};{};{}", i + 1, pct(c.0), pct(c.1), pct(c.2));
    }
    let pixel = |x: usize, y: usize| {
        if x < left || y < top || y - top >= 32 { return None; }
        which(GRID[y - top].as_bytes()[x - left])
    };
    for band in 0..h.div_ceil(6) {
        for ci in 0..colours.len() {
            let mut row = String::new();
            let mut any = false;
            for x in 0..w {
                let mut bits = 0u8;
                for k in 0..6 {
                    let y = band * 6 + k;
                    if y < h && pixel(x, y) == Some(ci) { bits |= 1 << k; }
                }
                any |= bits > 0;
                row.push((63 + bits) as char);
            }
            if any { s += &format!("#{}{}$", ci + 1, row); }
        }
        s.push('-');
    }
    s.push_str("\x1b\\");
    s
}

/// Ask the terminal for its text colour (OSC 10), so the headphones can match the theme.
/// Called before the interface starts; gives up after a moment if the terminal doesn't answer.
pub fn terminal_text_colour() -> Option<(u8, u8, u8)> {
    use crossterm::event::{self, Event, KeyCode};
    use crossterm::terminal;
    use std::io::Write;
    use std::time::{Duration, Instant};

    terminal::enable_raw_mode().ok()?;
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]10;?\x1b\\");
    let _ = out.flush();
    // the answer arrives as typed input, like "rgb:dcdc/dfdf/e4e4"
    let mut got = String::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(300) && !complete(&got) {
        if event::poll(Duration::from_millis(20)).unwrap_or(false) {
            if let Ok(Event::Key(k)) = event::read() {
                if let KeyCode::Char(c) = k.code { got.push(c); }
            }
        }
    }
    // swallow the end of the reply so it doesn't turn into stray key presses
    while event::poll(Duration::from_millis(30)).unwrap_or(false) {
        let _ = event::read();
    }
    let _ = terminal::disable_raw_mode();
    parse(&got)
}

fn complete(s: &str) -> bool {
    s.find("rgb:").is_some_and(|i| {
        let parts: Vec<&str> = s[i + 4..].split('/').collect();
        parts.len() == 3 && parts[2].chars().take_while(|c| c.is_ascii_hexdigit()).count() >= 4
    })
}

fn parse(s: &str) -> Option<(u8, u8, u8)> {
    let mut parts = s[s.find("rgb:")? + 4..].split('/');
    let mut channel = || -> Option<u8> {
        let hex: String = parts.next()?.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        if hex.len() < 2 { return None; }
        let v = u32::from_str_radix(&hex, 16).ok()?;
        let max = (1u32 << (4 * hex.len())) - 1;
        Some((v * 255 / max) as u8)
    };
    Some((channel()?, channel()?, channel()?))
}
