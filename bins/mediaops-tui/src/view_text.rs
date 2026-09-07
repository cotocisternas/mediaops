//! Column plan and wrapping for the resource browser.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::sanitize::clip;

#[derive(Debug, Clone, Copy)]
pub struct ColPlan {
    pub index: usize,
    pub header: &'static str,
    pub width: u16,
    pub numeric: bool,
}

pub fn numeric_header(header: &str) -> bool {
    matches!(header, "BYTES" | "ATTEMPTS" | "SIZE" | "AGE" | "PROGRESS")
}

fn min_width(header: &str) -> u16 {
    let preferred = match header {
        "TITLE" => 8,
        "PHASE" => 9,
        "PROGRESS" => 8,
        "BYTES" => 9,
        "ATTEMPTS" => 8,
        "NODE" => 7,
        "FAILURE" | "MESSAGE" => 12,
        "SIZE" => 7,
        "AGE" => 3,
        "READY" => 9,
        "HEARTBEAT" => 12,
        "ROOT" => 10,
        "PATH" => 6,
        "KIND" => 5,
        "FACT" => 24,
        _ => 4,
    };
    preferred.max(u16::try_from(header.width()).unwrap_or(u16::MAX))
}

pub fn plan_columns(headers: &[&'static str], total: u16) -> Vec<ColPlan> {
    if headers.is_empty() || total == 0 {
        return Vec::new();
    }
    let mut n = headers.len();
    while n >= 1 {
        let gutters = n.saturating_sub(1) as u16;
        let mins: u16 = headers[..n].iter().map(|h| min_width(h)).sum();
        if mins + gutters <= total {
            let extra = total - mins - gutters;
            let mut widths: Vec<u16> = headers[..n].iter().map(|h| min_width(h)).collect();
            let flex = headers[..n]
                .iter()
                .position(|h| matches!(*h, "TITLE" | "PATH"))
                .or_else(|| headers[..n].iter().position(|h| !numeric_header(h)))
                .unwrap_or(0);
            widths[flex] = widths[flex].saturating_add(extra);
            let readable_width = if headers[flex] == "TITLE" { 24 } else { 12 };
            if widths.get(flex).copied().unwrap_or(0) >= readable_width || n == 1 {
                return headers[..n]
                    .iter()
                    .enumerate()
                    .map(|(i, header)| ColPlan {
                        index: i,
                        header,
                        width: widths[i],
                        numeric: numeric_header(header),
                    })
                    .collect();
            }
        }
        n -= 1;
    }
    vec![ColPlan {
        index: 0,
        header: headers[0],
        width: total,
        numeric: numeric_header(headers[0]),
    }]
}

pub fn align_cell(text: &str, width: usize, numeric: bool) -> String {
    let (clipped, _) = clip(text, width);
    let used = clipped.width();
    if used >= width {
        return clipped;
    }
    let pad = " ".repeat(width - used);
    if numeric {
        format!("{pad}{clipped}")
    } else {
        format!("{clipped}{pad}")
    }
}

pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let clean = crate::sanitize::sanitize(text);
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0usize;
    for c in clean.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if c == ' ' && used == 0 {
            continue;
        }
        if used + cw > width && used > 0 {
            if c == ' ' {
                lines.push(line.trim_end().to_string());
                line.clear();
                used = 0;
                continue;
            }
            if let Some(boundary) = line.rfind(' ') {
                let remainder = line[boundary + 1..].to_string();
                lines.push(line[..boundary].trim_end().to_string());
                line = remainder;
                used = line.width();
            } else {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
        }
        if cw > width {
            continue;
        }
        line.push(c);
        used += cw;
    }
    lines.push(line);
    lines
}

pub fn help_lines(ui: &crate::model::UiModel) -> Vec<String> {
    let width = usize::from(crate::geometry::Shell::for_ui(ui).overlay.inner.width);
    let mut lines: Vec<String> = [
        "1 Overview  2 Wants  3 Jobs  4 Holds  5 Titles  6 Nodes  7 Box",
        "Tab / Shift-Tab  next/prev screen",
        "j k arrows  PageUp PageDown  Home End / g G  rows or scroll",
        "Enter / d  detail   Esc  back   ?  help   q  quit",
        "/ live literal filter; Enter keeps; Esc cancels editing",
        "Esc in list clears filter; changing resource resets it",
        ":overview :want(s) :job(s) :hold(s) :title(s) :node(s)",
        ":box :help :quit / :q; navigation, help or quit only",
        "Input: Backspace edits, Ctrl-U clears, Ctrl-C quits",
        "Input letters are text; Enter never syncs or mutates",
        "W apply Want   D delete Want   A approve Hold   X reject Hold",
        "p preview copies   S fresh sync (not watching)",
        "mutations only in selected detail; Enter never writes",
        "100% copied still needs verification and installation",
        "NOT CURRENT: reconnecting automatically; actions disabled",
        "Service: systemctl --user status mediaops-home.service",
        "Logs: journalctl --user -u mediaops-home.service -n 50",
    ]
    .into_iter()
    .flat_map(|text| wrap_text(text, width))
    .collect();
    let message = ui.connection_message.as_ref().or(ui.message.as_ref());
    if let Some(message) = message {
        lines.push(String::new());
        lines.extend(wrap_text(&format!("Status: {message}"), width));
    } else if let Some(message) = &ui.last_message {
        lines.push(String::new());
        lines.extend(wrap_text(&format!("Last status: {message}"), width));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_columns_sum_to_width() {
        let cols = plan_columns(
            &["TITLE", "PHASE", "BYTES", "ATTEMPTS", "NODE", "FAILURE"],
            60,
        );
        assert!(cols.len() >= 2);
        let sum: u16 = cols.iter().map(|c| c.width).sum::<u16>() + cols.len() as u16 - 1;
        assert_eq!(sum, 60);
        assert_eq!(cols[0].header, "TITLE");
        assert!(cols[0].width >= 12 || cols.len() == 1);
    }

    #[test]
    fn numeric_cells_pad_left() {
        assert_eq!(align_cell("12", 5, true), "   12");
        assert_eq!(align_cell("ab", 5, false), "ab   ");
    }

    #[test]
    fn all_visible_headers_fit_their_columns() {
        for width in [60, 80, 140] {
            for col in plan_columns(
                &["TITLE", "PHASE", "BYTES", "ATTEMPTS", "NODE", "FAILURE"],
                width,
            ) {
                assert!(
                    usize::from(col.width) >= col.header.width(),
                    "{} clipped at {width}",
                    col.header
                );
            }
        }
    }

    #[test]
    fn wrap_breaks_long_reason() {
        let lines = wrap_text("one two three four five", 10);
        assert!(lines.len() >= 2);
        assert!(lines.iter().all(|l| l.width() <= 10));
    }

    #[test]
    fn prose_prefers_words_and_long_unicode_identity_keeps_every_cell() {
        assert_eq!(
            wrap_text("see Overview / Nodes", 18),
            ["see Overview /", "Nodes"]
        );
        let id = "movie:key:映画日本語.1999";
        let lines = wrap_text(id, 9);
        assert_eq!(lines.concat(), id);
        assert!(lines.iter().all(|line| line.width() <= 9));
    }
}
