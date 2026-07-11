//! Reaper .rpp project file parser.
//!
//! Reaper projects are plain text with hierarchical bracket structure.
//! Line-by-line regex parsing — matches the Python reaper.py parser.

use super::{
    MarkerInfo, MidiTrackInfo, MixerInfo, ParsedIntelligence, PluginInfo, SampleInfo, TrackInfo,
};
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

const MAX_READ_BYTES: u64 = 10 * 1024 * 1024; // 10MB safety cap

// Regexes — compiled once via LazyLock
static RE_TEMPO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*TEMPO\s+([\d.]+)\s+(\d+)\s+(\d+)").unwrap());
static RE_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*MARKER\s+\d+\s+([\d.]+)\s+"([^"]*)""#).unwrap());
static RE_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^\s*NAME\s+"([^"]*)""#).unwrap());
static RE_VST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*<VST\s+"(?:VST[i3]?:\s*)([^"]+)""#).unwrap());
static RE_AU: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*<AU[i]?\s+"(?:AU[i]?:\s*)([^"]+)""#).unwrap());
static RE_CLAP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*<CLAP\s+"(?:CLAP:\s*)([^"]+)""#).unwrap());
static RE_JS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^\s*<JS\s+"([^"]+)""#).unwrap());
static RE_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^\s*FILE\s+"([^"]*)""#).unwrap());
static RE_MIDI_NOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*E\s+\d+\s+9[0-9a-fA-F]\s+([0-9a-fA-F]{2})\s+([0-9a-fA-F]{2})").unwrap()
});
static RE_AUXRECV: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*AUXRECV\s+").unwrap());
static RE_ENVELOPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*<(VOLENV|PANENV|WIDTHENV|PARMENV|MUTEENV|FXENV)").unwrap());
static RE_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*<REAPER_PROJECT\s+[\d.]+\s+"([^"]*)""#).unwrap());

/// Clean plugin name: strip vendor in parentheses.
/// "FabFilter Pro-Q 3 (FabFilter)" -> "FabFilter Pro-Q 3"
fn clean_plugin_name(raw: &str) -> String {
    if let Some(idx) = raw.find('(') {
        if idx > 0 {
            return raw[..idx].trim().to_string();
        }
    }
    raw.trim().to_string()
}

pub fn parse(path: &Path) -> Result<ParsedIntelligence, String> {
    // Size check
    let meta = std::fs::metadata(path).map_err(|e| format!("Metadata: {}", e))?;
    if meta.len() > MAX_READ_BYTES {
        return Err(format!("File too large: {} bytes", meta.len()));
    }

    let content = std::fs::read_to_string(path).map_err(|e| format!("Read: {}", e))?;
    parse_content(&content)
}

