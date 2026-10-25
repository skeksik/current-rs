//! What a detail page says about an item's media: a thin summary of its default version (ADR 0018). Media sources and the full
//! stream types are ADR 0019's; they map to these.

/// The default version of an item's media.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaSummary {
    /// How many versions the item has.
    pub source_count: u32,
    /// The container format, as the server names it.
    pub container: Option<String>,
    pub video: Option<StreamSummary>,
    pub audio: Vec<StreamSummary>,
    pub subtitles: Vec<StreamSummary>,
}

/// One stream of the default version, as a detail page lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamSummary {
    /// The server's display title for the stream.
    pub label: String,
    pub language: Option<String>,
    pub codec: Option<String>,
}
