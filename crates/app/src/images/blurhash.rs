//! BlurHash placeholders, decoded in the core to a small BMP every UI can show (ADR 0018, open choice 5).
//!
//! The format is the published one (github.com/woltapp/blurhash, `Algorithm.md` and the reference decoder
//! `Swift/BlurHashDecode.swift`, read 2026-10-25): one base-83 digit for the component counts (`(nx - 1) + (ny - 1) * 9`), one for
//! the largest AC value (`(q + 1) / 166`), four for the DC component (a 24-bit sRGB colour, red in the high bits), then two per AC
//! component, ordered by increasing x then y, each a base-19 triplet `R * 19² + G * 19 + B` dequantised as
//! `signPow((q - 9) / 9, 2) * maximumValue`. A pixel is the sum of `colour[i + j * nx] * cos(π x i / width) * cos(π y j / height)`
//! in linear RGB, converted back to sRGB with the reference's formulas.

use std::f64::consts::PI;

/// The base-83 alphabet, in digit order.
const ALPHABET: &[u8; 83] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~";

/// The long side of a decoded placeholder, in pixels: it is stretched over the box and blurred anyway.
pub(crate) const LONG_SIDE: u32 = 32;

fn digit(byte: u8) -> Option<u32> {
    ALPHABET
        .iter()
        .position(|&known| known == byte)
        .and_then(|position| u32::try_from(position).ok())
}

fn base83(digits: &[u8]) -> Option<u32> {
    digits
        .iter()
        .try_fold(0u32, |value, &byte| Some(value * 83 + digit(byte)?))
}

fn srgb_to_linear(value: u32) -> f64 {
    let v = f64::from(value) / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f64) -> u8 {
    let v = value.clamp(0.0, 1.0);
    let srgb = if v <= 0.003_130_8 {
        v * 12.92 * 255.0 + 0.5
    } else {
        (1.055 * v.powf(1.0 / 2.4) - 0.055) * 255.0 + 0.5
    };
    // Within 0.5..=255.5 after the clamp: the cast truncates as the reference's `Int(...)` does.
    srgb as u8
}

fn sign_pow(value: f64, exponent: f64) -> f64 {
    value.abs().powf(exponent).copysign(value)
}

/// The placeholder's size for a box of `width` by `height`: [`LONG_SIDE`] on the long side, the other at the box's ratio.
pub(crate) fn size_for(width: u32, height: u32) -> (u32, u32) {
    let (long, short) = (width.max(height).max(1), width.min(height));
    let scaled = (u64::from(short) * u64::from(LONG_SIDE) + u64::from(long) / 2) / u64::from(long);
    let scaled = u32::try_from(scaled)
        .unwrap_or(LONG_SIDE)
        .clamp(1, LONG_SIDE);
    if width >= height {
        (LONG_SIDE, scaled)
    } else {
        (scaled, LONG_SIDE)
    }
}

/// Decodes `hash` to `width` by `height` pixels, three bytes each (red, green, blue), rows top to bottom. `None` for a hash that
/// is not valid.
pub(crate) fn decode(hash: &str, width: u32, height: u32) -> Option<Vec<u8>> {
    let hash = hash.as_bytes();
    if hash.len() < 6 || width == 0 || height == 0 {
        return None;
    }
    let flag = base83(&hash[..1])?;
    let (nx, ny) = ((flag % 9 + 1) as usize, (flag / 9 + 1) as usize);
    if hash.len() != 4 + 2 * nx * ny {
        return None;
    }
    let maximum = f64::from(base83(&hash[1..2])? + 1) / 166.0;
    let dc = base83(&hash[2..6])?;
    let mut colours = Vec::with_capacity(nx * ny);
    colours.push([
        srgb_to_linear(dc >> 16),
        srgb_to_linear((dc >> 8) & 255),
        srgb_to_linear(dc & 255),
    ]);
    for component in 1..nx * ny {
        let value = base83(&hash[4 + component * 2..6 + component * 2])?;
        let channel = |quantised: u32| sign_pow((f64::from(quantised) - 9.0) / 9.0, 2.0) * maximum;
        colours.push([
            channel(value / (19 * 19)),
            channel((value / 19) % 19),
            channel(value % 19),
        ]);
    }

    let (w, h) = (width as usize, height as usize);
    // cos(π x i / width) for every x and i, and the same for y and j.
    let table = |size: usize, count: usize| -> Vec<f64> {
        (0..size)
            .flat_map(|at| {
                (0..count)
                    .map(move |component| (PI * at as f64 * component as f64 / size as f64).cos())
            })
            .collect()
    };
    let (across, down) = (table(w, nx), table(h, ny));
    let mut pixels = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0.0f64; 3];
            for j in 0..ny {
                for i in 0..nx {
                    let basis = across[x * nx + i] * down[y * ny + j];
                    for (channel, colour) in sum.iter_mut().zip(colours[i + j * nx]) {
                        *channel += colour * basis;
                    }
                }
            }
            pixels.extend(sum.map(linear_to_srgb));
        }
    }
    Some(pixels)
}

