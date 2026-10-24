//! How the app looks: preferences the user picks in the app's settings. The values are the app's own, not a server's, so the enum is
//! closed; the settings file keeps a value from a newer version that this one does not know (ADR 0013).

/// The theme the app shows: the platform's light or dark theme, or whichever the system uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum AppTheme {
    /// Follow the system's theme, including a change while the app runs.
    #[default]
    System,
    Light,
    Dark,
}

impl AppTheme {
    /// The name the settings file keeps.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// The theme a stored name stands for. `None` for a name this version does not know (a newer version's).
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for theme in [AppTheme::System, AppTheme::Light, AppTheme::Dark] {
            assert_eq!(AppTheme::parse(theme.as_str()), Some(theme));
        }
        assert_eq!(AppTheme::parse("sepia"), None);
        assert_eq!(AppTheme::default(), AppTheme::System);
    }
}
