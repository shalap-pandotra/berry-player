
/// A cover encoded as Sixel, plus its size in pixels.
pub struct Art {
    pub data: String,
    pub width: u32,
    pub height: u32,
}

pub fn encode(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Art> {
    let q = berry_core::art::quantize(bytes, max_w, max_h)?;
    let (w, h) = (q.width, q.height);
    let (palette, indices) = (&q.palette, &q.indices);
    let colours = palette.len() / 3;

    let mut s = String::with_capacity((w * h) as usize);
    s.push_str("\x1bP0;1;0q");
    s.push_str(&format!("\"1;1;{w};{h}"));
    for (i, c) in palette.chunks(3).enumerate() {
        let pct = |v: u8| (v as u32 * 100 + 127) / 255;
        s.push_str(&format!("#{i};2;{};{};{}", pct(c[0]), pct(c[1]), pct(c[2])));
    }

    // one row of six-pixel bits per colour, reused for every band
    let mut rows = vec![vec![0u8; w as usize]; colours];
    let mut used = vec![false; colours];
    for band in 0..h.div_ceil(6) {
        for k in 0..6 {
            let y = band * 6 + k;
            if y >= h { break; }
            for x in 0..w {
                let ci = indices[(y * w + x) as usize] as usize;
                rows[ci][x as usize] |= 1 << k;
                used[ci] = true;
            }
        }
        for ci in 0..colours {
            if !used[ci] { continue; }
            s.push_str(&format!("#{ci}"));
            push_rle(&mut s, &rows[ci]);
            s.push('$');
            rows[ci].iter_mut().for_each(|b| *b = 0);
            used[ci] = false;
        }
        s.push('-');
    }
    s.push_str("\x1b\\");
    Some(Art { data: s, width: w, height: h })
}

fn push_rle(s: &mut String, row: &[u8]) {
    let mut i = 0;
    while i < row.len() {
        let mut j = i;
        while j < row.len() && row[j] == row[i] { j += 1; }
        let ch = (63 + row[i]) as char;
        let n = j - i;
        if n > 3 { s.push_str(&format!("!{n}{ch}")); } else { (0..n).for_each(|_| s.push(ch)); }
        i = j;
    }
}

