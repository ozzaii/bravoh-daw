//! Ableton Live .als project file parser.
//!
//! .als files are gzip-compressed XML. We decompress with flate2,
//! then parse the XML with quick-xml's event-based reader.
//! Matches the Python backend parser's extraction logic.

use super::{MarkerInfo, MixerInfo, ParsedIntelligence, PluginInfo, SampleInfo, TrackInfo};
use flate2::read::GzDecoder;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::path::Path;

/// Max uncompressed XML size (5 MB).
const MAX_UNCOMPRESSED: usize = 5 * 1024 * 1024;

/// VST/AU wrapper element names — anything else inside <Devices> is a native Ableton device.
const VST_WRAPPER_TAGS: &[&str] = &[
    "PluginDevice",
    "AuPluginDevice",
    "Vst3PluginDevice",
    "MxPatchRef",
];

/// Track element tag → track type mapping.
fn track_type_for_tag(tag: &str) -> Option<&'static str> {
    match tag {
        "AudioTrack" => Some("audio"),
        "MidiTrack" => Some("midi"),
        "ReturnTrack" => Some("return"),
        "GroupTrack" => Some("group"),
        _ => None,
    }
}

/// Read and decompress a .als file, then parse the XML content.
pub fn parse(path: &Path) -> Result<ParsedIntelligence, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Open: {}", e))?;
    let decoder = GzDecoder::new(file);
    let mut reader = BufReader::new(decoder);

    let mut xml_bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_UNCOMPRESSED as u64)
        .read_to_end(&mut xml_bytes)
        .map_err(|e| format!("Decompress: {}", e))?;

    if xml_bytes.len() >= MAX_UNCOMPRESSED {
        return Err(format!(
            "Uncompressed XML exceeds {} byte limit",
            MAX_UNCOMPRESSED
        ));
    }

    parse_xml_content(&xml_bytes)
}

