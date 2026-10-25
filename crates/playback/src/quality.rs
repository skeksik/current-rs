//! The quality limit (ADR 0019, choice 10): "Maximum" by default, plus a per-playback menu remembered separately for local and
//! internet addresses. No automatic bandwidth test.

/// The highest bitrate the user wants a stream at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Quality {
    /// No limit of Current's: the service crate sends its server's ceiling, so the server's own limits are the only cap.
    #[default]
    Maximum,
    /// Bits per second.
    Bitrate(u32),
}

impl Quality {
    /// The menu's fixed steps below "Maximum", highest first.
    pub const STEPS: [Quality; 6] = [
        Quality::Bitrate(40_000_000),
        Quality::Bitrate(20_000_000),
        Quality::Bitrate(10_000_000),
        Quality::Bitrate(8_000_000),
        Quality::Bitrate(4_000_000),
        Quality::Bitrate(2_000_000),
    ];

    /// Whether the limit is below `bitrate` (a source's bitrate in bits per second). A source without a known bitrate counts as above
    /// any limit, as the server counts it.
    pub fn limits(self, bitrate: Option<u32>) -> bool {
        match self {
            Quality::Maximum => false,
            Quality::Bitrate(limit) => bitrate.is_none_or(|bitrate| bitrate > limit),
        }
    }
}

/// The quality remembered per kind of address. The composition root loads and saves it (task 4); the controller reads it and says when
/// the user changes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct QualityMemory {
    pub local: Quality,
    pub internet: Quality,
}

impl QualityMemory {
    pub fn for_address(&self, local: bool) -> Quality {
        if local { self.local } else { self.internet }
    }

    pub fn set(&mut self, local: bool, quality: Quality) {
        if local {
            self.local = quality;
        } else {
            self.internet = quality;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_limit_applies_below_the_source_bitrate_or_without_one() {
        assert!(!Quality::Maximum.limits(Some(100_000_000)));
        assert!(!Quality::Maximum.limits(None));
        assert!(Quality::Bitrate(8_000_000).limits(Some(20_000_000)));
        assert!(!Quality::Bitrate(8_000_000).limits(Some(5_000_000)));
        assert!(Quality::Bitrate(8_000_000).limits(None));
    }

    #[test]
    fn local_and_internet_are_remembered_apart() {
        let mut memory = QualityMemory::default();
        assert_eq!(memory.for_address(true), Quality::Maximum);
        memory.set(false, Quality::Bitrate(4_000_000));
        assert_eq!(memory.for_address(true), Quality::Maximum);
        assert_eq!(memory.for_address(false), Quality::Bitrate(4_000_000));
    }
}
