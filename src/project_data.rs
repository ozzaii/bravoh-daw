//! Logic Pro ProjectData binary parser.
//!
//! Extracts production intelligence from Logic's binary ProjectData file
//! via string scanning and embedded JSON parsing. No reverse engineering needed —
//! Logic embeds readable strings (plugin names, sample names, track names)
//! and JSON blobs (beat grids with BPM, time signatures, onset timing).

use regex::Regex;
use serde::Serialize;
use std::path::Path;
use std::sync::LazyLock;

const MAX_PROJECT_DATA_BYTES: u64 = 10 * 1024 * 1024; // 10MB

#[derive(Debug, Clone, Serialize, Default)]
pub struct ProjectIntelligence {
    /// BPM extracted from embedded JSON beat grid
    pub bpm: Option<f64>,
    /// Time signature (e.g. "4/4")
    pub time_signature: Option<String>,
    /// Track duration in seconds from beat grid analysis
    pub duration_seconds: Option<f64>,
    /// Swing percentage: 0% = straight, 67% = triplet swing
    pub swing_percentage: Option<f64>,
    /// Plugin names found in the project
    pub plugins: Vec<PluginCount>,
    /// Sample/one-shot names used in drum kits and samplers
    pub samples: Vec<String>,
    /// Section markers (verse, chorus, bridge, etc.)
    pub markers: Vec<String>,
    /// Hardware emulation plugins detected
    pub hardware_emulations: Vec<String>,
    /// Track-like names found
    pub track_hints: Vec<String>,
    /// Drummer groove template names (DD_GR_*)
    pub groove_templates: Vec<String>,
    /// Channel strip / synth patch names
    pub synth_patches: Vec<String>,
    /// Automation node count
    pub automation_count: u32,
    /// Whether Lead Sheet / Score editor was used
    pub has_lead_sheet: bool,
    /// Routing intelligence: buses, sends, signal flow
    pub routing_hints: RoutingHints,
    /// Logic Drummer character name (e.g. "Acoustic Drummer - Pop Rock")
    pub drummer_character: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RoutingHints {
    /// Bus numbers detected (e.g. Bus 1, Bus 3)
    pub buses_used: Vec<u32>,
    /// Total number of distinct buses
    pub bus_count: u32,
    /// Number of send-related strings (Send Level, Send Pan, Send On/Off)
    pub send_count: u32,
    /// Whether any routing (buses or sends) was detected
    pub has_routing: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginCount {
    pub name: String,
    pub count: u32,
}

/// Parse a Logic Pro ProjectData binary file and extract production intelligence.
pub fn parse_project_data(path: &Path) -> Option<ProjectIntelligence> {
    // Skip files larger than 10MB to avoid memory pressure
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > MAX_PROJECT_DATA_BYTES {
            tracing::warn!(
                "ProjectData too large ({} bytes), skipping: {:?}",
                meta.len(),
                path
            );
            return None;
        }
    }
    // Read into a temporary block so `bytes` is dropped as soon as `full_text` is
    // produced, keeping peak allocation to ~2× file size instead of holding both
    // the raw Vec<u8> and the lossy String simultaneously for the full parse pass.
    let (strings, full_text) = {
        let bytes = std::fs::read(path).ok()?;
        let strings = extract_strings(&bytes, 4);
        let full_text = String::from_utf8_lossy(&bytes).into_owned();
        (strings, full_text)
        // `bytes` is dropped here
    };
    let mut intel = ProjectIntelligence::default();

    // 1. BPM / time signature / duration / swing — DROPPED.
    // Logic's embedded JSON "tempo" is from Smart Tempo beat detection,
    // NOT the project tempo. Verified wrong in 3/6 test projects.
    // These fields are left as None — audio analysis provides reliable BPM.
    let _full_text = full_text.as_str();
    // extract_tempo_from_json/extract_duration_and_swing — DROPPED.
    // Logic's embedded JSON "tempo" is Smart Tempo beat detection, NOT project tempo.
    // Verified wrong in 3/6 test projects. Audio analysis provides reliable BPM.

    // 2. Extract plugins
    let plugin_names = [
        "Channel EQ",
        "Compressor",
        "Limiter",
        "Multipressor",
        "Space Designer",
        "ChromaVerb",
        "SilverVerb",
        "Alchemy",
        "Retro Synth",
        "ES2",
        "EXS24",
        "Sampler",
        "Ultrabeat",
        "Drum Kit Designer",
        "Drum Machine Designer",
        "AutoFilter",
        "Ringshifter",
        "Vocal Transformer",
        "Pitch Shifter",
        "Chorus",
        "Flanger",
        "Phaser",
        "Tremolo",
        "Distortion",
        "Overdrive",
        "Clip Distortion",
        "Bitcrusher",
        "Tape Delay",
        "Stereo Delay",
        "Echo",
        "Noise Gate",
        "DeEsser",
        "Exciter",
        "Gain",
        "Test Oscillator",
        "Tuner",
        "Rhino Reverb",
        "Camel Strip",
    ];

    let mut plugin_counts: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    for s in &strings {
        for plugin in &plugin_names {
            if s.starts_with(plugin) && s.len() < plugin.len() + 5 {
                *plugin_counts.entry(plugin.to_string()).or_insert(0) += 1;
            }
        }
    }
    intel.plugins = plugin_counts
        .into_iter()
        .map(|(name, count)| PluginCount { name, count })
        .collect();
    intel.plugins.sort_by(|a, b| b.count.cmp(&a.count));

    // 3. Extract hardware emulations (premium channel strips)
    let hw_names = [
        "Avalon VT-737",
        "Avalon VT-747SP",
        "SSL E Channel",
        "SSL G Channel",
        "Neve 1073",
        "API 550",
        "Manley VOXBOX",
        "Universal Audio",
        "Pultec EQP-1A",
        "LA-2A",
        "1176",
        "Fairchild 670",
    ];
    for s in &strings {
        for hw in &hw_names {
            if s.contains(hw) && !intel.hardware_emulations.contains(&hw.to_string()) {
                intel.hardware_emulations.push(hw.to_string());
            }
        }
    }

    // 4. Extract sample names (drum kits, one-shots)
    for s in &strings {
        let lower = s.to_lowercase();
        if (lower.contains("kick")
            || lower.contains("snare")
            || lower.contains("hat")
            || lower.contains("clap")
            || lower.contains("tom")
            || lower.contains("perc")
            || lower.contains("808")
            || lower.contains("one_shot")
            || lower.contains("drum"))
            && s.len() > 8
            && !s.contains("NSD")
            && !s.contains("NSO")
            && !s.contains("{")
            && !s.contains("customLabel")
            && !s.starts_with("Automatic")
            && !s.starts_with("DD_GR_")
            && !s.contains("kDisplay")
            && !s.contains("Track ")
            && !s.contains("=")
            && !s.contains("+")
            && !s.contains("/")
            && !s.contains("*")
            && !s.contains("contentTag")
            && !s.starts_with("GM ")
        {
            // Strip Logic's region numbering (.1, .2, etc.)
            let clean = s.split('.').next().unwrap_or(s).to_string();
            if !intel.samples.contains(&clean) && clean.len() > 5 {
                intel.samples.push(clean);
            }
        }
    }

    // 5. Extract section markers
    let marker_names = [
        "Verse",
        "Chorus",
        "Bridge",
        "Intro",
        "Outro",
        "Hook",
        "Drop",
        "Break",
        "Pre-Chorus",
        "Interlude",
        "Buildup",
        "Breakdown",
    ];
    for s in &strings {
        for marker in &marker_names {
            if (s == *marker || s.starts_with(&format!("{} ", marker)))
                && !intel.markers.contains(&marker.to_string())
            {
                intel.markers.push(marker.to_string());
            }
        }
    }

    // 6. Extract track hints
    for s in &strings {
        if (s.starts_with("Audio ")
            || s.starts_with("Inst ")
            || s.starts_with("Aux ")
            || s.starts_with("Bus ")
            || s.starts_with("Track "))
            && s.len() < 30
            && !intel.track_hints.contains(s)
        {
            intel.track_hints.push(s.clone());
        }
    }

    // 8. Extract groove templates (DD_GR_*)
    let mut groove_seen = std::collections::HashSet::new();
    for s in &strings {
        if s.starts_with("DD_GR_") {
            // Strip region numbering (.1, .2, etc.)
            let name = s
                .split('.')
                .next()
                .unwrap_or(s)
                .trim_end_matches('_')
                .to_string();
            if name.len() > 8 && groove_seen.insert(name.clone()) {
                intel.groove_templates.push(name);
            }
        }
    }

    // 9. Extract synth patches (Automatic-*)
    let mut patch_seen = std::collections::HashSet::new();
    for s in &strings {
        let cleaned = s.trim_start_matches(|c: char| !c.is_alphabetic());
        if cleaned.starts_with("Automatic-") && cleaned.len() > 12 {
            let name = cleaned.trim_start_matches("Automatic-").trim().to_string();
            if !name.is_empty() && patch_seen.insert(name.clone()) {
                intel.synth_patches.push(name);
            }
        }
    }

    // 10. Count automation nodes
    intel.automation_count = u32::try_from(
        strings
            .iter()
            .filter(|s| s.contains("Automation") && s.len() < 40)
            .count(),
    )
    .unwrap_or(u32::MAX);

    // 11. Detect Lead Sheet
    intel.has_lead_sheet = strings.iter().any(|s| s == "Lead Sheet");

    // 12. Duration/swing — DROPPED (derived from same unreliable beat grid).
    // extract_duration_and_swing(full_text, &mut intel);

    // 13. Extract routing hints (buses, sends)
    intel.routing_hints = extract_routing_hints(&strings);

    Some(intel)
}

/// Extract BPM and time signature from embedded JSON in the binary.
/// DROPPED: Logic's Smart Tempo ≠ project tempo. Kept for future reference.
#[allow(dead_code)]
fn extract_tempo_from_json(text: &str, intel: &mut ProjectIntelligence) {
    // Logic embeds JSON blobs with beat grid data
    // Look for "Tempo":[{"t":0,"conf":-1,"tempo":111}]
    if let Some(pos) = text.find("\"Tempo\":[{") {
        let after = &text[pos..];
        if let Some(tempo_pos) = after.find("\"tempo\":") {
            let val_start = tempo_pos + 8;
            let val_end = after[val_start..]
                .find(|c: char| !c.is_ascii_digit() && c != '.')
                .map(|i| val_start + i)
                .unwrap_or(val_start);
            if let Ok(bpm) = after[val_start..val_end].parse::<f64>() {
                intel.bpm = Some(bpm);
            }
        }
    }

    // Look for time signature
    if let Some(pos) = text.find("\"signature\":\"") {
        let val_start = pos + 13;
        let val_end = text[val_start..]
            .find('"')
            .map(|i| val_start + i)
            .unwrap_or(val_start);
        let sig = &text[val_start..val_end];
        if sig.contains('/') {
            intel.time_signature = Some(sig.to_string());
        }
    }

    // Look for Logic Drummer character (selectedCharacterIdentifier)
    if let Some(pos) = text.find("\"selectedCharacterIdentifier\":\"") {
        let val_start = pos + 31;
        let val_end = text[val_start..]
            .find('"')
            .map(|i| val_start + i)
            .unwrap_or(val_start);
        let character = &text[val_start..val_end];
        if !character.is_empty() {
            intel.drummer_character = Some(character.to_string());
        }
    }
}

/// Extract duration and swing feel from embedded JSON beat grid.
/// DROPPED: Same unreliable Smart Tempo data. Kept for future reference.
#[allow(dead_code)]
fn extract_duration_and_swing(text: &str, intel: &mut ProjectIntelligence) {
    // Find the JSON blob: {"version":2,"rec_ctx"...}
    let Some(start) = text.find("{\"version\":2,\"rec_ctx\"") else {
        return;
    };

    // Find matching closing brace
    let mut depth: i32 = 0;
    let mut end = start;
    for (i, c) in text[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }

    let blob = &text[start..end];

    // Extract duration (simple regex-like scan)
    if let Some(dur_pos) = blob.find("\"duration\":") {
        let val_start = dur_pos + 11;
        let val_end = blob[val_start..]
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .map(|i| val_start + i)
            .unwrap_or(val_start);
        if let Ok(dur) = blob[val_start..val_end].parse::<f64>() {
            if dur > 0.0 {
                intel.duration_seconds = Some((dur * 100.0).round() / 100.0);
            }
        }
    }

    // Extract beat timestamps for swing analysis
    let mut times: Vec<f64> = Vec::new();
    let mut search_start = 0;
    while let Some(pos) = blob[search_start..].find("\"t\":") {
        let abs_pos = search_start + pos + 4;
        let val_end = blob[abs_pos..]
            .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .map(|i| abs_pos + i)
            .unwrap_or(abs_pos);
        if let Ok(t) = blob[abs_pos..val_end].parse::<f64>() {
            times.push(t);
        }
        search_start = val_end;
    }

    if times.len() >= 8 {
        // Calculate intervals and detect swing
        let intervals: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
        let mut ratios: Vec<f64> = Vec::new();
        for pair in intervals.chunks(2) {
            if pair.len() == 2 && pair[0] > 0.0 && pair[1] > 0.0 {
                let short = pair[0].min(pair[1]);
                let long = pair[0].max(pair[1]);
                ratios.push(long / short);
            }
        }
        if !ratios.is_empty() {
            let avg_ratio: f64 = ratios.iter().sum::<f64>() / ratios.len() as f64;
            let swing_pct = ((avg_ratio - 1.0) * 67.0).clamp(0.0, 100.0);
            intel.swing_percentage = Some((swing_pct * 10.0).round() / 10.0);
        }
    }
}

/// Extract routing hints from project strings: bus usage and send counts.
static BUS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Bus\s+(\d+)").unwrap());

fn extract_routing_hints(strings: &[String]) -> RoutingHints {
    let bus_re = &*BUS_RE;
    let mut buses: Vec<u32> = Vec::new();
    let mut send_count: u32 = 0;

    for s in strings {
        // Detect bus usage: "Bus 1", "Bus 3", etc.
        if let Some(caps) = bus_re.captures(s) {
            if let Ok(num) = caps[1].parse::<u32>() {
                if !buses.contains(&num) {
                    buses.push(num);
                }
            }
        }

        // Count send-related strings
        if s.contains("Send Level") || s.contains("Send Pan") || s.contains("Send On/Off") {
            send_count += 1;
        }
    }

    buses.sort();
    let bus_count = buses.len() as u32;

    RoutingHints {
        has_routing: bus_count > 0 || send_count > 0,
        buses_used: buses,
        bus_count,
        send_count,
    }
}

/// Extract readable ASCII strings from binary data.
fn extract_strings(bytes: &[u8], min_len: usize) -> Vec<String> {
    let mut strings = Vec::new();
    let mut current = String::new();

    for &byte in bytes {
        if (0x20..=0x7E).contains(&byte) {
            current.push(byte as char);
        } else {
            if current.len() >= min_len {
                strings.push(current.clone());
            }
            current.clear();
        }
    }
    if current.len() >= min_len {
        strings.push(current);
    }

    strings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_strings_basic() {
        let data = b"hello\x00world\x00ab\x00longstring";
        let strings = extract_strings(data, 4);
        assert!(strings.contains(&"hello".to_string()));
        assert!(strings.contains(&"world".to_string()));
        assert!(strings.contains(&"longstring".to_string()));
        assert!(!strings.contains(&"ab".to_string())); // too short
    }

    #[test]
    fn extract_tempo_from_json_test() {
        let text = r#"stuff"Tempo":[{"t":0,"conf":-1,"tempo":128}]more"time_signatures":[{"t":0,"conf":-1,"signature":"4/4"}]"#;
        let mut intel = ProjectIntelligence::default();
        extract_tempo_from_json(text, &mut intel);
        assert_eq!(intel.bpm, Some(128.0));
        assert_eq!(intel.time_signature, Some("4/4".to_string()));
    }

    #[test]
    fn extract_drummer_character() {
        let text = "blah\"selectedCharacterIdentifier\":\"Acoustic Drummer - Pop Rock\"blah";
        let mut intel = ProjectIntelligence::default();
        extract_tempo_from_json(text, &mut intel);
        assert_eq!(
            intel.drummer_character,
            Some("Acoustic Drummer - Pop Rock".to_string())
        );
    }

    #[test]
    fn parse_nonexistent_file_returns_none() {
        let result = parse_project_data(Path::new("/nonexistent/ProjectData"));
        assert!(result.is_none());
    }

    #[test]
    fn extract_routing_hints_detects_buses() {
        let strings = vec![
            "Bus 1".to_string(),
            "Bus 3".to_string(),
            "Bus 1".to_string(), // duplicate
            "Send Level".to_string(),
            "Send Pan".to_string(),
            "Other string".to_string(),
        ];
        let hints = extract_routing_hints(&strings);
        assert!(hints.has_routing);
        assert_eq!(hints.bus_count, 2);
        assert_eq!(hints.buses_used, vec![1, 3]);
        assert_eq!(hints.send_count, 2);
    }

    #[test]
    fn extract_routing_hints_empty() {
        let strings = vec!["Channel EQ".to_string(), "Compressor".to_string()];
        let hints = extract_routing_hints(&strings);
        assert!(!hints.has_routing);
        assert_eq!(hints.bus_count, 0);
        assert!(hints.buses_used.is_empty());
        assert_eq!(hints.send_count, 0);
    }

    #[test]
    fn routing_hints_default_serializes() {
        let hints = RoutingHints::default();
        let json = serde_json::to_string(&hints).unwrap();
        assert!(json.contains("\"has_routing\":false"));
        assert!(json.contains("\"bus_count\":0"));
    }
}
