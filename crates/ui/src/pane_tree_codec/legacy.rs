//! Backward-compat decoders for the pane-tree JSON blob.
//!
//! Lives next to [`super::pane_tree_codec`] for the 600-line cap. Two
//! responsibilities:
//!
//! 1. Lenient `Deserialize` for [`super::WireImageExpand`]: accepts both
//!    the current `{source_byte: u64}` shape and the legacy
//!    `{url: String}` shape that older blobs in the wild still carry.
//!    Legacy entries lose their source byte (no longer reconstructable
//!    without the original rope), so they decode with `source_byte = 0`
//!    — the renderer silently discards toggles whose byte no longer
//!    points at an image URL, so the worst-case is a stale expand
//!    state, never a "skip this window."
//! 2. [`buffer_ids_in_json_lenient`]: walks the JSON via
//!    [`serde_json::Value`] so a single malformed auxiliary field
//!    (`image_expand_state`, an unrecognized tab `kind`, a corrupted
//!    `recently_closed` entry, …) does not cascade into "skip this
//!    window" during startup restoration.
//!
//! Single-writer rule: this module is pure deserialization — it never
//! holds state and never mutates a [`super::PaneTree`].

use continuity_buffer::BufferId;
use serde::Deserialize;
use uuid::Uuid;

use super::{WireImageExpand, WireUuid};

impl<'de> Deserialize<'de> for WireImageExpand {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Legacy blobs carried a `url: String` keying field that has
        // since been replaced with `source_byte: u64`. We accept either
        // by leaving unknown fields untouched (serde's default
        // behaviour without `deny_unknown_fields`) and defaulting
        // `source_byte` when absent.
        #[derive(Deserialize)]
        struct Raw {
            buffer: WireUuid,
            #[serde(default)]
            source_byte: Option<u64>,
            expanded: bool,
        }
        let raw = Raw::deserialize(d)?;
        Ok(WireImageExpand {
            buffer: raw.buffer,
            source_byte: raw.source_byte.unwrap_or(0),
            expanded: raw.expanded,
        })
    }
}

/// Return every distinct buffer id referenced by encoded pane-tree tabs,
/// even when the JSON's structural fields (`root`, `groups`, `focused`)
/// are missing, malformed, or carry an incompatible shape from an older
/// codec. Walks the JSON tree manually via [`serde_json::Value`] so any
/// schema drift on auxiliary fields does not cascade into "skip this
/// window."
///
/// Returns an empty vector when no recognizable tab carrying a non-nil
/// buffer id can be found. Never errors: a window row whose JSON is
/// total gibberish should fall through to a fresh restore, not block
/// session restore for every other window.
#[must_use]
pub fn buffer_ids_in_json_lenient(json: &str) -> Vec<BufferId> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(tabs) = value.get("tabs").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for tab in tabs {
        let kind = tab.get("kind").and_then(serde_json::Value::as_str);
        if matches!(kind, Some(k) if k != "buffer") {
            continue;
        }
        let Some(arr) = tab.get("buffer").and_then(serde_json::Value::as_array) else {
            continue;
        };
        if arr.len() != 16 {
            continue;
        }
        let mut bytes = [0u8; 16];
        let mut ok = true;
        for (i, v) in arr.iter().enumerate() {
            match v.as_u64() {
                Some(n) if n <= u8::MAX as u64 => bytes[i] = n as u8,
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        let id = BufferId::from_uuid(Uuid::from_bytes(bytes));
        if id.is_nil() || out.contains(&id) {
            continue;
        }
        out.push(id);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lenient_extractor_handles_legacy_image_expand_url_field() {
        // Synthesize a blob whose `image_expand_state` uses the legacy
        // `url: String` shape. The strict codec would reject it; the
        // lenient extractor must still yield the tab's buffer id.
        let id = BufferId::new();
        let bytes: Vec<u64> = id.as_uuid().as_bytes().iter().map(|b| *b as u64).collect();
        let json = format!(
            r#"{{"root":{{"kind":"leaf","pane":1}},
                 "groups":[{{"id":1,"tabs":[2],"active":2,"mru":[2]}}],
                 "tabs":[{{"id":2,"buffer":{bytes:?},
                           "label_override":null,"created_at_ms":0,
                           "file_associated":false,"kind":"buffer"}}],
                 "focused":1,
                 "image_expand_state":[{{"buffer":{bytes:?},
                                         "url":"images/x.png",
                                         "expanded":true}}]}}"#,
            bytes = bytes
        );
        // Strict path accepts it — the legacy WireImageExpand deserializer
        // tolerates the `url` field by ignoring it.
        let strict = super::super::buffer_ids_in_json(&json).expect("strict ok with legacy field");
        assert_eq!(strict, vec![id]);
        // Lenient path also yields the id.
        let lenient = buffer_ids_in_json_lenient(&json);
        assert_eq!(lenient, vec![id]);
    }

    #[test]
    fn lenient_extractor_yields_empty_on_total_garbage() {
        assert!(buffer_ids_in_json_lenient("not json at all").is_empty());
        assert!(buffer_ids_in_json_lenient("{}").is_empty());
        assert!(buffer_ids_in_json_lenient(r#"{"tabs":42}"#).is_empty());
    }

    #[test]
    fn lenient_extractor_skips_non_buffer_tabs_and_nil_buffers() {
        let real = BufferId::new();
        let real_bytes: Vec<u64> = real
            .as_uuid()
            .as_bytes()
            .iter()
            .map(|b| *b as u64)
            .collect();
        let nil_bytes: Vec<u64> = vec![0u64; 16];
        let json = format!(
            r#"{{"tabs":[
                {{"id":1,"buffer":{nil:?},"kind":"buffer"}},
                {{"id":2,"buffer":{real:?},"kind":"buffer_history"}},
                {{"id":3,"buffer":{real:?},"kind":"buffer"}}
              ]}}"#,
            nil = nil_bytes,
            real = real_bytes
        );
        assert_eq!(buffer_ids_in_json_lenient(&json), vec![real]);
    }
}
