//! Settings' "Supported formats" page across the bridge (ADR 0021): [`Core::supported_formats`] takes the facts the UI found out (the
//! graphics card's decoder profiles, the window's display) and gives the whole page; the UI only renders it.

use std::sync::Arc;

use crate::cancel::{self, CancelToken};
use crate::player_records::DisplayFacts;
use crate::{Core, CoreError};

/// One of the graphics card's video decoder profiles (`ID3D11VideoDevice::GetVideoDecoderProfile` on Windows) and whether the driver
/// accepts NV12 (8-bit) and P010 (10-bit) output for it (`CheckVideoDecoderFormat`).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DecoderProfileFact {
    /// The profile's GUID as text, in any case, with or without braces.
    pub guid: String,
    pub nv12: bool,
    pub p010: bool,
}

/// What the UI's test of the graphics card found: the adapter's name (shown on the page, never logged) and every decoder profile it
/// lists.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct HardwareDecodeFacts {
    pub adapter: String,
    pub profiles: Vec<DecoderProfileFact>,
}

impl std::fmt::Debug for HardwareDecodeFacts {
    /// Leaves out the adapter's name.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HardwareDecodeFacts")
            .field("adapter", &"<redacted>")
            .field("profiles", &self.profiles)
            .finish()
    }
}

impl From<HardwareDecodeFacts> for current_app::HardwareDecoding {
    fn from(facts: HardwareDecodeFacts) -> Self {
        Self {
            adapter: facts.adapter,
            profiles: facts
                .profiles
                .into_iter()
                .map(|profile| current_app::DecoderProfile {
                    guid: profile.guid,
                    nv12: profile.nv12,
                    p010: profile.p010,
                })
                .collect(),
        }
    }
}

/// The page's tabs, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FormatTabKind {
    Video,
    Audio,
    Containers,
    Subtitles,
    Hdr,
}

impl From<current_app::FormatTabKind> for FormatTabKind {
    fn from(kind: current_app::FormatTabKind) -> Self {
        use current_app::FormatTabKind as K;
        match kind {
            K::Video => Self::Video,
            K::Audio => Self::Audio,
            K::Containers => Self::Containers,
            K::Subtitles => Self::Subtitles,
            K::Hdr => Self::Hdr,
        }
    }
}

/// The "On this PC" column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FormatSupport {
    /// Decoded by the graphics card.
    Hardware,
    /// Decoded by the processor.
    Software,
    /// Plays (containers, subtitles; video when the graphics card could not be tested).
    Plays,
    /// Plays after the server repackages the stream (remux).
    ServerRepackages,
    /// Plays after the server converts it (transcode).
    ServerConverts,
    NotSupported,
    /// HDR shown in HDR.
    Hdr,
    /// HDR converted to SDR by the player.
    ToneMapped,
    /// Not known: the player could not be started to find out.
    Unknown,
}

impl From<current_app::FormatSupport> for FormatSupport {
    fn from(support: current_app::FormatSupport) -> Self {
        use current_app::FormatSupport as S;
        match support {
            S::Hardware => Self::Hardware,
            S::Software => Self::Software,
            S::Plays => Self::Plays,
            S::ServerRepackages => Self::ServerRepackages,
            S::ServerConverts => Self::ServerConverts,
            S::NotSupported => Self::NotSupported,
            S::Hdr => Self::Hdr,
            S::ToneMapped => Self::ToneMapped,
            S::Unknown => Self::Unknown,
        }
    }
}

/// One row: the format's name, the column (its value and the word to show for it), and a note (empty when there is nothing to say).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FormatRow {
    pub name: String,
    pub support: FormatSupport,
    pub support_label: String,
    pub notes: String,
}

/// One tab: its title and the headers of its three columns ("Codec" or "Format", "On this PC", "Notes"), and its rows. Every word
/// the page shows comes from the core.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FormatTab {
    pub kind: FormatTabKind,
    pub title: String,
    pub name_header: String,
    pub support_header: String,
    pub notes_header: String,
    pub rows: Vec<FormatRow>,
}

/// The page: the text above the tabs, what the Hardware column rests on, and the tabs.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SupportedFormats {
    pub description: String,
    pub hardware_note: String,
    pub tabs: Vec<FormatTab>,
}

impl From<current_app::SupportedFormats> for SupportedFormats {
    fn from(page: current_app::SupportedFormats) -> Self {
        Self {
            description: page.description,
            hardware_note: page.hardware_note,
            tabs: page
                .tabs
                .into_iter()
                .map(|tab| FormatTab {
                    kind: tab.kind.into(),
                    title: tab.title,
                    name_header: tab.name_header,
                    support_header: tab.support_header,
                    notes_header: tab.notes_header,
                    rows: tab
                        .rows
                        .into_iter()
                        .map(|row| FormatRow {
                            name: row.name,
                            support: row.support.into(),
                            support_label: row.support_label,
                            notes: row.notes,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

#[uniffi::export]
impl Core {
    /// Settings' "Supported formats" page. `hardware` is the UI's test of the graphics card (`None` when it could not run), `display`
    /// the facts of the display the window is on (`None` when unknown). The first call may start a headless engine for a moment to read
    /// what it plays (no window, no audio device); later ones answer at once.
    ///
    /// # Errors
    /// [`CoreError::Cancelled`] if `token` is cancelled first.
    pub async fn supported_formats(
        &self,
        hardware: Option<HardwareDecodeFacts>,
        display: Option<DisplayFacts>,
        token: Arc<CancelToken>,
    ) -> Result<SupportedFormats, CoreError> {
        let app = self.app.clone();
        cancel::run_cancellable(&self.runtime, token, async move {
            let page = app
                .supported_formats(hardware.map(Into::into), display.map(Into::into))
                .await;
            Ok(page.into())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::block_on;

    #[test]
    fn the_page_crosses_with_the_facts_passed_in() {
        let core = Core::new().unwrap();
        let hardware = HardwareDecodeFacts {
            adapter: "Example Graphics".into(),
            profiles: vec![DecoderProfileFact {
                guid: "{5B11D51B-2F4C-4452-BCC3-09F2A1160CC0}".into(),
                nv12: true,
                p010: false,
            }],
        };
        let display = DisplayFacts {
            hdr: true,
            peak_nits: 1000.0,
            sdr_white_nits: 240.0,
            refresh_hz: 60.0,
        };
        let page =
            block_on(core.supported_formats(Some(hardware), Some(display), CancelToken::new()))
                .unwrap();
        assert_eq!(page.tabs.len(), 5);
        assert!(page.tabs.iter().all(|tab| !tab.rows.is_empty()));
        assert_eq!(page.hardware_note, "Tested on: Example Graphics");
        let video = &page.tabs[0];
        assert_eq!(video.kind, FormatTabKind::Video);
        let hevc = video
            .rows
            .iter()
            .find(|row| row.name == "HEVC (H.265)")
            .unwrap();
        assert_eq!(hevc.support, FormatSupport::Hardware);
        assert_eq!(hevc.support_label, "Hardware");
        assert_eq!(video.title, "Video");
        assert_eq!(
            (video.support_header.as_str(), video.notes_header.as_str()),
            ("On this PC", "Notes")
        );
        assert!(
            page.tabs[4]
                .rows
                .iter()
                .all(|row| row.support == FormatSupport::Hdr)
        );
    }

    #[test]
    fn the_facts_hide_the_adapter_name() {
        let facts = HardwareDecodeFacts {
            adapter: "Example Graphics".into(),
            profiles: Vec::new(),
        };
        assert!(!format!("{facts:?}").contains("Example"));
    }
}
