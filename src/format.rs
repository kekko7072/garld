//! Human-readable renderings of the raw numbers in a snapshot.

/// Formats a byte count the way `top` does: three significant digits and a
/// single-letter binary suffix.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    if n < 1024 {
        return format!("{n}B");
    }
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0}{}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1}{}", UNITS[unit])
    } else {
        format!("{value:.2}{}", UNITS[unit])
    }
}

/// Formats a byte-per-sample count as a rate, or `-` when idle.
#[cfg(feature = "gui")]
pub fn rate(n: u64) -> String {
    if n == 0 {
        "-".to_string()
    } else {
        format!("{}/s", bytes(n))
    }
}

/// Formats a wall-clock duration compactly: `42s`, `12m30s`, `3h05m`, `9d21h`.
pub fn duration(secs: u64) -> String {
    let (d, h, m, s) = (
        secs / 86_400,
        secs % 86_400 / 3600,
        secs % 3600 / 60,
        secs % 60,
    );
    if d > 0 {
        format!("{d}d{h:02}h")
    } else if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Formats accumulated CPU time as `top`'s TIME+ column: `MM:SS.hh`, widening
/// to include hours and days as needed.
pub fn cpu_time(ms: u64) -> String {
    let total_secs = ms / 1000;
    let hundredths = (ms % 1000) / 10;
    let (d, h, m, s) = (
        total_secs / 86_400,
        total_secs % 86_400 / 3600,
        total_secs % 3600 / 60,
        total_secs % 60,
    );
    if d > 0 {
        format!("{d}d{h:02}:{m:02}:{s:02}")
    } else if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}.{hundredths:02}")
    }
}

/// Formats a percentage with one decimal place.
pub fn percent(p: f32) -> String {
    format!("{p:.1}")
}

/// A fixed-width text meter, e.g. `[|||||     ]`.
pub fn meter(fraction: f32, width: usize) -> String {
    let filled = ((fraction.clamp(0.0, 1.0) * width as f32).round() as usize).min(width);
    format!("[{}{}]", "|".repeat(filled), " ".repeat(width - filled))
}

/// Truncates to `max` display columns, marking elision with `…`.
///
/// Counts `char`s rather than grapheme clusters — good enough for the paths and
/// argv we display, and avoids pulling in a segmentation dependency.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let len = s.chars().count();
    if len <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

/// Pads to `width` columns, counting `char`s so multi-byte names still line up.
pub fn pad_right(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
}

/// Right-aligns to `width` columns.
pub fn pad_left(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{}{s}", " ".repeat(width - len))
    }
}