/// Parse raw Ableton XML bytes into structured intelligence.
/// Public for unit tests (pass raw XML strings without gzip).
pub fn parse_xml_content(xml: &[u8]) -> Result<ParsedIntelligence, String> {
    let mut intel = ParsedIntelligence {
        daw: "Ableton Live".to_string(),
        ..Default::default()
    };

    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();

    // Element stack for context tracking
    let mut stack: Vec<String> = Vec::new();

    // Track state
    let mut in_track = false;
    let mut current_track_type: Option<&'static str> = None;
    let mut current_track_name = String::new();
    let mut current_track_plugins: Vec<String> = Vec::new();

    // Plugin state
    let mut in_devices = false;
    let mut in_vst_wrapper = false;
    let mut plugin_counts: HashMap<String, u32> = HashMap::new();

    // BPM state: track whether we're inside MasterTrack/MainTrack
    let mut in_master_track = false;

    // Marker state
    let mut in_locator = false;
    let mut locator_name = String::new();
    let mut locator_time: f64 = 0.0;

    // Sample state
    let mut in_sample_ref = false;
    let mut in_file_ref = false;

    // MIDI state (v0.29.0)
    let mut in_midi_clip = false;
    let mut in_key_track = false;
    let mut midi_note_count: u32 = 0;
    let midi_track_name = String::new();

    // Sidechain state (v0.29.0)
    let mut in_compressor = false;
    let mut in_sidechain_section = false;
    let mut sidechain_on_pending = false;

    // Automation state (v0.29.0)
    let mut in_automation_envelope = false;

    // Track color (v0.29.0)
    let mut current_track_color: Option<String> = None;

    // Version
    let mut found_version = false;

    // Track counts for MixerInfo
    let mut audio_count: u32 = 0;
    let mut midi_count: u32 = 0;
    let mut return_count: u32 = 0;
    let mut group_count: u32 = 0;

    // All tracks collected
    let mut tracks: Vec<TrackInfo> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                stack.push(tag_name.clone());

                // Ableton version from root <Ableton> element
                if !found_version && tag_name == "Ableton" {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"Creator" {
                            intel.daw_version =
                                Some(String::from_utf8_lossy(&attr.value).to_string());
                            found_version = true;
                        }
                    }
                }

                // MasterTrack (Live 10/11) or MainTrack (Live 12)
                if tag_name == "MasterTrack" || tag_name == "MainTrack" {
                    in_master_track = true;
                }

                // Track elements
                if let Some(ttype) = track_type_for_tag(&tag_name) {
                    in_track = true;
                    current_track_type = Some(ttype);
                    current_track_name.clear();
                    current_track_plugins.clear();
                }

                // Devices container
                if tag_name == "Devices" && in_track {
                    in_devices = true;
                }

                // VST/AU wrapper inside Devices
                if in_devices && VST_WRAPPER_TAGS.contains(&tag_name.as_str()) {
                    in_vst_wrapper = true;
                }

                // Native device: inside <Devices> but NOT a VST wrapper and not a child of one
                if in_devices && !in_vst_wrapper && tag_name != "Devices" {
                    // Check if direct child of Devices by seeing if the parent is "Devices"
                    if stack.len() >= 2 && stack[stack.len() - 2] == "Devices" {
                        let pname = tag_name.clone();
                        current_track_plugins.push(pname.clone());
                        let key = format!("native:{}", pname);
                        *plugin_counts.entry(key).or_insert(0) += 1;
                    }
                }

                // MidiClip for MIDI note counting (v0.29.0)
                if tag_name == "MidiClip" {
                    in_midi_clip = true;
                    midi_note_count = 0;
                }
                if tag_name == "KeyTrack" && in_midi_clip {
                    in_key_track = true;
                }
                if tag_name == "MidiNoteEvent" && in_key_track {
                    midi_note_count += 1;
                }

                // Sidechain detection (v0.29.0)
                if tag_name == "Compressor" || tag_name == "GlueCompressor" || tag_name == "Gate" {
                    in_compressor = true;
                }
                if in_compressor && (tag_name == "Sidechain" || tag_name == "SideChain") {
                    in_sidechain_section = true;
                }

                // Automation envelope detection (v0.29.0)
                if tag_name == "AutomationEnvelope" {
                    in_automation_envelope = true;
                }

                // Locator for markers
                if tag_name == "Locator" {
                    in_locator = true;
                    locator_name.clear();
                    locator_time = 0.0;
                }

                // SampleRef / FileRef for samples
                if tag_name == "SampleRef" {
                    in_sample_ref = true;
                }
                if tag_name == "FileRef" && in_sample_ref {
                    in_file_ref = true;
                }
            }

            Ok(Event::Empty(ref e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                // MIDI note event as self-closing tag: <MidiNoteEvent/>
                if tag_name == "MidiNoteEvent" && in_key_track {
                    midi_note_count += 1;
                }

                // Native device as self-closing tag: <Eq8/> inside <Devices>
                if in_devices
                    && !in_vst_wrapper
                    && tag_name != "Devices"
                    && stack.last().map(|s| s.as_str()) == Some("Devices")
                {
                    current_track_plugins.push(tag_name.clone());
                    let key = format!("native:{}", tag_name);
                    *plugin_counts.entry(key).or_insert(0) += 1;
                }

                // BPM: <Manual Value="128.0"/> inside <Tempo>
                if tag_name == "Manual" {
                    let parent = stack.last().map(|s| s.as_str());
                    if parent == Some("Tempo") {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"Value" {
                                if let Ok(val) = String::from_utf8_lossy(&attr.value).parse::<f64>()
                                {
                                    if (30.0..=300.0).contains(&val) {
                                        // Only set BPM from master/main track level or top-level
                                        // (before any track context, or inside master track)
                                        if in_master_track || !in_track {
                                            intel.bpm = Some(val);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Track name: <EffectiveName Value="..."/> inside <Name>
                // or <UserName Value="..."/>
                if in_track && current_track_name.is_empty() {
                    if tag_name == "EffectiveName" {
                        let parent = stack.last().map(|s| s.as_str());
                        if parent == Some("Name") {
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"Value" {
                                    let val = String::from_utf8_lossy(&attr.value).to_string();
                                    if !val.is_empty() {
                                        current_track_name = val;
                                    }
                                }
                            }
                        }
                    }
                    if current_track_name.is_empty() && tag_name == "UserName" {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"Value" {
                                let val = String::from_utf8_lossy(&attr.value).to_string();
                                if !val.is_empty() {
                                    current_track_name = val;
                                }
                            }
                        }
                    }
                }

                // Plugin name from VST wrapper: <PlugName Value="Serum"/>
                if in_vst_wrapper && (tag_name == "PlugName" || tag_name == "UserName") {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"Value" {
                            let pname = String::from_utf8_lossy(&attr.value).to_string();
                            if !pname.is_empty() {
                                current_track_plugins.push(pname.clone());
                                let key = format!("vst:{}", pname);
                                *plugin_counts.entry(key).or_insert(0) += 1;
                            }
                        }
                    }
                }

                // Locator fields
                if in_locator {
                    if tag_name == "Name" {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"Value" {
                                locator_name = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    if tag_name == "Time" {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"Value" {
                                locator_time =
                                    String::from_utf8_lossy(&attr.value).parse().unwrap_or(0.0);
                            }
                        }
                    }
                }

                // Sidechain On: <Manual Value="true"/> inside <On> inside <Sidechain> (v0.29.0)
                if in_sidechain_section && tag_name == "Manual" {
                    let parent = stack.last().map(|s| s.as_str());
                    if parent == Some("On") {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"Value" {
                                let val = String::from_utf8_lossy(&attr.value);
                                if val == "true" {
                                    sidechain_on_pending = true;
                                }
                            }
                        }
                    }
                }

                // Track color: <Color Value="N"/> (v0.29.0)
                if in_track && tag_name == "Color" && current_track_color.is_none() {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"Value" {
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            if !val.is_empty() {
                                current_track_color = Some(format!("ableton_color_{}", val));
                            }
                        }
                    }
                }

                // Automation: <PointeeId Value="N"/> (v0.29.0)
                if in_automation_envelope && tag_name == "PointeeId" {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"Value" {
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            if !val.is_empty() && !intel.automated_params.contains(&val) {
                                intel.automated_params.push(val);
                            }
                        }
                    }
                }

                // Sample name: <Name Value="kick.wav"/> inside <FileRef> inside <SampleRef>
                if in_file_ref && tag_name == "Name" {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"Value" {
                            let sname = String::from_utf8_lossy(&attr.value).to_string();
                            if !sname.is_empty() {
                                intel.samples.push(SampleInfo {
                                    name: sname,
                                    path: None,
                                });
                            }
                        }
                    }
                }
            }

            Ok(Event::End(ref e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                // Close MasterTrack / MainTrack
                if tag_name == "MasterTrack" || tag_name == "MainTrack" {
                    in_master_track = false;
                }

                // Close MidiClip — emit MIDI track info (v0.29.0)
                if tag_name == "MidiClip" {
                    if in_midi_clip && midi_note_count > 0 {
                        let track_name = if midi_track_name.is_empty() {
                            current_track_name.clone()
                        } else {
                            midi_track_name.clone()
                        };
                        intel.midi_tracks.push(super::MidiTrackInfo {
                            track_name,
                            note_count: midi_note_count,
                            pitch_low: None,
                            pitch_high: None,
                        });
                    }
                    in_midi_clip = false;
                    in_key_track = false;
                    midi_note_count = 0;
                }
                if tag_name == "KeyTrack" {
                    in_key_track = false;
                }

                // Close Compressor/Sidechain (v0.29.0)
                if tag_name == "Compressor" || tag_name == "GlueCompressor" || tag_name == "Gate" {
                    if sidechain_on_pending {
                        intel.mixer.has_sidechain = true;
                        sidechain_on_pending = false;
                    }
                    in_compressor = false;
                    in_sidechain_section = false;
                }
                if tag_name == "Sidechain" || tag_name == "SideChain" {
                    in_sidechain_section = false;
                }

                // Close AutomationEnvelope (v0.29.0)
                if tag_name == "AutomationEnvelope" {
                    in_automation_envelope = false;
                }

                // Close track
                if let Some(ttype) = track_type_for_tag(&tag_name) {
                    if in_track && current_track_type == Some(ttype) {
                        // Count track type
                        match ttype {
                            "audio" => audio_count += 1,
                            "midi" => midi_count += 1,
                            "return" => return_count += 1,
                            "group" => group_count += 1,
                            _ => {}
                        }

                        tracks.push(TrackInfo {
                            name: current_track_name.clone(),
                            track_type: ttype.to_string(),
                            plugins: current_track_plugins.clone(),
                            color: current_track_color.take(),
                        });

                        in_track = false;
                        current_track_type = None;
                        in_devices = false;
                        in_vst_wrapper = false;
                    }
                }

                // Close Devices
                if tag_name == "Devices" {
                    in_devices = false;
                    in_vst_wrapper = false;
                }

                // Close VST wrapper
                if VST_WRAPPER_TAGS.contains(&tag_name.as_str()) {
                    in_vst_wrapper = false;
                }

                // Close Locator — emit marker
                if tag_name == "Locator" {
                    if in_locator && !locator_name.is_empty() {
                        intel.markers.push(MarkerInfo {
                            name: locator_name.clone(),
                            position_seconds: locator_time,
                        });
                    }
                    in_locator = false;
                }

                // Close SampleRef / FileRef
                if tag_name == "FileRef" {
                    in_file_ref = false;
                }
                if tag_name == "SampleRef" {
                    in_sample_ref = false;
                }

                stack.pop();
            }

            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error: {}", e)),
            _ => {}
        }

        buf.clear();
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

    // Deduplicate samples by name
    let mut seen_samples: HashMap<String, bool> = HashMap::new();
    intel.samples.retain(|s| {
        if seen_samples.contains_key(&s.name) {
            false
        } else {
            seen_samples.insert(s.name.clone(), true);
            true
        }
    });

    // Build MixerInfo
    let total = tracks.len() as u32;
    intel.mixer = MixerInfo {
        total_tracks: total,
        audio_tracks: audio_count,
        midi_tracks: midi_count,
        return_tracks: return_count,
        group_tracks: group_count,
        has_sidechain: intel.mixer.has_sidechain, // Detected from Compressor/Gate Sidechain On
    };

    intel.tracks = tracks;

    Ok(intel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bpm() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <MasterTrack>
            <DeviceChain>
                <Mixer>
                    <Tempo>
                        <Manual Value="128"/>
                    </Tempo>
                </Mixer>
            </DeviceChain>
        </MasterTrack>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert_eq!(intel.bpm, Some(128.0));
        assert_eq!(intel.daw, "Ableton Live");
        assert_eq!(intel.daw_version.as_deref(), Some("Ableton Live 11.3.2"));
    }

    #[test]
    fn parse_bpm_live12_main_track() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 12.0">
    <LiveSet>
        <MainTrack>
            <DeviceChain>
                <Mixer>
                    <Tempo>
                        <Manual Value="140.5"/>
                    </Tempo>
                </Mixer>
            </DeviceChain>
        </MainTrack>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert_eq!(intel.bpm, Some(140.5));
    }

    #[test]
    fn parse_bpm_rejects_out_of_range() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <MasterTrack>
            <Tempo>
                <Manual Value="999"/>
            </Tempo>
        </MasterTrack>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert_eq!(intel.bpm, None);
    }

    #[test]
    fn parse_tracks() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <Tracks>
            <AudioTrack>
                <Name>
                    <EffectiveName Value="Drums"/>
                </Name>
            </AudioTrack>
            <MidiTrack>
                <Name>
                    <EffectiveName Value="Synth Lead"/>
                </Name>
            </MidiTrack>
            <ReturnTrack>
                <Name>
                    <EffectiveName Value="Reverb Bus"/>
                </Name>
            </ReturnTrack>
            <GroupTrack>
                <Name>
                    <EffectiveName Value="Drums Group"/>
                </Name>
            </GroupTrack>
        </Tracks>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert_eq!(intel.tracks.len(), 4);

        assert_eq!(intel.tracks[0].name, "Drums");
        assert_eq!(intel.tracks[0].track_type, "audio");

        assert_eq!(intel.tracks[1].name, "Synth Lead");
        assert_eq!(intel.tracks[1].track_type, "midi");

        assert_eq!(intel.tracks[2].name, "Reverb Bus");
        assert_eq!(intel.tracks[2].track_type, "return");

        assert_eq!(intel.tracks[3].name, "Drums Group");
        assert_eq!(intel.tracks[3].track_type, "group");

        assert_eq!(intel.mixer.total_tracks, 4);
        assert_eq!(intel.mixer.audio_tracks, 1);
        assert_eq!(intel.mixer.midi_tracks, 1);
        assert_eq!(intel.mixer.return_tracks, 1);
        assert_eq!(intel.mixer.group_tracks, 1);
    }

    #[test]
    fn parse_plugins_native() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <Tracks>
            <AudioTrack>
                <Name>
                    <EffectiveName Value="Bass"/>
                </Name>
                <DeviceChain>
                    <Devices>
                        <Eq8/>
                        <Compressor/>
                    </Devices>
                </DeviceChain>
            </AudioTrack>
        </Tracks>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();

        // Track should have the native plugins listed
        assert_eq!(intel.tracks.len(), 1);
        assert!(intel.tracks[0].plugins.contains(&"Eq8".to_string()));
        assert!(intel.tracks[0].plugins.contains(&"Compressor".to_string()));

        // Global plugin list
        assert_eq!(intel.plugins.len(), 2);
        let names: Vec<&str> = intel.plugins.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Eq8"));
        assert!(names.contains(&"Compressor"));
        for p in &intel.plugins {
            assert_eq!(p.plugin_type, "native");
        }
    }

    #[test]
    fn parse_plugins_vst() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <Tracks>
            <MidiTrack>
                <Name>
                    <EffectiveName Value="Lead"/>
                </Name>
                <DeviceChain>
                    <Devices>
                        <PluginDevice>
                            <PluginDesc>
                                <VstPluginInfo>
                                    <PlugName Value="Serum"/>
                                </VstPluginInfo>
                            </PluginDesc>
                        </PluginDevice>
                        <AuPluginDevice>
                            <PluginDesc>
                                <AuPluginInfo>
                                    <UserName Value="FabFilter Pro-Q 3"/>
                                </AuPluginInfo>
                            </PluginDesc>
                        </AuPluginDevice>
                    </Devices>
                </DeviceChain>
            </MidiTrack>
        </Tracks>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();

        assert_eq!(intel.tracks.len(), 1);
        assert!(intel.tracks[0].plugins.contains(&"Serum".to_string()));
        assert!(intel.tracks[0]
            .plugins
            .contains(&"FabFilter Pro-Q 3".to_string()));

        let names: Vec<&str> = intel.plugins.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Serum"));
        assert!(names.contains(&"FabFilter Pro-Q 3"));
        for p in &intel.plugins {
            assert_eq!(p.plugin_type, "vst");
        }
    }

    #[test]
    fn parse_markers() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <Locators>
            <Locators>
                <Locator>
                    <Name Value="Intro"/>
                    <Time Value="0.0"/>
                </Locator>
                <Locator>
                    <Name Value="Chorus"/>
                    <Time Value="45.0"/>
                </Locator>
            </Locators>
        </Locators>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert_eq!(intel.markers.len(), 2);
        assert_eq!(intel.markers[0].name, "Intro");
        assert!((intel.markers[0].position_seconds - 0.0).abs() < 0.01);
        assert_eq!(intel.markers[1].name, "Chorus");
        assert!((intel.markers[1].position_seconds - 45.0).abs() < 0.01);
    }

    #[test]
    fn parse_samples() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <Tracks>
            <AudioTrack>
                <Name>
                    <EffectiveName Value="Drums"/>
                </Name>
                <DeviceChain>
                    <MainSequencer>
                        <ClipSlotList>
                            <ClipSlot>
                                <ClipSlot>
                                    <Value>
                                        <AudioClip>
                                            <SampleRef>
                                                <FileRef>
                                                    <Name Value="kick.wav"/>
                                                </FileRef>
                                            </SampleRef>
                                        </AudioClip>
                                    </Value>
                                </ClipSlot>
                            </ClipSlot>
                        </ClipSlotList>
                    </MainSequencer>
                </DeviceChain>
            </AudioTrack>
        </Tracks>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert_eq!(intel.samples.len(), 1);
        assert_eq!(intel.samples[0].name, "kick.wav");
    }

    #[test]
    fn parse_empty_project() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert_eq!(intel.daw, "Ableton Live");
        assert!(intel.tracks.is_empty());
        assert!(intel.plugins.is_empty());
        assert!(intel.samples.is_empty());
        assert!(intel.markers.is_empty());
        assert_eq!(intel.mixer.total_tracks, 0);
    }

    #[test]
    fn parse_track_name_fallback_to_username() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <Tracks>
            <AudioTrack>
                <Name>
                    <EffectiveName Value=""/>
                    <UserName Value="My Custom Name"/>
                </Name>
            </AudioTrack>
        </Tracks>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert_eq!(intel.tracks.len(), 1);
        assert_eq!(intel.tracks[0].name, "My Custom Name");
    }

    #[test]
    fn parse_midi_note_count() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <Tracks>
            <MidiTrack>
                <Name><EffectiveName Value="Piano"/></Name>
                <DeviceChain>
                    <MainSequencer>
                        <ClipSlotList>
                            <ClipSlot><ClipSlot><Value>
                                <MidiClip>
                                    <Notes>
                                        <KeyTracks>
                                            <KeyTrack>
                                                <MidiKey Value="60"/>
                                                <Notes>
                                                    <MidiNoteEvent/>
                                                    <MidiNoteEvent/>
                                                    <MidiNoteEvent/>
                                                </Notes>
                                            </KeyTrack>
                                        </KeyTracks>
                                    </Notes>
                                </MidiClip>
                            </Value></ClipSlot></ClipSlot>
                        </ClipSlotList>
                    </MainSequencer>
                </DeviceChain>
            </MidiTrack>
        </Tracks>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert!(!intel.midi_tracks.is_empty(), "Should have MIDI track info");
        assert_eq!(intel.midi_tracks[0].note_count, 3);
    }

    #[test]
    fn parse_sidechain_detection() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <Tracks>
            <AudioTrack>
                <Name><EffectiveName Value="Bass"/></Name>
                <DeviceChain>
                    <Devices>
                        <Compressor>
                            <Sidechain>
                                <On>
                                    <Manual Value="true"/>
                                </On>
                            </Sidechain>
                        </Compressor>
                    </Devices>
                </DeviceChain>
            </AudioTrack>
        </Tracks>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert!(intel.mixer.has_sidechain, "Should detect sidechain");
    }

    #[test]
    fn parse_track_color() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <Tracks>
            <AudioTrack>
                <Color Value="13"/>
                <Name><EffectiveName Value="Drums"/></Name>
            </AudioTrack>
        </Tracks>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert_eq!(intel.tracks[0].color.as_deref(), Some("ableton_color_13"));
    }

    #[test]
    fn parse_automation_pointee_id() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <Tracks>
            <AudioTrack>
                <Name><EffectiveName Value="Lead"/></Name>
                <AutomationEnvelopes>
                    <Envelopes>
                        <AutomationEnvelope>
                            <PointeeId Value="42"/>
                        </AutomationEnvelope>
                        <AutomationEnvelope>
                            <PointeeId Value="77"/>
                        </AutomationEnvelope>
                    </Envelopes>
                </AutomationEnvelopes>
            </AudioTrack>
        </Tracks>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert_eq!(intel.automated_params.len(), 2);
        assert!(intel.automated_params.contains(&"42".to_string()));
        assert!(intel.automated_params.contains(&"77".to_string()));
    }

    #[test]
    fn parse_no_sidechain_when_off() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <Tracks>
            <AudioTrack>
                <Name><EffectiveName Value="Bass"/></Name>
                <DeviceChain>
                    <Devices>
                        <Compressor>
                            <Sidechain>
                                <On>
                                    <Manual Value="false"/>
                                </On>
                            </Sidechain>
                        </Compressor>
                    </Devices>
                </DeviceChain>
            </AudioTrack>
        </Tracks>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert!(
            !intel.mixer.has_sidechain,
            "Sidechain Off should not be detected"
        );
    }

    #[test]
    fn parse_deduplicates_samples() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 11.3.2">
    <LiveSet>
        <Tracks>
            <AudioTrack>
                <Name><EffectiveName Value="Drums"/></Name>
                <DeviceChain>
                    <MainSequencer>
                        <SampleRef><FileRef><Name Value="kick.wav"/></FileRef></SampleRef>
                        <SampleRef><FileRef><Name Value="kick.wav"/></FileRef></SampleRef>
                        <SampleRef><FileRef><Name Value="snare.wav"/></FileRef></SampleRef>
                    </MainSequencer>
                </DeviceChain>
            </AudioTrack>
        </Tracks>
    </LiveSet>
