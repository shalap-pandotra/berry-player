//! Lyrics: plain text, or synced LRC (`[01:23.45]a line`), where each line carries the moment it's
//! sung so the panel can follow along.

use std::time::Duration;

pub struct Lyrics {
    pub lines: Vec<LyricLine>,
    pub synced: bool,
}

pub struct LyricLine {
    pub at: Option<Duration>, // when it's sung (synced lyrics only)
    pub text: String,
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss.xxx` (also with a `:` before the fraction, which some files use).
fn parse_time(s: &str) -> Option<Duration> {
    let (min, rest) = s.split_once(':')?;
    let min: u64 = min.trim().parse().ok()?;
    let (sec, frac) = match rest.find(['.', ':']) {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    let sec: u64 = sec.trim().parse().ok()?;
    if sec >= 60 { return None; }
    let frac = frac.trim();
    let millis = if frac.is_empty() {
        0
    } else {
        let digits: String = frac.chars().take(3).collect();
        let n: u64 = digits.parse().ok()?;
        n * 10u64.pow(3 - digits.len() as u32) // .5 is 500ms, .45 is 450ms, .456 is 456ms
    };
    Some(Duration::from_millis((min * 60 + sec) * 1000 + millis))
}

pub fn parse(text: &str) -> Lyrics {
    let mut offset_ms: i64 = 0;
    let mut timed: Vec<LyricLine> = Vec::new();
    let mut plain: Vec<LyricLine> = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        let mut tag_only = false;
        // a line can start with several [..] groups: times, or tags like [ar:Artist]
        while let Some(inner) = rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
            let (group, after) = inner;
            if let Some(t) = parse_time(group) {
                times.push(t);
            } else if let Some((key, value)) = group.split_once(':') {
                if key.trim().eq_ignore_ascii_case("offset") {
                    offset_ms = value.trim().parse().unwrap_or(0);
                }
                tag_only = true;
            } else {
                break; // just brackets in a lyric, like "[Chorus]": keep it as text
            }
            rest = after.trim_start();
        }
        if times.is_empty() {
            if tag_only && rest.is_empty() { continue; } // a header tag, not a lyric
            plain.push(LyricLine { at: None, text: raw.trim().to_string() });
        } else {
            for t in times {
                timed.push(LyricLine { at: Some(t), text: rest.trim().to_string() });
            }
        }
    }
    // mostly timestamped: synced. Otherwise treat the whole thing as plain text.
    let words = plain.iter().filter(|l| !l.text.is_empty()).count();
    if !timed.is_empty() && timed.len() >= words {
        // a positive offset means the lyrics should show earlier
        for l in &mut timed {
            let ms = l.at.unwrap_or_default().as_millis() as i64 - offset_ms;
            l.at = Some(Duration::from_millis(ms.max(0) as u64));
        }
        timed.sort_by_key(|l| l.at);
        Lyrics { lines: timed, synced: true }
    } else {
        // drop leading and trailing blank lines, keep the blank lines between verses
        while plain.first().is_some_and(|l| l.text.is_empty()) { plain.remove(0); }
        while plain.last().is_some_and(|l| l.text.is_empty()) { plain.pop(); }
        Lyrics { lines: plain, synced: false }
    }
}

impl Lyrics {
    /// The line being sung at `pos` (synced lyrics only): the last one that's started.
    pub fn current(&self, pos: Duration) -> Option<usize> {
        if !self.synced { return None; }
        let started = self.lines.partition_point(|l| l.at.is_some_and(|t| t <= pos));
        started.checked_sub(1)
    }
}

/// Break text into rows of at most `width` characters, at spaces where possible.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        // a single word longer than the row: break it
        while word.chars().count() > width {
            if !row.is_empty() { rows.push(std::mem::take(&mut row)); }
            let head: String = word.chars().take(width).collect();
            word = word.chars().skip(width).collect();
            rows.push(head);
        }
        let need = row.chars().count() + if row.is_empty() { 0 } else { 1 } + word.chars().count();
        if need > width && !row.is_empty() {
            rows.push(std::mem::take(&mut row));
        }
        if !row.is_empty() { row.push(' '); }
        row.push_str(&word);
    }
    rows.push(row); // an empty line stays one empty row
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synced_lyrics() {
        let l = parse("[ar:Someone]\n[ti:Song]\n[offset:500]\n[00:12.00]first line\n[00:15.5][01:02.25]chorus\n[00:20.123]\n[00:18.00]second [quietly]");
        assert!(l.synced);
        let lines: Vec<(u64, &str)> = l.lines.iter().map(|x| (x.at.unwrap().as_millis() as u64, x.text.as_str())).collect();
        // sorted by time, offset applied (500ms earlier), the repeated chorus at both times
        assert_eq!(lines, vec![(11500, "first line"), (15000, "chorus"), (17500, "second [quietly]"), (19623, ""), (61750, "chorus")]);
        assert_eq!(l.current(Duration::from_secs(5)), None);
        assert_eq!(l.current(Duration::from_millis(16000)), Some(1));
        assert_eq!(l.current(Duration::from_secs(300)), Some(4));
    }

    #[test]
    fn plain_lyrics() {
        let l = parse("\n[Verse 1]\nSome words here\n\nMore words\n\n");
        assert!(!l.synced);
        let lines: Vec<&str> = l.lines.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(lines, vec!["[Verse 1]", "Some words here", "", "More words"]);
    }

    #[test]
    fn wrapping() {
        assert_eq!(wrap("the quick brown fox jumps", 10), vec!["the quick", "brown fox", "jumps"]);
        assert_eq!(wrap("", 10), vec![""]);
        assert_eq!(wrap("abcdefghijkl", 5), vec!["abcde", "fghij", "kl"]);
    }
}