/// An uncompressed 24-bit BMP of `rgb` (rows top to bottom, red first), as Microsoft documents it (learn.microsoft.com,
/// `BITMAPFILEHEADER`, `BITMAPINFOHEADER` and `RGBTRIPLE`, read 2026-10-25): the file header ("bfType ... must be 0x4d42 (the
/// ASCII string "BM")", the file size, two reserved zeros, the offset to the bits), a 40-byte `BITMAPINFOHEADER` with a positive
/// height ("a bottom-up DIB with the origin at the lower left corner"), one plane, 24 bits, `BI_RGB` and a size of 0 ("can be set
/// to 0 for uncompressed RGB bitmaps"); then the rows bottom to top, each pixel blue, green, red (`RGBTRIPLE`'s order), each row
/// padded to "the image width in bytes, rounded up to the nearest DWORD".
pub(crate) fn bmp(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    const HEADERS: u32 = 14 + 40;
    let stride = (width * 3).div_ceil(4) * 4;
    let size = HEADERS + stride * height;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&HEADERS.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    // BI_RGB (0), image size 0, resolution 0 by 0, no colour table.
    out.extend_from_slice(&[0; 24]);
    let row = width as usize * 3;
    for line in rgb.chunks_exact(row).rev() {
        for [red, green, blue] in line.as_chunks::<3>().0 {
            out.extend_from_slice(&[*blue, *green, *red]);
        }
        out.resize(out.len() + (stride as usize - row), 0);
    }
    out
}

