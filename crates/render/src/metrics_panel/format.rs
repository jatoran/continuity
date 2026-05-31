//! Formatting and scalar helpers for metrics-panel layout.

pub(super) fn chars_to_words(chars: u64) -> u64 {
    chars / 5
}

pub(super) fn plural_words(words: u64) -> String {
    if words == 1 {
        "1 word".into()
    } else {
        format!("{words} words")
    }
}

pub(super) fn plural_keystrokes(count: u64) -> String {
    if count == 1 {
        "1 keystroke".into()
    } else {
        format!("{count} keystrokes")
    }
}

pub(super) fn plural_chars(count: u64) -> String {
    if count == 1 {
        "1 char".into()
    } else {
        format!("{count} chars")
    }
}

pub(super) fn format_duration(ms: u64) -> String {
    let minutes = ms / 60_000;
    if minutes < 1 {
        return "under 1 min active".into();
    }
    if minutes < 60 {
        return format!("{minutes} min active");
    }
    let hours = minutes / 60;
    let rem = minutes % 60;
    if rem == 0 {
        format!("{hours}h active")
    } else {
        format!("{hours}h {rem}m active")
    }
}

pub(super) fn lerp_color(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |sa: u8, sb: u8| -> u8 {
        let fa = f32::from(sa);
        let fb = f32::from(sb);
        let m = fa + (fb - fa) * t;
        m.round().clamp(0.0, 255.0) as u8
    };
    let a_a = ((a >> 24) & 0xFF) as u8;
    let r_a = ((a >> 16) & 0xFF) as u8;
    let g_a = ((a >> 8) & 0xFF) as u8;
    let b_a = (a & 0xFF) as u8;
    let a_b = ((b >> 24) & 0xFF) as u8;
    let r_b = ((b >> 16) & 0xFF) as u8;
    let g_b = ((b >> 8) & 0xFF) as u8;
    let b_b = (b & 0xFF) as u8;
    (u32::from(mix(a_a, a_b)) << 24)
        | (u32::from(mix(r_a, r_b)) << 16)
        | (u32::from(mix(g_a, g_b)) << 8)
        | u32::from(mix(b_a, b_b))
}
