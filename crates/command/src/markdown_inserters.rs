//! §H5 slash-palette inserters that compute their text at dispatch
//! time. Each command is a single `Context::insert_text` call — the
//! insertion-only contract the slash palette enforces — and each is
//! registered with the `palette_safe` flag so the H5 populator picks
//! it up.
//!
//! Live wall-clock readers (`timestamp`, `date`) and UUID v7
//! (`uuid`) read from `std::time::SystemTime` / `uuid::Uuid::now_v7()`
//! respectively. No external time / chrono dependency — date
//! conversion uses Howard Hinnant's `civil_from_days` algorithm.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::id::CommandId;
use crate::registry::Registry;
use crate::ContextPredicate;

/// `markdown.insert_footnote` — inserts `[^N]` at the caret with `N`
/// chosen as a stable placeholder. Users fill in the body manually.
pub const MARKDOWN_INSERT_FOOTNOTE: CommandId = CommandId("markdown.insert_footnote");
/// `markdown.insert_callout` — inserts a GitHub-flavoured callout
/// starter: `> [!NOTE]\n> `. Lands at the caret.
pub const MARKDOWN_INSERT_CALLOUT: CommandId = CommandId("markdown.insert_callout");
/// `markdown.insert_timestamp` — inserts an ISO-8601 UTC timestamp
/// derived from `SystemTime::now()`.
pub const MARKDOWN_INSERT_TIMESTAMP: CommandId = CommandId("markdown.insert_timestamp");
/// `markdown.insert_date` — inserts an ISO-8601 UTC date
/// (`YYYY-MM-DD`).
pub const MARKDOWN_INSERT_DATE: CommandId = CommandId("markdown.insert_date");
/// `markdown.insert_uuid` — inserts a UUID v7 string.
pub const MARKDOWN_INSERT_UUID: CommandId = CommandId("markdown.insert_uuid");
/// `markdown.insert_horizontal_rule` — inserts the markdown
/// thematic-break (`\n---\n`).
pub const MARKDOWN_INSERT_HORIZONTAL_RULE: CommandId = CommandId("markdown.insert_horizontal_rule");

/// Register every §H5 slash-palette inserter with `palette_safe`
/// flipped. Called from `register_markdown_commands` so the inserters
/// share the `editor.focused` predicate.
pub fn register_markdown_inserters(registry: &mut Registry, focused: &ContextPredicate) {
    registry.register_palette_safe(
        MARKDOWN_INSERT_FOOTNOTE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.insert_text("[^1]")),
    );
    registry.register_palette_safe(
        MARKDOWN_INSERT_CALLOUT,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.insert_text("> [!NOTE]\n> ")),
    );
    registry.register_palette_safe(
        MARKDOWN_INSERT_TIMESTAMP,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.insert_text(&now_iso_timestamp())),
    );
    registry.register_palette_safe(
        MARKDOWN_INSERT_DATE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.insert_text(&today_iso_date())),
    );
    registry.register_palette_safe(
        MARKDOWN_INSERT_UUID,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.insert_text(&uuid::Uuid::now_v7().to_string())),
    );
    registry.register_palette_safe(
        MARKDOWN_INSERT_HORIZONTAL_RULE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.insert_text("\n---\n")),
    );
}

/// Format `SystemTime::now()` as `YYYY-MM-DDTHH:MM:SSZ`. Falls back
/// to the UNIX epoch when the clock is set before 1970-01-01.
fn now_iso_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_unix_timestamp(secs)
}

/// Format `SystemTime::now()` as `YYYY-MM-DD`.
fn today_iso_date() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_unix_date(secs)
}

/// Format Unix seconds-since-epoch as `YYYY-MM-DDTHH:MM:SSZ`.
pub(crate) fn format_unix_timestamp(secs: u64) -> String {
    let days = secs / 86_400;
    let time = secs % 86_400;
    let hour = time / 3600;
    let minute = (time % 3600) / 60;
    let second = time % 60;
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hour, minute, second
    )
}