/// The placeholder for `hash` in a box of `width` by `height`, as a BMP. `None` for a hash that is not valid.
pub(crate) fn placeholder(hash: &str, width: u32, height: u32) -> Option<Vec<u8>> {
    let (w, h) = size_for(width, height);
    decode(hash, w, h).map(|rgb| bmp(w, h, &rgb))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example in the format's TypeScript README (`decode("LEHV6nWB2yk8pyo0adR*.7kCMdnj", 32, 32)`, read 2026-10-25).
    const EXAMPLE: &str = "LEHV6nWB2yk8pyo0adR*.7kCMdnj";

    fn pixel(rgb: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
        let at = ((y * width + x) * 3) as usize;
        [rgb[at], rgb[at + 1], rgb[at + 2]]
    }

    fn close(actual: [u8; 3], expected: [u8; 3]) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(&a, e)| a.abs_diff(e) <= 1)
    }

    #[test]
    fn the_published_example_decodes_as_the_reference_decoder_does() {
        // No published decoded pixels were found. These were computed independently, with a direct transliteration of the reference
        // decoder (`Swift/BlurHashDecode.swift`) into Python in double precision; the reference works in single precision, so each
        // channel may differ by one.
        let rgb = decode(EXAMPLE, 32, 32).unwrap();
        assert_eq!(rgb.len(), 32 * 32 * 3);
        for ((x, y), expected) in [
            ((0, 0), [135, 164, 177]),
            ((31, 0), [137, 166, 181]),
            ((0, 31), [136, 144, 147]),
            ((31, 31), [133, 142, 147]),
            ((16, 16), [158, 125, 108]),
            ((7, 23), [143, 138, 137]),
        ] {
            let actual = pixel(&rgb, 32, x, y);
            assert!(
                close(actual, expected),
                "({x}, {y}): {actual:?} != {expected:?}"
            );
        }
        let wide = decode(EXAMPLE, 32, 18).unwrap();
        for ((x, y), expected) in [
            ((0, 0), [135, 164, 177]),
            ((31, 17), [132, 142, 147]),
            ((10, 5), [158, 154, 155]),
        ] {
            let actual = pixel(&wide, 32, x, y);
            assert!(
                close(actual, expected),
                "({x}, {y}): {actual:?} != {expected:?}"
            );
        }
    }

    #[test]
    fn the_header_digits_read_as_the_format_says() {
        // "L" is 21: 4 by 3 components; "HV6n", after the maximum's "E", is the DC colour.
        assert_eq!(base83(b"L"), Some(21));
        assert_eq!(base83(b"HV6n"), Some(0x97_96_95));
        assert_eq!(base83(b"~"), Some(82));
        assert_eq!(base83(b"00"), Some(0));
    }

    #[test]
    fn a_hash_with_only_the_dc_component_is_its_colour_everywhere() {
        // Size flag 0 (1 by 1), any maximum, DC 0x979695.
        let rgb = decode("00HV6n", 5, 3).unwrap();
        assert!(
            rgb.as_chunks::<3>()
                .0
                .iter()
                .all(|p| *p == [0x97, 0x96, 0x95])
        );
    }

    #[test]
    fn invalid_hashes_decode_to_nothing() {
        assert_eq!(decode("", 32, 32), None);
        assert_eq!(decode("???", 32, 32), None);
        assert_eq!(decode(&EXAMPLE[..20], 32, 32), None);
        assert_eq!(decode(&format!("{EXAMPLE}00"), 32, 32), None);
        assert_eq!(decode("00V6n\"", 32, 32), None);
        assert_eq!(decode("LEHV6nWB2yk8pyo0adR*.7kCMdné", 32, 32), None);
        assert_eq!(decode(EXAMPLE, 0, 32), None);
    }

    #[test]
    fn placeholders_keep_the_box_ratio_at_most_32_pixels_long() {
        assert_eq!(size_for(320, 480), (21, 32));
        assert_eq!(size_for(1920, 1080), (32, 18));
        assert_eq!(size_for(100, 100), (32, 32));
        assert_eq!(size_for(10_000, 1), (32, 1));
        assert_eq!(size_for(0, 0), (32, 1));
    }

    #[test]
    fn the_bmp_has_the_documented_layout() {
        // 2 by 2: red, green on top; blue, white below. Rows are 6 bytes, padded to 8.
        let rgb = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
        let file = bmp(2, 2, &rgb);
        assert_eq!(file.len(), 54 + 2 * 8);
        let u32_at = |at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap());
        let u16_at = |at: usize| u16::from_le_bytes(file[at..at + 2].try_into().unwrap());
        assert_eq!(&file[..2], b"BM");
        assert_eq!(u32_at(2), 70);
        assert_eq!(u32_at(6), 0);
        assert_eq!(u32_at(10), 54);
        assert_eq!(u32_at(14), 40);
        assert_eq!((u32_at(18), u32_at(22)), (2, 2));
        assert_eq!((u16_at(26), u16_at(28)), (1, 24));
        assert_eq!(u32_at(30), 0);
        // The bottom row first, blue-green-red.
        assert_eq!(&file[54..62], &[255, 0, 0, 255, 255, 255, 0, 0]);
        assert_eq!(&file[62..70], &[0, 0, 255, 0, 255, 0, 0, 0]);
    }

    #[test]
    fn a_placeholder_is_a_bmp_of_the_boxs_shape() {
        let file = placeholder(EXAMPLE, 640, 360).unwrap();
        let u32_at = |at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap());
        assert_eq!((u32_at(18), u32_at(22)), (32, 18));
        assert_eq!(file.len(), 54 + 96 * 18);
        assert_eq!(placeholder("nope", 640, 360), None);
    }
}