</Ableton>"#;
        let intel = parse_xml_content(xml).unwrap();
        assert_eq!(intel.samples.len(), 2);
        let names: Vec<&str> = intel.samples.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"kick.wav"));
        assert!(names.contains(&"snare.wav"));
    }

    // ── v0.29.7: Edge-case hardening ─────────────────────────────────

    #[test]
    fn parse_empty_xml() {
        let result = parse_xml_content(b"");
        assert!(result.is_ok());
        assert!(result.unwrap().tracks.is_empty());
    }

    #[test]
    fn parse_non_ableton_xml() {
        let xml = br#"<?xml version="1.0"?><Root><Child/></Root>"#;
        let result = parse_xml_content(xml);
        assert!(result.is_ok());
    }

    #[test]
    fn parse_truncated_xml_no_panic() {
        let result = parse_xml_content(b"<Ableton Creator=\"Test\"><LiveSet><Tracks><AudioTrack>");
        // Truncated — may error or produce partial result, but must not panic
        let _ = result;
    }

    #[test]
    fn parse_deeply_nested_xml() {
        let mut xml = String::from("<?xml version=\"1.0\"?><Ableton Creator=\"Test\">");
        for _ in 0..100 {
            xml.push_str("<Nested>");
        }
        for _ in 0..100 {
            xml.push_str("</Nested>");
        }
        xml.push_str("</Ableton>");
        assert!(parse_xml_content(xml.as_bytes()).is_ok());
    }
}