/// Parse from string content (testable without files).
pub fn parse_content(content: &str) -> Result<ParsedIntelligence, String> {
    let mut intel = ParsedIntelligence {
        daw: "Reaper".to_string(),
        ..Default::default()
    };

    // State tracking
    let mut depth: i32 = 0;
    let mut in_track = false;
    let mut in_source = false;
    let mut current_track_name = String::new();
    let mut current_track_plugins: Vec<String> = Vec::new();
    let mut current_track_has_auxrecv = false;
    let mut current_midi_notes: Vec<u8> = Vec::new(); // pitches
    let mut plugin_counts: HashMap<String, u32> = HashMap::new();
    let mut tracks: Vec<TrackInfo> = Vec::new();
    let mut audio_tracks: u32 = 0;
    let mut midi_tracks: u32 = 0;
    let mut return_tracks: u32 = 0;
    let mut has_sidechain = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Version from first line
        if intel.daw_version.is_none() {
            if let Some(caps) = RE_VERSION.captures(trimmed) {
                intel.daw_version = Some(caps[1].to_string());
            }
        }

        // Bracket tracking
        if trimmed.starts_with('<') {
            depth += 1;

            // Track start
            if trimmed.starts_with("<TRACK") && depth == 2 {
                in_track = true;
                current_track_name.clear();
                current_track_plugins.clear();
                current_track_has_auxrecv = false;
                current_midi_notes.clear();
            }

            // Source block
            if trimmed.starts_with("<SOURCE") {
                in_source = true;
            }

            // Plugin detection
            if in_track {
                let plugin_name = if let Some(caps) = RE_VST.captures(trimmed) {
                    Some(("vst", clean_plugin_name(&caps[1])))
                } else if let Some(caps) = RE_AU.captures(trimmed) {
                    Some(("au", clean_plugin_name(&caps[1])))
                } else if let Some(caps) = RE_CLAP.captures(trimmed) {
                    Some(("clap", clean_plugin_name(&caps[1])))
                } else {
                    RE_JS
                        .captures(trimmed)
                        .map(|caps| ("js", caps[1].to_string()))
                };

                if let Some((ptype, name)) = plugin_name {
                    current_track_plugins.push(name.clone());
                    *plugin_counts
                        .entry(format!("{}:{}", ptype, name))
                        .or_insert(0) += 1;
                }
            }
        }

        if trimmed == ">" {
            depth -= 1;

            // Track end
            if in_track && depth < 2 {
                let track_type = if current_track_has_auxrecv {
                    return_tracks += 1;
                    "return"
                } else if !current_midi_notes.is_empty() {
                    midi_tracks += 1;
                    "midi"
                } else {
                    audio_tracks += 1;
                    "audio"
                };

                // MIDI summary
                if !current_midi_notes.is_empty() {
                    let low = *current_midi_notes.iter().min().unwrap_or(&0);
                    let high = *current_midi_notes.iter().max().unwrap_or(&127);
                    intel.midi_tracks.push(MidiTrackInfo {
                        track_name: current_track_name.clone(),
                        note_count: current_midi_notes.len() as u32,
                        pitch_low: Some(low),
                        pitch_high: Some(high),
                    });
                }

                tracks.push(TrackInfo {
                    name: current_track_name.clone(),
                    track_type: track_type.to_string(),
                    plugins: current_track_plugins.clone(),
                    color: None,
                });

                in_track = false;
            }

            if in_source {
                in_source = false;
            }
        }

        // TEMPO line (top-level only)
        if depth <= 1 {
            if let Some(caps) = RE_TEMPO.captures(trimmed) {
                if let Ok(bpm) = caps[1].parse::<f64>() {
                    if (30.0..=300.0).contains(&bpm) {
                        intel.bpm = Some(bpm);
                    }
                }
                let num: u32 = caps[2].parse().unwrap_or(4);
                let denom: u32 = caps[3].parse().unwrap_or(4);
                intel.time_signature = Some(format!("{}/{}", num, denom));
            }
        }

        // MARKER line
        if let Some(caps) = RE_MARKER.captures(trimmed) {
            let pos: f64 = caps[1].parse().unwrap_or(0.0);
            let name = caps[2].to_string();
            if !name.is_empty() {
                intel.markers.push(MarkerInfo {
                    name,
                    position_seconds: pos,
                });
            }
        }

        // NAME inside track
        if in_track && current_track_name.is_empty() {
            if let Some(caps) = RE_NAME.captures(trimmed) {
                current_track_name = caps[1].to_string();
            }
        }

        // AUXRECV — indicates return track
        if in_track && RE_AUXRECV.is_match(trimmed) {
            current_track_has_auxrecv = true;
            has_sidechain = true;
        }

        // FILE inside source — samples
        if in_source {
            if let Some(caps) = RE_FILE.captures(trimmed) {
                let file_path = caps[1].to_string();
                let name = file_path
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(&file_path)
                    .to_string();
                intel.samples.push(SampleInfo {
                    name,
                    path: Some(file_path),
                });
            }
        }

        // MIDI note-on
        if in_track {
            if let Some(caps) = RE_MIDI_NOTE.captures(trimmed) {
                if let (Ok(pitch), Ok(vel)) = (
                    u8::from_str_radix(&caps[1], 16),
                    u8::from_str_radix(&caps[2], 16),
                ) {
                    if vel > 0 {
                        current_midi_notes.push(pitch);
                    }
                }
            }
        }

        // Automation envelopes
        if RE_ENVELOPE.is_match(trimmed) {
            intel.automated_params.push(trimmed.to_string());
        }
    }

    // Build plugin list from counts
    for (key, count) in &plugin_counts {
        let parts: Vec<&str> = key.splitn(2, ':').collect();
        if parts.len() == 2 {
            intel.plugins.push(PluginInfo {
                name: parts[1].to_string(),
                plugin_type: parts[0].to_string(),
                count: *count,
            });
        }
    }

    // Build mixer info
    let total = tracks.len() as u32;
    intel.mixer = MixerInfo {
        total_tracks: total,
        audio_tracks,
        midi_tracks,
        return_tracks,
        group_tracks: 0, // Reaper doesn't have explicit group tracks
        has_sidechain,
    };

    intel.tracks = tracks;

    Ok(intel)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_RPP: &str = r#"<REAPER_PROJECT 0.1 "7.0" 1234567890
  TEMPO 120 4 4
  MARKER 1 10.5 "Verse"
  MARKER 2 45.0 "Chorus"
  <TRACK
    NAME "Bass"
    <FXCHAIN
      <VST "VST: Serum (Xfer Records)" Serum.dll
      >
    >
  >
  <TRACK
    NAME "Drums"
    <SOURCE WAVE
      FILE "kick_808.wav"
    >
  >
  <TRACK
    NAME "FX Return"
    AUXRECV 0 0 1 0 0 0 0
  >
