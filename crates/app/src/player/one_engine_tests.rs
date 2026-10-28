//! ADR 0025 with no native engine (W2): the device profile the server gets is exactly the one before ADR 0025, byte for byte, for
//! libmpv's capabilities and for the test engine's. The routes, reasons and fallback are pinned by `current-playback`'s controller
//! and planner tests, the play method by `current-jellyfin`'s.

use current_mpv::BuildCapabilities;
use current_playback::{EngineCapabilities, Engines};

/// libmpv's capabilities for the D6 Windows build: the decoder and demuxer names `build/mpv-windows/check.py` checks for (ADR 0019,
/// "Engine capabilities and the device profile").
fn libmpv_windows() -> EngineCapabilities {
    let names = |list: &[&str]| list.iter().map(|name| (*name).to_owned()).collect();
    BuildCapabilities {
        decoders: names(&[
            "h264",
            "hevc",
            "av1",
            "vp9",
            "vp8",
            "mpeg2video",
            "vc1",
            "aac",
            "ac3",
            "eac3",
            "truehd",
            "dts",
            "flac",
            "opus",
            "vorbis",
            "mp3",
            "pcm_s16le",
            "pcm_s24le",
        ]),
        demuxers: names(&[
            "matroska,webm",
            "mov,mp4,m4a,3gp,3g2,mj2",
            "mpegts",
            "avi",
            "ogg",
            "flac",
        ]),
    }
    .engine_capabilities()
}

#[test]
fn the_device_profile_is_todays_byte_for_byte() {
    for capabilities in [
        libmpv_windows(),
        super::test_engine::capabilities(),
        EngineCapabilities::default(),
    ] {
        let union = Engines::app_only(capabilities.clone()).device_profile();
        for bitrate in [i32::MAX, 20_000_000, 1_000_000] {
            let before = current_jellyfin::device_profile_json(&capabilities, bitrate);
            let now = current_jellyfin::device_profile_json(&union, bitrate);
            assert!(before.contains("DirectPlayProfiles"), "{before}");
            assert_eq!(now, before);
        }
    }
}
