//! FL Studio .flp project file parser (v0.29.0).
//!
//! FLP files use a binary TLV (tag-length-value) event stream format.
//! We extract BPM, time signature, channel count, channel names, and plugin names.
//!
//! Event ranges:
//! - BYTE  (0..63):   1-byte value
//! - WORD  (64..127):  2-byte LE u16 value
//! - DWORD (128..191): 4-byte LE u32 value
//! - TEXT  (192..255): variable-length (4-byte LE u32 length prefix + data)

use super::{ParsedIntelligence, PluginInfo, TrackInfo};
use std::path::Path;

/// FLP event IDs.
const EVENT_TIME_SIG_NUM: u8 = 17; // BYTE: time signature numerator
const EVENT_TIME_SIG_DEN: u8 = 18; // BYTE: time signature denominator
const EVENT_NEW_CHANNEL: u8 = 64; // WORD: new channel marker
const EVENT_BPM: u8 = 156; // DWORD: BPM * 1000
const EVENT_CHANNEL_NAME: u8 = 192; // TEXT: channel name (UTF-16LE in newer FLPs)
const EVENT_PLUGIN_NAME: u8 = 201; // TEXT: plugin name (UTF-16LE)

/// Max file size we'll attempt to parse (5 MB).
const MAX_FLP_BYTES: u64 = 5 * 1024 * 1024;

/// Parse an FL Studio .flp file from disk.
pub fn parse(path: &Path) -> Result<ParsedIntelligence, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("Metadata: {}", e))?;
    if meta.len() > MAX_FLP_BYTES {
        return Err(format!("File too large: {} bytes", meta.len()));
    }

    let bytes = std::fs::read(path).map_err(|e| format!("Read: {}", e))?;
    parse_bytes(&bytes)
}

/// Try to decode a TEXT event payload as UTF-16LE, falling back to UTF-8/Latin-1.
fn decode_text(data: &[u8]) -> String {
    // Try UTF-16LE first (FL Studio uses this for most text events)
    if data.len() >= 2 && data.len().is_multiple_of(2) {
        let u16_values: Vec<u16> = data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        // Strip null terminators
        let trimmed: Vec<u16> = u16_values.into_iter().take_while(|&c| c != 0).collect();
        if !trimmed.is_empty() {
            let decoded = String::from_utf16_lossy(&trimmed);
            if !decoded.is_empty() && decoded.chars().all(|c| !c.is_control() || c == '\n') {
                return decoded;
            }
        }
    }
    // Fallback: treat as UTF-8 with null-terminator stripping
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).to_string()
}

