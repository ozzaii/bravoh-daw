//! Logic Pro .logicx project file parser.
//!
//! Thin wrapper over the existing `project_data.rs` module that already
//! parses Logic Pro's ProjectData binary. This module finds the ProjectData
//! file inside the .logicx bundle, calls the existing parser, and maps
//! the Logic-specific `ProjectIntelligence` to the unified `ParsedIntelligence`.

use super::{MarkerInfo, MixerInfo, ParsedIntelligence, PluginInfo, TrackInfo};
use std::path::Path;

/// Find the ProjectData file inside a .logicx bundle.
/// Logic stores it in several possible locations:
/// - <project>.logicx/ProjectData
/// - <project>.logicx/Alternatives/000/ProjectData
/// - <project>.logicx/Alternatives/<NNN>/ProjectData
fn find_project_data(logicx_path: &Path) -> Option<std::path::PathBuf> {
    // Direct path
    let direct = logicx_path.join("ProjectData");
    if direct.exists() {
        return Some(direct);
    }

    // Alternatives/000/
    let alt_000 = logicx_path
        .join("Alternatives")
        .join("000")
        .join("ProjectData");
    if alt_000.exists() {
        return Some(alt_000);
    }

    // Scan numbered alternatives directories
    let alt_dir = logicx_path.join("Alternatives");
    if let Ok(entries) = std::fs::read_dir(&alt_dir) {
        let mut candidates: Vec<std::path::PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
            .map(|e| e.path().join("ProjectData"))
            .filter(|p| p.exists())
            .collect();
        // Sort to get a deterministic result (lowest numbered alternative)
        candidates.sort();
        if let Some(first) = candidates.into_iter().next() {
            return Some(first);
        }
    }

    None
}

/// Classify a Logic track hint string into a track type.
fn classify_track_hint(hint: &str) -> &'static str {
    if hint.starts_with("Audio ") {
        "audio"
    } else if hint.starts_with("Inst ") {
        "midi"
    } else if hint.starts_with("Aux ") || hint.starts_with("Bus ") {
        "return"
    } else {
        "audio" // default for generic "Track N" and unknown hints
    }
}

/// Map Logic's `ProjectIntelligence` to the unified `ParsedIntelligence`.
fn map_to_unified(raw: crate::project_data::ProjectIntelligence) -> ParsedIntelligence {
    let mut intel = ParsedIntelligence {
        daw: "Logic Pro".to_string(),
        bpm: raw.bpm,
        time_signature: raw.time_signature.clone(),
        duration_seconds: raw.duration_seconds,
        swing_percentage: raw.swing_percentage,
        ..Default::default()
    };

    // Map plugins: PluginCount -> PluginInfo
    intel.plugins = raw
        .plugins
        .iter()
        .map(|pc| PluginInfo {
            name: pc.name.clone(),
            plugin_type: "native".to_string(),
            count: pc.count,
        })
        .collect();

    // Map track hints -> TrackInfo with classification
    let mut audio_count: u32 = 0;
    let mut midi_count: u32 = 0;
    let mut return_count: u32 = 0;

    for hint in &raw.track_hints {
        let track_type = classify_track_hint(hint);
        match track_type {
            "audio" => audio_count += 1,
            "midi" => midi_count += 1,
            "return" => return_count += 1,
            _ => {}
        }
        intel.tracks.push(TrackInfo {
            name: hint.clone(),
            track_type: track_type.to_string(),
            plugins: Vec::new(),
            color: None,
        });
    }

    // Map markers (Logic parser doesn't extract positions, so position_seconds = 0.0)
    intel.markers = raw
        .markers
        .iter()
        .map(|name| MarkerInfo {
            name: name.clone(),
            position_seconds: 0.0,
        })
        .collect();

    // Map samples
    intel.samples = raw
        .samples
        .iter()
        .map(|name| super::SampleInfo {
            name: name.clone(),
            path: None,
        })
        .collect();

    // Build MixerInfo from classified tracks
    let total = intel.tracks.len() as u32;
    intel.mixer = MixerInfo {
        total_tracks: total,
        audio_tracks: audio_count,
        midi_tracks: midi_count,
        return_tracks: return_count,
        group_tracks: 0, // Logic doesn't expose group tracks in ProjectData strings
        has_sidechain: raw.routing_hints.has_routing,
    };

    intel
}

