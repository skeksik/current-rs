//! Artwork: an image of an item or user, with what is needed to lay it out before it loads.

use crate::identity::ItemRef;
use crate::open::open_value;

open_value! {
    /// The role of an image. Open: service crates map their server's image types to these where the meaning matches.
    ImageType {
        /// The main image: a poster, cover or portrait.
        PRIMARY = "primary",
        /// A wide background image.
        BACKDROP = "backdrop",
        /// A title logo with transparency.
        LOGO = "logo",
        /// A wide thumbnail.
        THUMB = "thumb",
        BANNER = "banner",
    }
}

/// A width to height ratio, always stored reduced (16:9, never 1920:1080) so equal ratios compare equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AspectRatio {
    width: u32,
    height: u32,
}

impl AspectRatio {
    /// The reduced ratio of `width` to `height`, or `None` if either is zero.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let divisor = gcd(width, height);
        Some(Self {
            width: width / divisor,
            height: height / divisor,
        })
    }

    pub fn width(self) -> u32 {
        self.width
    }

    pub fn height(self) -> u32 {
        self.height
    }

    /// Width divided by height, for layout.
    pub fn as_f64(self) -> f64 {
        f64::from(self.width) / f64::from(self.height)
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Which image, in a form that outlives the address it was read from (ADR 0018): the service crate builds the URL when it fetches,
/// from the address that serves at that moment.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ImageRef {
    /// Whose image it is: an episode's row may show its series' poster.
    pub item: ItemRef,
    pub kind: ImageType,
    /// The image's position among the item's images of `kind`: non-zero only for backdrops after the first.
    pub index: u32,
    /// The server's version of the image. A new image gets a new tag, so an image with the same tag never changes.
    pub tag: String,
}

/// One image.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Artwork {
    pub kind: ImageType,
    pub image: ImageRef,
    pub aspect_ratio: Option<AspectRatio>,
    /// A BlurHash placeholder, if the server provides one.
    pub blurhash: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_are_reduced() {
        let ratio = AspectRatio::new(1920, 1080);
        assert_eq!(ratio, AspectRatio::new(16, 9));
        assert_eq!(ratio.map(|r| (r.width(), r.height())), Some((16, 9)));
        assert_eq!(AspectRatio::new(1000, 1500), AspectRatio::new(2, 3));
        assert_eq!(
            AspectRatio::new(7, 7).map(|r| (r.width(), r.height())),
            Some((1, 1))
        );
    }

    #[test]
    fn zero_sides_have_no_ratio() {
        assert_eq!(AspectRatio::new(0, 9), None);
        assert_eq!(AspectRatio::new(16, 0), None);
    }

    #[test]
    fn ratio_as_float() {
        let ratio = AspectRatio::new(2, 3).map(AspectRatio::as_f64);
        assert!(ratio.is_some_and(|r| (r - 2.0 / 3.0).abs() < f64::EPSILON));
    }
}
