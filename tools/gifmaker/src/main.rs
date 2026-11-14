//! Development tool: numbered PNG frames → an animated GIF for the README.
//!
//! `kiritori-gifmaker <frames folder> <out.gif> [width] [ms per frame]`
//! Frames are read in name order and scaled to `width` (default 960). A frame identical to the
//! one before is merged into it (longer delay), which keeps pauses small.

use image::{RgbaImage, imageops};
use std::fs::File;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: kiritori-gifmaker <frames folder> <out.gif> [width] [ms per frame]");
        std::process::exit(2);
    }
    let width: u32 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(960);
    let ms: u32 = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(200);
    let mut paths: Vec<_> = std::fs::read_dir(&args[1]).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "png")).collect();
    paths.sort();
    let mut frames: Vec<(RgbaImage, u32)> = Vec::new();
    for p in &paths {
        let img = image::open(p).unwrap().to_rgba8();
        let h = img.height() * width / img.width();
        let small = imageops::resize(&img, width, h, imageops::FilterType::Triangle);
        match frames.last_mut() {
            Some((last, d)) if *last == small => *d += ms,
            _ => frames.push((small, ms)),
        }
    }
    let (w, h) = frames[0].0.dimensions();
    let mut enc = gif::Encoder::new(File::create(&args[2]).unwrap(), w as u16, h as u16, &[]).unwrap();
    enc.set_repeat(gif::Repeat::Infinite).unwrap();
    let n = frames.len();
    // Only the part that changed since the last frame is stored; "keep" disposal leaves the
    // rest of the picture as it was.
    let mut prev: Option<RgbaImage> = None;
    for (img, d) in frames {
        let (x0, y0, x1, y1) = match &prev {
            Some(p) => {
                let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
                for (x, y, px) in img.enumerate_pixels() {
                    let q = p.get_pixel(x, y);
                    if (0..3).any(|c| (px[c] as i32 - q[c] as i32).abs() > 6) {
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x + 1);
                        y1 = y1.max(y + 1);
                    }
                }
                if x1 <= x0 { (0, 0, 1, 1) } else { (x0, y0, x1, y1) }
            }
            None => (0, 0, w, h),
        };
        let mut part = imageops::crop_imm(&img, x0, y0, x1 - x0, y1 - y0).to_image().into_raw();
        let mut f = gif::Frame::from_rgba_speed((x1 - x0) as u16, (y1 - y0) as u16, &mut part, 10);
        f.left = x0 as u16;
        f.top = y0 as u16;
        f.delay = (d / 10) as u16;
        f.dispose = gif::DisposalMethod::Keep;
        enc.write_frame(&f).unwrap();
        prev = Some(img);
    }
    println!("{} frames from {} pictures → {}", n, paths.len(), args[2]);
}
