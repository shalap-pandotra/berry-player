//! The berry look for cover art: shrink the picture, pick its best 256 colours, and dither it onto
//! them (Floyd-Steinberg). That's where the slightly grainy, lofi feel of berry's covers comes from.
//! The palette comes from the cover itself, so it keeps its own colours.
//!
//! The terminal writes the result out as Sixel; anything else (the Android app, later) can take it
//! as plain RGBA with `to_rgba` and show it scaled up without smoothing, so the pixels stay crisp.

use color_quant::NeuQuant;
use image::imageops::FilterType;

/// A cover after the berry filter: every pixel is an index into a palette of up to 256 colours.
pub struct Quantized {
    pub width: u32,
    pub height: u32,
    /// The colours, three bytes (r, g, b) each.
    pub palette: Vec<u8>,
    /// One palette index per pixel, row by row.
    pub indices: Vec<u8>,
}

/// Decode an image, fit it inside max_w x max_h pixels (keeping its shape), and reduce it to its
/// best 256 colours with dithering. `None` if the bytes aren't an image berry can read.
pub fn quantize(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Quantized> {
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
    let indices = dither(&rgba, w as usize, h as usize, &nq);
    Some(Quantized { width: w, height: h, palette: nq.color_map_rgb(), indices })
}

impl Quantized {
    /// The picture as plain RGBA bytes (fully opaque), for anything that doesn't speak Sixel.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.indices.len() * 4);
        for &i in &self.indices {
            let c = &self.palette[i as usize * 3..i as usize * 3 + 3];
            out.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
        out
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_a_cover() {
        // a 300x200 gradient, as a PNG, squeezed into a 120x120 box
        let img = image::RgbImage::from_fn(300, 200, |x, y| image::Rgb([(x * 255 / 299) as u8, (y * 255 / 199) as u8, 128]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let q = quantize(&png, 120, 120).unwrap();
        assert_eq!((q.width, q.height), (120, 80)); // shape kept
        assert!(q.palette.len() <= 256 * 3 && q.palette.len() % 3 == 0);
        assert_eq!(q.indices.len(), 120 * 80);
        assert!(q.indices.iter().all(|&i| (i as usize) < q.palette.len() / 3));
        assert_eq!(q.to_rgba().len(), 120 * 80 * 4);
        assert!(quantize(b"not an image", 10, 10).is_none());
    }
}
