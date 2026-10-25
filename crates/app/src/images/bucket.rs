//! Box sizes rounded to a few widths, so that a few sizes per layout serve every display scale (ADR 0018, section 4).

/// The widths a request is rounded up to, in physical pixels.
pub const WIDTH_BUCKETS: [u32; 10] = [160, 240, 320, 480, 640, 960, 1280, 1920, 2560, 3840];

/// The size to fetch for a box of `width` by `height` physical pixels: the width rounded up to the next bucket (the largest for
/// anything wider), the height scaled by the box's ratio and rounded up. `None` for an empty box.
pub fn bucket(width: u32, height: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let largest = WIDTH_BUCKETS[WIDTH_BUCKETS.len() - 1];
    let bucket = WIDTH_BUCKETS
        .iter()
        .copied()
        .find(|&bucket| bucket >= width)
        .unwrap_or(largest);
    let scaled = (u64::from(height) * u64::from(bucket)).div_ceil(u64::from(width));
    // A box hundreds of times taller than wide would ask for more than any image has: the server never upscales anyway.
    let scaled = u32::try_from(scaled)
        .unwrap_or(u32::MAX)
        .clamp(1, largest * 4);
    Some((bucket, scaled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_round_up_to_a_bucket_and_heights_follow_the_ratio() {
        assert_eq!(bucket(150, 225), Some((160, 240)));
        assert_eq!(bucket(160, 240), Some((160, 240)));
        assert_eq!(bucket(161, 90), Some((240, 135)));
        // 2:3 at 1.5x scale: 300 by 450 view pixels times 1.5.
        assert_eq!(bucket(450, 675), Some((480, 720)));
        assert_eq!(bucket(1921, 1081), Some((2560, 1441)));
        assert_eq!(bucket(1, 1), Some((160, 160)));
    }

    #[test]
    fn wider_than_the_largest_bucket_is_scaled_down_to_it() {
        assert_eq!(bucket(7680, 4320), Some((3840, 2160)));
    }

    #[test]
    fn empty_and_extreme_boxes() {
        assert_eq!(bucket(0, 100), None);
        assert_eq!(bucket(100, 0), None);
        assert_eq!(bucket(1, u32::MAX), Some((160, 3840 * 4)));
        assert_eq!(bucket(u32::MAX, 1), Some((3840, 1)));
    }
}