/// Parse FLP binary content directly (testable without files).
pub fn parse_bytes(bytes: &[u8]) -> Result<ParsedIntelligence, String> {
    // 1. Validate FLhd header
    if bytes.len() < 12 {
        return Err("File too short for FLP header".to_string());
    }
    if &bytes[0..4] != b"FLhd" {
        return Err("Missing FLhd magic — not an FLP file".to_string());
    }

    let header_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    let header_end = 8 + header_size;

    if bytes.len() < header_end {
        return Err("Truncated FLP header data".to_string());
    }

    // 2. Find FLdt data chunk
    if bytes.len() < header_end + 8 {
        return Err("Missing FLdt chunk".to_string());
    }
    if &bytes[header_end..header_end + 4] != b"FLdt" {
        return Err("Expected FLdt chunk after header".to_string());
    }

    let data_size = u32::from_le_bytes([
        bytes[header_end + 4],
        bytes[header_end + 5],
        bytes[header_end + 6],
        bytes[header_end + 7],
    ]) as usize;

    let data_start = header_end + 8;
    let data_end = (data_start + data_size).min(bytes.len());

    let mut intel = ParsedIntelligence {
        daw: "FL Studio".to_string(),
        ..Default::default()
    };

    let mut time_sig_num: Option<u8> = None;
    let mut time_sig_den: Option<u8> = None;
    let mut channel_count: u32 = 0;
    let mut channel_names: Vec<String> = Vec::new();
    let mut plugin_names: Vec<String> = Vec::new();

    // 3. Scan event stream
    let mut pos = data_start;
    while pos < data_end {
        if pos >= bytes.len() {
            break;
        }
        let event_id = bytes[pos];
        pos += 1;

        if event_id < 64 {
            // BYTE event: 1-byte value
            if pos >= bytes.len() {
                break;
            }
            let value = bytes[pos];
            pos += 1;

            if event_id == EVENT_TIME_SIG_NUM {
                time_sig_num = Some(value);
            } else if event_id == EVENT_TIME_SIG_DEN {
                time_sig_den = Some(value);
            }
        } else if event_id < 128 {
            // WORD event: 2-byte LE u16 value
            if pos + 2 > bytes.len() {
                break;
            }
            let _value = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]);
            pos += 2;

            if event_id == EVENT_NEW_CHANNEL {
                channel_count += 1;
            }
        } else if event_id < 192 {
            // DWORD event: 4-byte LE u32 value
            if pos + 4 > bytes.len() {
                break;
            }
            let value =
                u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]);
            pos += 4;

            if event_id == EVENT_BPM {
                let bpm = value as f64 / 1000.0;
                if (30.0..=300.0).contains(&bpm) {
                    intel.bpm = Some(bpm);
                }
            }
        } else {
            // Variable-length event: 4-byte LE u32 length prefix, then data
            if pos + 4 > bytes.len() {
                break;
            }
            let length =
                u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
                    as usize;
            pos += 4;

            if pos + length > bytes.len() {
                break;
            }
            let event_data = &bytes[pos..pos + length];

            if event_id == EVENT_CHANNEL_NAME && !event_data.is_empty() {
                let name = decode_text(event_data);
                if !name.is_empty() {
                    channel_names.push(name);
                }
            } else if event_id == EVENT_PLUGIN_NAME && !event_data.is_empty() {
                let name = decode_text(event_data);
                if !name.is_empty() {
                    plugin_names.push(name);
                }
            }

            pos += length;
        }
    }

    // Build time signature
    if let (Some(num), Some(den)) = (time_sig_num, time_sig_den) {
        if num > 0 && den > 0 {
            intel.time_signature = Some(format!("{}/{}", num, den));
        }
    }

    // Build tracks from channel names
    for name in &channel_names {
        intel.tracks.push(TrackInfo {
            name: name.clone(),
            track_type: "channel".to_string(),
            plugins: vec![],
            color: None,
        });
    }

    // Build plugin list (deduplicated with counts)
    let mut plugin_counts: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    for name in &plugin_names {
        *plugin_counts.entry(name.clone()).or_insert(0) += 1;
    }
    for (name, count) in &plugin_counts {
        intel.plugins.push(PluginInfo {
            name: name.clone(),
            plugin_type: "vst".to_string(),
            count: *count,
        });
    }

    // Set mixer info
    intel.mixer.total_tracks = if channel_count > 0 {
        channel_count
    } else {
        channel_names.len() as u32
    };

    Ok(intel)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a valid minimal FLP with custom event data in the FLdt chunk.
    fn build_flp(events: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        // FLhd header
        bytes.extend_from_slice(b"FLhd");
        bytes.extend_from_slice(&6u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 6]); // header data
                                            // FLdt chunk
        bytes.extend_from_slice(b"FLdt");
        let data_len = events.len() as u32;
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.extend_from_slice(events);
        bytes
    }

    #[test]
    fn rejects_non_flp() {
        let data = b"This is not an FLP file at all";
        let result = parse_bytes(data);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("FLhd"));
    }

    #[test]
    fn rejects_truncated() {
        let data = b"FLhd";
        let result = parse_bytes(data);
        assert!(result.is_err());
    }

    #[test]
    fn parses_minimal_valid() {
        let bytes = build_flp(&[]);
        let intel = parse_bytes(&bytes).unwrap();
        assert_eq!(intel.daw, "FL Studio");
        assert_eq!(intel.bpm, None);
    }

    #[test]
    fn extracts_bpm_from_event_156() {
        // BPM event: ID 156, value 128000 = 128.0 BPM
        let mut events = Vec::new();
        events.push(EVENT_BPM);
        events.extend_from_slice(&128000u32.to_le_bytes());
        let bytes = build_flp(&events);

        let intel = parse_bytes(&bytes).unwrap();
        assert_eq!(intel.bpm, Some(128.0));
    }

    #[test]
    fn rejects_bpm_out_of_range() {
        let mut events = Vec::new();
        events.push(EVENT_BPM);
        events.extend_from_slice(&500000u32.to_le_bytes()); // 500 BPM — out of range
        let bytes = build_flp(&events);

        let intel = parse_bytes(&bytes).unwrap();
        assert_eq!(intel.bpm, None);
    }

    #[test]
    fn extracts_time_signature() {
        // BYTE events: time sig num = 3, time sig den = 4
        let events = vec![EVENT_TIME_SIG_NUM, 3, EVENT_TIME_SIG_DEN, 4];
        let bytes = build_flp(&events);

        let intel = parse_bytes(&bytes).unwrap();
        assert_eq!(intel.time_signature.as_deref(), Some("3/4"));
    }

    #[test]
    fn counts_new_channels() {
        let mut events = Vec::new();
        // 3 WORD events for new channels (ID 64)
        for _ in 0..3 {
            events.push(EVENT_NEW_CHANNEL);
            events.extend_from_slice(&0u16.to_le_bytes());
        }
        let bytes = build_flp(&events);

        let intel = parse_bytes(&bytes).unwrap();
        assert_eq!(intel.mixer.total_tracks, 3);
    }

    #[test]
    fn extracts_channel_names() {
        let mut events = Vec::new();
        // Channel name: "Kick" as UTF-16LE + null terminator
        let name_bytes: Vec<u8> = "Kick"
            .encode_utf16()
            .chain(std::iter::once(0u16))
            .flat_map(|c| c.to_le_bytes())
            .collect();
        events.push(EVENT_CHANNEL_NAME);
        events.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
        events.extend_from_slice(&name_bytes);
        let bytes = build_flp(&events);

        let intel = parse_bytes(&bytes).unwrap();
        assert_eq!(intel.tracks.len(), 1);
        assert_eq!(intel.tracks[0].name, "Kick");
        assert_eq!(intel.tracks[0].track_type, "channel");
    }

    #[test]
    fn extracts_plugin_names() {
        let mut events = Vec::new();
        // Two plugin name events
        for plugin in &["Serum", "Serum"] {
            let name_bytes: Vec<u8> = plugin
                .encode_utf16()
                .chain(std::iter::once(0u16))
                .flat_map(|c| c.to_le_bytes())
                .collect();
            events.push(EVENT_PLUGIN_NAME);
            events.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            events.extend_from_slice(&name_bytes);
        }
        let bytes = build_flp(&events);

        let intel = parse_bytes(&bytes).unwrap();
        assert_eq!(intel.plugins.len(), 1); // Deduplicated
        assert_eq!(intel.plugins[0].name, "Serum");
        assert_eq!(intel.plugins[0].count, 2);
    }

    #[test]
    fn skips_non_bpm_events() {
        let mut events = Vec::new();
        // BYTE event (ID 10)
        events.push(10);
        events.push(0x42);
        // WORD event (ID 64 = new channel)
        events.push(64);
        events.extend_from_slice(&1u16.to_le_bytes());
        // DWORD event (ID 130, not BPM)
        events.push(130);
        events.extend_from_slice(&999u32.to_le_bytes());
        // Variable-length event (ID 200, 3 bytes of data)
        events.push(200);
        events.extend_from_slice(&3u32.to_le_bytes());
        events.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        // Finally: BPM event
        events.push(EVENT_BPM);
        events.extend_from_slice(&140000u32.to_le_bytes());
        let bytes = build_flp(&events);

        let intel = parse_bytes(&bytes).unwrap();
        assert_eq!(intel.bpm, Some(140.0));
    }

    #[test]
    fn full_project_parsing() {
        let mut events = Vec::new();
        // BPM 120
        events.push(EVENT_BPM);
        events.extend_from_slice(&120000u32.to_le_bytes());
        // Time sig 4/4
        events.push(EVENT_TIME_SIG_NUM);
        events.push(4);
        events.push(EVENT_TIME_SIG_DEN);
        events.push(4);
        // 2 channels
        events.push(EVENT_NEW_CHANNEL);
        events.extend_from_slice(&0u16.to_le_bytes());
        events.push(EVENT_NEW_CHANNEL);
        events.extend_from_slice(&0u16.to_le_bytes());
        // Channel names
        for name in &["Kick", "Snare"] {
            let name_bytes: Vec<u8> = name
                .encode_utf16()
                .chain(std::iter::once(0u16))
                .flat_map(|c| c.to_le_bytes())
                .collect();
            events.push(EVENT_CHANNEL_NAME);
            events.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            events.extend_from_slice(&name_bytes);
        }
        // Plugin
        let plugin_bytes: Vec<u8> = "Sausage Fattener"
            .encode_utf16()
            .chain(std::iter::once(0u16))
            .flat_map(|c| c.to_le_bytes())
            .collect();
        events.push(EVENT_PLUGIN_NAME);
        events.extend_from_slice(&(plugin_bytes.len() as u32).to_le_bytes());
        events.extend_from_slice(&plugin_bytes);

        let bytes = build_flp(&events);
        let intel = parse_bytes(&bytes).unwrap();

        assert_eq!(intel.bpm, Some(120.0));
        assert_eq!(intel.time_signature.as_deref(), Some("4/4"));
        assert_eq!(intel.mixer.total_tracks, 2);
        assert_eq!(intel.tracks.len(), 2);
        assert_eq!(intel.tracks[0].name, "Kick");
        assert_eq!(intel.plugins.len(), 1);
        assert_eq!(intel.plugins[0].name, "Sausage Fattener");
    }

    // ── v0.29.7: Edge-case hardening ─────────────────────────────────

    #[test]
    fn parse_truncated_dword_event() {
        // DWORD event (BPM) with only 1 of 4 value bytes
        let events = vec![156u8, 0x00];
        let bytes = build_flp(&events);
        let intel = parse_bytes(&bytes).unwrap();
        assert_eq!(intel.daw, "FL Studio");
        // Should not panic, just skip truncated event
    }

    #[test]
    fn parse_zero_length_text_event() {
        let mut events = Vec::new();
        events.push(192u8); // TEXT event
        events.extend_from_slice(&0u32.to_le_bytes());
        let bytes = build_flp(&events);
        assert!(parse_bytes(&bytes).is_ok());
    }

    #[test]
    fn parse_oversized_text_length() {
        // Claimed length far exceeds actual data
        let mut events = Vec::new();
        events.push(192u8);
        events.extend_from_slice(&999999u32.to_le_bytes());
        events.extend_from_slice(b"hi");
        let bytes = build_flp(&events);
        assert!(parse_bytes(&bytes).is_ok()); // Graceful stop, not panic
    }

    #[test]
    fn parse_all_byte_event_ids_no_panic() {
        let mut events: Vec<u8> = Vec::new();
        for id in 0..64u8 {
            events.push(id);
            events.push(0);
        }
        let bytes = build_flp(&events);
        assert!(parse_bytes(&bytes).is_ok());
    }
}
