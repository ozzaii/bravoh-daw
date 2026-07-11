//! # bravo-daw
//!
//! Parse DAW project files in pure Rust — no DAW installation required.
//!
//! Supported formats:
//! - **Ableton Live** `.als` (gzipped XML)
//! - **FL Studio** `.flp` (binary event stream)
//! - **Logic Pro** `.logicx` (bundle containing the ProjectData binary)
//! - **REAPER** `.rpp` (plain-text chunk format)
//!
//! Every parser produces the same unified [`ParsedIntelligence`] struct, so
//! downstream code never needs to care which DAW a project came from.
//!
//! ```no_run
//! let intel = bravo_daw::parse("my_track.als").unwrap();
//! println!("{} @ {:?} BPM, {} tracks", intel.daw, intel.bpm, intel.tracks.len());
//! ```
//!
//! Reliability doctrine: if a DAW does not expose a field in a way that can be
//! parsed dependably, the field is omitted (`None` / empty) rather than guessed.

pub mod ableton;
pub mod fl_studio;
pub mod logic;
pub mod project_data;
pub mod reaper;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The DAWs bravo-daw can parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Daw {
    AbletonLive,
    LogicPro,
    FlStudio,
    Reaper,
}

impl Daw {
    /// Stable machine-readable identifier.
    pub fn as_str(&self) -> &'static str {
        match self {
            Daw::AbletonLive => "ableton",
            Daw::LogicPro => "logic",
            Daw::FlStudio => "fl_studio",
            Daw::Reaper => "reaper",
        }
    }

    /// Human-readable name.
    pub fn label(&self) -> &'static str {
        match self {
            Daw::AbletonLive => "Ableton Live",
            Daw::LogicPro => "Logic Pro",
            Daw::FlStudio => "FL Studio",
            Daw::Reaper => "Reaper",
        }
    }

    /// Detect a DAW from a file extension (case-insensitive).
    pub fn from_ext(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "als" | "ableton" => Some(Daw::AbletonLive),
            "logicx" | "logic" => Some(Daw::LogicPro),
            "flp" | "fl_studio" => Some(Daw::FlStudio),
            "rpp" | "reaper" => Some(Daw::Reaper),
            _ => None,
        }
    }

    /// Detect a DAW from a project path's extension.
    pub fn detect(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(Self::from_ext)
    }
}

impl std::fmt::Display for Daw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Errors returned by [`parse`] and [`parse_as`].
#[derive(Debug)]
pub enum Error {
    /// The file extension did not match any supported DAW format.
    UnknownFormat(PathBuf),
    /// The file matched a supported format but could not be parsed.
    Parse {
        daw: Daw,
        path: PathBuf,
        message: String,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::UnknownFormat(path) => {
                write!(f, "unrecognized project format: {}", path.display())
            }
            Error::Parse { daw, path, message } => {
                write!(
                    f,
                    "{} parse failed for {}: {}",
                    daw,
                    path.display(),
                    message
                )
            }
        }
    }
}

impl std::error::Error for Error {}

/// Unified intelligence struct produced by all four parsers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ParsedIntelligence {
    pub daw: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daw_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swing_percentage: Option<f64>,
    pub tracks: Vec<TrackInfo>,
    pub plugins: Vec<PluginInfo>,
    pub samples: Vec<SampleInfo>,
    pub midi_tracks: Vec<MidiTrackInfo>,
    pub markers: Vec<MarkerInfo>,
    pub mixer: MixerInfo,
    pub automated_params: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrackInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub track_type: String, // audio | midi | return | group | master
    pub plugins: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub plugin_type: String, // native | vst | au | clap | js
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SampleInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MidiTrackInfo {
    pub track_name: String,
    pub note_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch_low: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch_high: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MarkerInfo {
    pub name: String,
    pub position_seconds: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MixerInfo {
    pub total_tracks: u32,
    pub audio_tracks: u32,
    pub midi_tracks: u32,
    pub return_tracks: u32,
    pub group_tracks: u32,
    pub has_sidechain: bool,
}

/// Parse a DAW project file, detecting the format from its extension.
pub fn parse<P: AsRef<Path>>(path: P) -> Result<ParsedIntelligence, Error> {
    let path = path.as_ref();
    let daw = Daw::detect(path).ok_or_else(|| Error::UnknownFormat(path.to_path_buf()))?;
    parse_as(path, daw)
}

/// Parse a DAW project file as a specific format.
pub fn parse_as<P: AsRef<Path>>(path: P, daw: Daw) -> Result<ParsedIntelligence, Error> {
    let path = path.as_ref();
    let result = match daw {
        Daw::AbletonLive => ableton::parse(path),
        Daw::LogicPro => logic::parse(path),
        Daw::FlStudio => fl_studio::parse(path),
        Daw::Reaper => reaper::parse(path),
    };
    result.map_err(|message| Error::Parse {
        daw,
        path: path.to_path_buf(),
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsed_intelligence_serializes_to_json() {
        let intel = ParsedIntelligence {
            daw: "Ableton Live".to_string(),
            bpm: Some(128.0),
            time_signature: Some("4/4".to_string()),
            tracks: vec![TrackInfo {
                name: "Drums".to_string(),
                track_type: "audio".to_string(),
                plugins: vec!["Drum Rack".to_string()],
                color: None,
            }],
            plugins: vec![PluginInfo {
                name: "Serum".to_string(),
                plugin_type: "vst".to_string(),
                count: 2,
            }],
            mixer: MixerInfo {
                total_tracks: 14,
                audio_tracks: 10,
                midi_tracks: 3,
                return_tracks: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&intel).unwrap();
        assert!(json.contains("\"daw\":\"Ableton Live\""));
        assert!(json.contains("\"bpm\":128.0"));
        assert!(json.contains("\"type\":\"audio\""));
        assert!(json.contains("\"type\":\"vst\""));
        // Optional None fields should be absent
        assert!(!json.contains("\"key\""));
        assert!(!json.contains("\"daw_version\""));
    }

    #[test]
    fn mixer_info_defaults() {
        let m = MixerInfo::default();
        assert_eq!(m.total_tracks, 0);
        assert!(!m.has_sidechain);
    }

    #[test]
    fn detect_by_extension() {
        assert_eq!(Daw::detect(Path::new("x.als")), Some(Daw::AbletonLive));
        assert_eq!(Daw::detect(Path::new("x.FLP")), Some(Daw::FlStudio));
        assert_eq!(Daw::detect(Path::new("x.logicx")), Some(Daw::LogicPro));
        assert_eq!(Daw::detect(Path::new("x.rpp")), Some(Daw::Reaper));
        assert_eq!(Daw::detect(Path::new("x.wav")), None);
        assert_eq!(Daw::detect(Path::new("noext")), None);
    }

    #[test]
    fn parse_unknown_format_errors() {
        let err = parse("/nonexistent/song.wav").unwrap_err();
        assert!(matches!(err, Error::UnknownFormat(_)));
    }

    #[test]
    fn parse_nonexistent_file_errors() {
        let err = parse("/nonexistent/project.als").unwrap_err();
        assert!(matches!(
            err,
            Error::Parse {
                daw: Daw::AbletonLive,
                ..
            }
        ));
    }

    #[test]
    fn parse_corrupt_file_errors() {
        // A text file pretending to be .als won't parse
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake.als");
        std::fs::write(&path, b"not a gzip file").unwrap();
        assert!(parse(&path).is_err());
    }
}