/// Parse a Logic Pro .logicx project bundle.
pub fn parse(path: &Path) -> Result<ParsedIntelligence, String> {
    let pd_path =
        find_project_data(path).ok_or_else(|| format!("No ProjectData found in {:?}", path))?;

    let raw = crate::project_data::parse_project_data(&pd_path)
        .ok_or_else(|| format!("Failed to parse ProjectData at {:?}", pd_path))?;

    Ok(map_to_unified(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_data::{PluginCount, ProjectIntelligence, RoutingHints};

    /// Helper to build a ProjectIntelligence for testing the mapping.
    fn sample_raw() -> ProjectIntelligence {
        ProjectIntelligence {
            bpm: Some(120.0),
            time_signature: Some("4/4".to_string()),
            duration_seconds: Some(180.0),
            swing_percentage: Some(12.5),
            plugins: vec![
                PluginCount {
                    name: "Channel EQ".to_string(),
                    count: 3,
                },
                PluginCount {
                    name: "Compressor".to_string(),
                    count: 2,
                },
            ],
            track_hints: vec![
                "Audio 1".to_string(),
                "Audio 2".to_string(),
                "Inst 1".to_string(),
                "Aux 1".to_string(),
                "Bus 1".to_string(),
            ],
            markers: vec!["Verse".to_string(), "Chorus".to_string()],
            samples: vec!["kick_909.wav".to_string()],
            routing_hints: RoutingHints {
                has_routing: true,
                buses_used: vec![1],
                bus_count: 1,
                send_count: 2,
            },
            ..Default::default()
        }
    }

    #[test]
    fn map_to_unified_preserves_bpm() {
        let raw = sample_raw();
        let intel = map_to_unified(raw);
        assert_eq!(intel.daw, "Logic Pro");
        assert_eq!(intel.bpm, Some(120.0));
        assert_eq!(intel.time_signature.as_deref(), Some("4/4"));
        assert_eq!(intel.duration_seconds, Some(180.0));
        assert_eq!(intel.swing_percentage, Some(12.5));
    }

    #[test]
    fn map_to_unified_maps_plugins() {
        let raw = sample_raw();
        let intel = map_to_unified(raw);
        assert_eq!(intel.plugins.len(), 2);

        let eq = intel
            .plugins
            .iter()
            .find(|p| p.name == "Channel EQ")
            .unwrap();
        assert_eq!(eq.plugin_type, "native");
        assert_eq!(eq.count, 3);

        let comp = intel
            .plugins
            .iter()
            .find(|p| p.name == "Compressor")
            .unwrap();
        assert_eq!(comp.plugin_type, "native");
        assert_eq!(comp.count, 2);
    }

    #[test]
    fn map_to_unified_classifies_track_hints() {
        let raw = sample_raw();
        let intel = map_to_unified(raw);

        // 5 tracks total: Audio 1, Audio 2, Inst 1, Aux 1, Bus 1
        assert_eq!(intel.tracks.len(), 5);

        let audio_1 = intel.tracks.iter().find(|t| t.name == "Audio 1").unwrap();
        assert_eq!(audio_1.track_type, "audio");

        let inst_1 = intel.tracks.iter().find(|t| t.name == "Inst 1").unwrap();
        assert_eq!(inst_1.track_type, "midi");

        let aux_1 = intel.tracks.iter().find(|t| t.name == "Aux 1").unwrap();
        assert_eq!(aux_1.track_type, "return");

        let bus_1 = intel.tracks.iter().find(|t| t.name == "Bus 1").unwrap();
        assert_eq!(bus_1.track_type, "return");

        // Mixer counts
        assert_eq!(intel.mixer.total_tracks, 5);
        assert_eq!(intel.mixer.audio_tracks, 2);
        assert_eq!(intel.mixer.midi_tracks, 1);
        assert_eq!(intel.mixer.return_tracks, 2);
        assert_eq!(intel.mixer.group_tracks, 0);
        assert!(intel.mixer.has_sidechain); // routing_hints.has_routing
    }

    #[test]
    fn map_to_unified_maps_markers_and_samples() {
        let raw = sample_raw();
        let intel = map_to_unified(raw);

        assert_eq!(intel.markers.len(), 2);
        assert_eq!(intel.markers[0].name, "Verse");
        assert_eq!(intel.markers[0].position_seconds, 0.0);
        assert_eq!(intel.markers[1].name, "Chorus");

        assert_eq!(intel.samples.len(), 1);
        assert_eq!(intel.samples[0].name, "kick_909.wav");
        assert!(intel.samples[0].path.is_none());
    }

    #[test]
    fn map_empty_project() {
        let raw = ProjectIntelligence::default();
        let intel = map_to_unified(raw);
        assert_eq!(intel.daw, "Logic Pro");
        assert_eq!(intel.bpm, None);
        assert!(intel.tracks.is_empty());
        assert!(intel.plugins.is_empty());
        assert!(intel.markers.is_empty());
        assert!(intel.samples.is_empty());
        assert_eq!(intel.mixer.total_tracks, 0);
        assert!(!intel.mixer.has_sidechain);
    }

    #[test]
    fn parse_nonexistent_logicx_returns_error() {
        let result = parse(Path::new("/nonexistent/project.logicx"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No ProjectData"));
    }
}