/// Format Unix seconds-since-epoch as `YYYY-MM-DD`.
pub(crate) fn format_unix_date(secs: u64) -> String {
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Howard Hinnant's `civil_from_days` algorithm: convert days since
/// the Unix epoch (1970-01-01) into a proleptic Gregorian
/// `(year, month, day)` tuple. Handles negative days (pre-1970) and
/// leap years correctly.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe as i32 + (era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_unix_epoch_is_1970_01_01() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn civil_from_days_handles_leap_day() {
        // 2024-02-29 is 19782 days after 1970-01-01.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn format_unix_timestamp_zero_is_epoch() {
        assert_eq!(format_unix_timestamp(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn format_unix_timestamp_round_trips_known_moment() {
        // 2024-01-01T00:00:00Z = 1704067200 seconds since the epoch.
        assert_eq!(format_unix_timestamp(1_704_067_200), "2024-01-01T00:00:00Z");
    }

    #[test]
    fn format_unix_date_strips_time_component() {
        // Anything inside 2024-01-01 (UTC) renders as the same date.
        assert_eq!(format_unix_date(1_704_067_200), "2024-01-01");
        assert_eq!(format_unix_date(1_704_067_200 + 60_000), "2024-01-01");
    }

    #[test]
    fn registration_marks_every_inserter_palette_safe() {
        let mut registry = Registry::new();
        let focused = ContextPredicate::parse("editor.focused");
        register_markdown_inserters(&mut registry, &focused);
        for id in [
            MARKDOWN_INSERT_FOOTNOTE,
            MARKDOWN_INSERT_CALLOUT,
            MARKDOWN_INSERT_TIMESTAMP,
            MARKDOWN_INSERT_DATE,
            MARKDOWN_INSERT_UUID,
            MARKDOWN_INSERT_HORIZONTAL_RULE,
        ] {
            assert!(
                registry.is_palette_safe(id.as_str()),
                "expected {} to be palette_safe",
                id.as_str()
            );
        }
    }

    /// Stub Context that records every `insert_text` call so we can
    /// assert each inserter emits a single `Insert` of the expected
    /// content.
    #[derive(Default)]
    struct InsertCaptor {
        inserts: Vec<String>,
    }
    impl crate::FindContext for InsertCaptor {}
    impl crate::Context for InsertCaptor {
        fn lookup(&self, key: &str) -> Option<&str> {
            (key == "editor.focused").then_some("true")
        }
        fn insert_text(&mut self, text: &str) -> Result<(), crate::Error> {
            self.inserts.push(text.to_string());
            Ok(())
        }
    }
    impl crate::ViewContext for InsertCaptor {}

    fn dispatch_inserter(id: CommandId) -> Vec<String> {
        let mut registry = Registry::new();
        let focused = ContextPredicate::parse("editor.focused");
        register_markdown_inserters(&mut registry, &focused);
        let mut ctx = InsertCaptor::default();
        registry
            .dispatch(id, &serde_json::Value::Null, &mut ctx)
            .expect("dispatch ok");
        ctx.inserts
    }

    #[test]
    fn footnote_inserter_emits_placeholder_marker() {
        let out = dispatch_inserter(MARKDOWN_INSERT_FOOTNOTE);
        assert_eq!(out, vec!["[^1]"]);
    }

    #[test]
    fn callout_inserter_emits_github_starter() {
        let out = dispatch_inserter(MARKDOWN_INSERT_CALLOUT);
        assert_eq!(out, vec!["> [!NOTE]\n> "]);
    }

    #[test]
    fn horizontal_rule_inserter_emits_thematic_break() {
        let out = dispatch_inserter(MARKDOWN_INSERT_HORIZONTAL_RULE);
        assert_eq!(out, vec!["\n---\n"]);
    }

    #[test]
    fn timestamp_inserter_emits_iso_8601_shape() {
        let out = dispatch_inserter(MARKDOWN_INSERT_TIMESTAMP);
        assert_eq!(out.len(), 1);
        let s = &out[0];
        // Shape: YYYY-MM-DDTHH:MM:SSZ — 20 chars exactly.
        assert_eq!(s.len(), 20, "got `{}`", s);
        assert!(s.ends_with('Z'));
        assert_eq!(s.as_bytes()[10], b'T');
    }

    #[test]
    fn date_inserter_emits_iso_8601_date_shape() {
        let out = dispatch_inserter(MARKDOWN_INSERT_DATE);
        assert_eq!(out.len(), 1);
        let s = &out[0];
        // Shape: YYYY-MM-DD — 10 chars, hyphens at 4 and 7.
        assert_eq!(s.len(), 10, "got `{}`", s);
        assert_eq!(s.as_bytes()[4], b'-');
        assert_eq!(s.as_bytes()[7], b'-');
    }

    #[test]
    fn uuid_inserter_emits_v7_shape() {
        let out = dispatch_inserter(MARKDOWN_INSERT_UUID);
        assert_eq!(out.len(), 1);
        let s = &out[0];
        // UUID v7 string form is 36 chars with hyphens at the
        // standard positions.
        assert_eq!(s.len(), 36, "got `{}`", s);
        assert_eq!(s.as_bytes()[8], b'-');
        assert_eq!(s.as_bytes()[13], b'-');
        // Version nibble is at byte index 14 (`Mxxx-...`) — v7 is
        // the literal `7`.
        assert_eq!(s.as_bytes()[14], b'7', "expected v7 prefix, got `{}`", s);
    }
}
