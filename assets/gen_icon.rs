// Generates assets/icon_1024.png — the Redbull app icon: a white lightning
// bolt on a red "squircle", in the macOS app-icon style. Pure std (a tiny
// built-in PNG encoder), so it builds with `rustc -O gen_icon.rs`.
//
// The build pipeline (see package.sh) turns this PNG into Redbull.icns via
// `sips` + `iconutil`.

const SIZE: usize = 1024;

fn main() {
    // --- Rounded-rect ("squircle") background, with a small transparent margin.
    let cx = SIZE as f64 / 2.0;
    let cy = SIZE as f64 / 2.0;
    let half = 412.0; // half-extent => 824px rect, ~100px margin all around
    let radius = 185.0;

    // --- Lightning bolt (Feather "zap") in a 24x24 grid, centered & scaled.
    const BOLT: [(f64, f64); 6] = [
        (13.0, 2.0), (3.0, 14.0), (12.0, 14.0),
        (11.0, 22.0), (21.0, 10.0), (12.0, 10.0),
    ];
    let bolt_scale = 470.0 / 20.0; // ~470px tall
    let poly: Vec<(f64, f64)> = BOLT
        .iter()
        .map(|&(x, y)| (cx + (x - 12.0) * bolt_scale, cy + (y - 12.0) * bolt_scale))
        .collect();

    let in_rect = |x: f64, y: f64| -> bool {
        let ax = (x - cx).abs();
        let ay = (y - cy).abs();
        if ax > half || ay > half {
            return false;
        }
        if ax <= half - radius || ay <= half - radius {
            return true;
        }
        let dx = ax - (half - radius);
        let dy = ay - (half - radius);
        dx * dx + dy * dy <= radius * radius
    };

    let in_bolt = |x: f64, y: f64| -> bool {
        let n = poly.len();
        let mut c = false;
        let mut j = n - 1;
        for i in 0..n {
            let (xi, yi) = poly[i];
            let (xj, yj) = poly[j];
            if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
                c = !c;
            }
            j = i;
        }
        c
    };

    // Diagonal red gradient (top-left -> bottom-right).
    let top = (255.0, 99.0, 72.0);
    let bot = (178.0, 17.0, 38.0);

    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            // 4x4 supersample for smooth edges.
            let (mut cov, mut sr, mut sg, mut sb) = (0u32, 0.0, 0.0, 0.0);
            for sy in 0..4 {
                for sx in 0..4 {
                    let fx = x as f64 + (sx as f64 + 0.5) / 4.0;
                    let fy = y as f64 + (sy as f64 + 0.5) / 4.0;
                    if in_rect(fx, fy) {
                        cov += 1;
                        if in_bolt(fx, fy) {
                            sr += 255.0;
                            sg += 255.0;
                            sb += 255.0;
                        } else {
                            let t = (fx + fy) / (2.0 * SIZE as f64);
                            sr += top.0 + (bot.0 - top.0) * t;
                            sg += top.1 + (bot.1 - top.1) * t;
                            sb += top.2 + (bot.2 - top.2) * t;
                        }
                    }
                }
            }
            let idx = (y * SIZE + x) * 4;
            if cov > 0 {
                rgba[idx] = (sr / cov as f64).round() as u8;
                rgba[idx + 1] = (sg / cov as f64).round() as u8;
                rgba[idx + 2] = (sb / cov as f64).round() as u8;
                rgba[idx + 3] = (cov as f64 / 16.0 * 255.0).round() as u8;
            }
        }
    }

    let png = encode_png(&rgba, SIZE, SIZE);
    std::fs::write("icon_1024.png", &png).expect("write png");
    eprintln!("wrote icon_1024.png ({} bytes)", png.len());
}

// --- Minimal PNG encoder: 8-bit RGBA, stored (uncompressed) deflate. ---------
fn encode_png(rgba: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        raw.push(0); // filter type: none
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }

    let mut out = Vec::new();
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]); // signature

    // IHDR
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // depth=8, color=RGBA, no interlace
    write_chunk(&mut out, b"IHDR", &ihdr);

    // IDAT (zlib: header + stored deflate + adler32)
    let mut zlib = vec![0x78, 0x01];
    let mut pos = 0;
    while pos < raw.len() {
        let chunk = (raw.len() - pos).min(65535);
        let last = if pos + chunk >= raw.len() { 1 } else { 0 };
        zlib.push(last);
        zlib.extend_from_slice(&(chunk as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(chunk as u16)).to_le_bytes());
        zlib.extend_from_slice(&raw[pos..pos + chunk]);
        pos += chunk;
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    write_chunk(&mut out, b"IDAT", &zlib);

    write_chunk(&mut out, b"IEND", &[]);
    out
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_in = Vec::with_capacity(4 + data.len());
    crc_in.extend_from_slice(kind);
    crc_in.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    crc ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}