>"#;

    #[test]
    fn parse_bpm_and_time_sig() {
        let intel = parse_content(SIMPLE_RPP).unwrap();
        assert_eq!(intel.bpm, Some(120.0));
        assert_eq!(intel.time_signature.as_deref(), Some("4/4"));
    }

    #[test]
    fn parse_version() {
        let intel = parse_content(SIMPLE_RPP).unwrap();
        assert_eq!(intel.daw_version.as_deref(), Some("7.0"));
    }

    #[test]
    fn parse_markers() {
        let intel = parse_content(SIMPLE_RPP).unwrap();
        assert_eq!(intel.markers.len(), 2);
        assert_eq!(intel.markers[0].name, "Verse");
        assert_eq!(intel.markers[1].name, "Chorus");
        assert!((intel.markers[0].position_seconds - 10.5).abs() < 0.01);
    }

    #[test]
    fn parse_tracks_and_types() {
        let intel = parse_content(SIMPLE_RPP).unwrap();
        assert_eq!(intel.tracks.len(), 3);
        assert_eq!(intel.tracks[0].name, "Bass");
        assert_eq!(intel.tracks[1].name, "Drums");
        assert_eq!(intel.tracks[2].name, "FX Return");
        assert_eq!(intel.tracks[2].track_type, "return");
    }

    #[test]
    fn parse_plugins() {
        let intel = parse_content(SIMPLE_RPP).unwrap();
        assert_eq!(intel.plugins.len(), 1);
        assert_eq!(intel.plugins[0].name, "Serum");
        assert_eq!(intel.plugins[0].plugin_type, "vst");
    }

    #[test]
    fn parse_samples() {
        let intel = parse_content(SIMPLE_RPP).unwrap();
        assert_eq!(intel.samples.len(), 1);
        assert_eq!(intel.samples[0].name, "kick_808.wav");
    }

    #[test]
    fn parse_mixer_info() {
        let intel = parse_content(SIMPLE_RPP).unwrap();
        assert_eq!(intel.mixer.total_tracks, 3);
        assert_eq!(intel.mixer.audio_tracks, 2);
        assert_eq!(intel.mixer.return_tracks, 1);
        assert!(intel.mixer.has_sidechain);
    }

    #[test]
    fn parse_rejects_out_of_range_bpm() {
        let content = r#"<REAPER_PROJECT 0.1 "7.0" 1234567890
  TEMPO 999 4 4
>"#;
        let intel = parse_content(content).unwrap();
        assert_eq!(intel.bpm, None); // 999 is out of range
    }

    #[test]
    fn parse_empty_project() {
        let content = r#"<REAPER_PROJECT 0.1 "7.0" 1234567890
>"#;
        let intel = parse_content(content).unwrap();
        assert_eq!(intel.daw, "Reaper");
        assert!(intel.tracks.is_empty());
    }

    #[test]
    fn clean_plugin_name_strips_vendor() {
        assert_eq!(
            clean_plugin_name("FabFilter Pro-Q 3 (FabFilter)"),
            "FabFilter Pro-Q 3"
        );
        assert_eq!(clean_plugin_name("Serum"), "Serum");
    }

    // ── v0.29.7: Edge-case hardening ─────────────────────────────────

    #[test]
    fn parse_empty_content() {
        let intel = parse_content("").unwrap();
        assert_eq!(intel.daw, "Reaper");
        assert!(intel.tracks.is_empty());
    }

    #[test]
    fn parse_unclosed_brackets() {
        let content = "<REAPER_PROJECT 0.1 \"7.0\" 123\n  <TRACK\n    NAME \"Orphan\"";
        assert!(parse_content(content).is_ok());
    }

    #[test]
    fn parse_binary_garbage_in_text() {
        let content = "<REAPER_PROJECT 0.1 \"7.0\" 123\n  TEMPO 120 4 4\n\x00\x01\x02\n>";
        assert!(parse_content(content).is_ok());
    }

    #[test]
    fn parse_extremely_long_track_name() {
        let long_name = "A".repeat(10000);
        let content = format!(
            "<REAPER_PROJECT 0.1 \"7.0\" 123\n  <TRACK\n    NAME \"{}\"\n  >\n>",
            long_name
        );
        let intel = parse_content(&content).unwrap();
        assert_eq!(intel.tracks[0].name, long_name);
    }
}
