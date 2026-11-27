use color_quant::NeuQuant;
use image::imageops::FilterType;

/// A cover encoded as Sixel, plus its size in pixels.
pub struct Art {
    pub data: String,
    pub width: u32,
    pub height: u32,
}

/// Decode an image, fit it inside max_w x max_h pixels (keeping its shape),
/// reduce it to 256 colours, and encode it as Sixel.
pub fn encode(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Art> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = img.resize(max_w.max(1), max_h.max(1), FilterType::Lanczos3).to_rgba8();
    let (w, h) = img.dimensions();

    // flatten any transparency onto black, then pick the best 256 colours
    let mut rgba = img.into_raw();
    for px in rgba.chunks_mut(4) {
        let a = px[3] as u32;
        for c in &mut px[..3] { *c = (*c as u32 * a / 255) as u8; }
        px[3] = 255;
    }
    let nq = NeuQuant::new(10, 256, &rgba);
    let palette = nq.color_map_rgb();
    let indices = dither(&rgba, w as usize, h as usize, &nq);
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

/// Map each pixel to its nearest palette colour with Floyd-Steinberg dithering,
/// so smooth gradients don't band into flat patches.
fn dither(rgba: &[u8], w: usize, h: usize, nq: &NeuQuant) -> Vec<u8> {
    let palette = nq.color_map_rgb();
    let mut buf: Vec<[f32; 3]> = rgba.chunks(4).map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect();
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let c = buf[i].map(|v| v.clamp(0.0, 255.0));
            let idx = nq.index_of(&[c[0] as u8, c[1] as u8, c[2] as u8, 255]);
            out[i] = idx as u8;
            let chosen = &palette[idx * 3..idx * 3 + 3];
            let err = [c[0] - chosen[0] as f32, c[1] - chosen[1] as f32, c[2] - chosen[2] as f32];
            let mut spread = |xx: isize, yy: usize, f: f32| {
                if xx >= 0 && (xx as usize) < w && yy < h {
                    let px = &mut buf[yy * w + xx as usize];
                    for k in 0..3 { px[k] += err[k] * f; }
                }
            };
            spread(x as isize + 1, y, 7.0 / 16.0);
            spread(x as isize - 1, y + 1, 3.0 / 16.0);
            spread(x as isize, y + 1, 5.0 / 16.0);
            spread(x as isize + 1, y + 1, 1.0 / 16.0);
        }
    }
    out
}

/// Write one colour's row of sixels, collapsing repeats as "!<count><char>".
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

