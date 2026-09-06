//! Strip untrusted control sequences before draw.

use unicode_width::UnicodeWidthStr;

pub fn sanitize(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Clip to `cols` display cells. Second value is true when content was cut.
pub fn clip(raw: &str, cols: usize) -> (String, bool) {
    let clean = sanitize(raw);
    if cols == 0 {
        return (String::new(), !clean.is_empty());
    }
    if clean.width() <= cols {
        return (clean, false);
    }
    if cols == 1 {
        return ("…".into(), true);
    }
    let mut out = String::new();
    let mut used = 0usize;
    let budget = cols - 1;
    for c in clean.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    (out, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_sequences_become_spaces() {
        assert_eq!(sanitize("a\u{1b}[31mb\n"), "a [31mb ");
    }

    #[test]
    fn wide_chars_count_as_two_cells() {
        let (text, clipped) = clip("日本語", 4);
        assert!(clipped);
        assert!(text.width() <= 4);
        assert!(text.ends_with('…'));
    }
}
