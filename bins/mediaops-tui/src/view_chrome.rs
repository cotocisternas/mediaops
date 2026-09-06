//! Masthead, table, detail, status, footer.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use crate::disk::DiskObservation;
use crate::format::{fmt_age, fmt_bytes};
use crate::model::{Screen, SyncState, UiModel};
use crate::projection::{HOLD_CAPTION, ListingKind, Projection};
use crate::view_text::{align_cell, help_lines, plan_columns, wrap_text};

fn style_focus(color: bool) -> Style {
    let base = Style::default().add_modifier(Modifier::REVERSED);
    if color { base.fg(Color::Cyan) } else { base }
}

fn style_status(sync: SyncState, color: bool) -> Style {
    if !color {
        return if matches!(sync, SyncState::Stale | SyncState::Connecting) {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
    }
    match sync {
        SyncState::Current => Style::default().fg(Color::Green),
        SyncState::Stale | SyncState::Connecting => Style::default().fg(Color::Yellow),
        SyncState::Synchronizing => Style::default(),
    }
}

pub(crate) fn render_masthead(
    frame: &mut Frame<'_>,
    area: Rect,
    ui: &UiModel,
    sync: SyncState,
    disk: &DiskObservation,
    color: bool,
) {
    let disk_text = match disk {
        DiskObservation::Ready { bytes, age } => {
            format!(
                "disk  {} free  {}",
                fmt_bytes(*bytes),
                fmt_age(age.as_secs())
            )
        }
        DiskObservation::Unavailable { .. } => "disk  unavailable".into(),
    };
    let line = Line::from(vec![
        Span::styled("mediaops", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::raw(ui.screen.title()),
        Span::raw("  "),
        Span::styled(sync.label(), style_status(sync, color)),
        Span::raw("  "),
        Span::raw(disk_text),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

pub(crate) fn render_rule(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(Paragraph::new("-".repeat(area.width as usize)), area);
}

pub(crate) fn render_undersize(frame: &mut Frame<'_>, color: bool) {
    let style = if color {
        Style::default().fg(Color::Red)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled("terminal too small  60x16 required", style),
            Line::from("q quit"),
        ]),
        frame.area(),
    );
}

pub(crate) fn render_help(frame: &mut Frame<'_>, area: Rect, ui: &UiModel) {
    let lines = help_lines(ui);
    let offset =
        usize::from(ui.help_offset).min(lines.len().saturating_sub(usize::from(area.height)));
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(offset)
                .map(Line::from)
                .collect::<Vec<_>>(),
        ),
        area,
    );
}

pub(crate) fn render_status(
    frame: &mut Frame<'_>,
    area: Rect,
    ui: &UiModel,
    sync: SyncState,
    projection: &Projection,
    list_failed: bool,
) {
    let pending = ui.mutation_pending || ui.sync_pending;
    let progress = ui
        .pending_started
        .map(|started| format!("pending {}", fmt_age(started.elapsed().as_secs())))
        .unwrap_or_else(|| "pending".into());
    let text = match (ui.help, pending, ui.message.as_deref()) {
        (true, _, _) => position_status(ui, sync, projection, list_failed),
        (false, true, Some(msg)) => format!("{progress}  {msg}"),
        (false, true, None) => progress,
        (false, false, Some(msg)) => msg.to_string(),
        (false, false, None) => position_status(ui, sync, projection, list_failed),
    };
    let (text, clipped) = crate::sanitize::clip(&text, usize::from(area.width));
    let text = if clipped {
        let width = usize::from(area.width).saturating_sub(9);
        format!("{}  ? status", crate::sanitize::clip(&text, width).0)
    } else {
        text
    };
    frame.render_widget(Paragraph::new(text), area);
}

fn position_status(
    ui: &UiModel,
    sync: SyncState,
    projection: &Projection,
    list_failed: bool,
) -> String {
    let visible = usize::from(ui.rows.saturating_sub(5)).max(1);
    if ui.help {
        return line_position(usize::from(ui.help_offset), help_lines(ui).len(), visible);
    }
    if !sync.writes_allowed() {
        return "actions disabled; reconnecting automatically; ? help".into();
    }
    if list_failed {
        return "Home API read failed; reconnecting; ? help".into();
    }
    if let Some(report) = &ui.report {
        return line_position(
            usize::from(ui.report_offset),
            report.lines(usize::from(ui.cols)).len(),
            visible,
        );
    }
    if projection.listing == ListingKind::Unavailable {
        return if matches!(ui.screen, Screen::Holds | Screen::BoxListing) {
            "listing unavailable; check inventory on 6 Nodes".into()
        } else {
            "worker status unavailable; ? help to inspect service".into()
        };
    }
    if ui.in_detail {
        let width = if ui.split_detail() {
            Layout::horizontal([
                Constraint::Percentage(58),
                Constraint::Length(1),
                Constraint::Min(24),
            ])
            .split(Rect::new(0, 0, ui.cols, ui.rows))[2]
                .width
        } else {
            ui.cols
        };
        let total = projection
            .detail
            .iter()
            .map(|line| wrap_text(&line.value, usize::from(width.saturating_sub(15)).max(1)).len())
            .sum();
        let position = line_position(
            usize::from(ui.detail_offset),
            total,
            visible.saturating_sub(usize::from(projection.hold_caption)),
        );
        if ui.identity_clipped
            && matches!(ui.screen, Screen::Wants | Screen::Titles | Screen::Holds)
        {
            return if ui.detail_offset > 0 {
                "actions disabled: Home to show identity; resize if needed".into()
            } else {
                "actions disabled: enlarge terminal to show full identity".into()
            };
        }
        return position;
    }
    if projection.rows.is_empty() {
        return String::new();
    }
    format!(
        "row {} of {}  |  Enter detail",
        ui.selected.saturating_add(1).min(projection.rows.len()),
        projection.rows.len()
    )
}

fn line_position(offset: usize, total: usize, visible: usize) -> String {
    if total == 0 {
        return "no details".into();
    }
    let start = offset.min(total.saturating_sub(visible));
    format!(
        "lines {}-{} of {}  |  j/k scroll",
        start + 1,
        (start + visible).min(total),
        total
    )
}

pub(crate) fn render_footer(
    frame: &mut Frame<'_>,
    area: Rect,
    ui: &UiModel,
    sync: SyncState,
    _color: bool,
) {
    let text = if ui.help {
        "Esc dismiss  j/k scroll  ? help  q quit".to_string()
    } else if ui.report.is_some() {
        if ui.sync_actions_enabled(sync) && !ui.sync_key_held {
            "p preview  S sync  Esc back  j/k scroll  ? help  q quit".to_string()
        } else if ui.sync_actions_enabled(sync) {
            "p preview  Esc back  j/k scroll  ? help  q quit".to_string()
        } else {
            "Esc back  j/k scroll  ? help  q quit".to_string()
        }
    } else {
        footer_keys(ui, sync)
    };
    frame.render_widget(Paragraph::new(text), area);
}

fn footer_keys(ui: &UiModel, sync: SyncState) -> String {
    let mut parts = vec![
        "1-7 screens",
        if ui.in_detail {
            "j/k scroll"
        } else {
            "j/k rows"
        },
        "? help",
        "q quit",
    ];
    if ui.sync_actions_enabled(sync) {
        parts.insert(
            0,
            if ui.sync_key_held {
                "p preview"
            } else {
                "p preview  S sync"
            },
        );
    }
    if ui.in_detail {
        parts.insert(0, "Esc back");
        if ui.mutations_enabled(sync) {
            match ui.screen {
                Screen::Wants => parts.insert(0, "W apply  D delete"),
                Screen::Titles => parts.insert(0, "W apply"),
                Screen::Holds => parts.insert(0, "A approve  X reject"),
                Screen::Overview | Screen::Jobs | Screen::Nodes | Screen::BoxListing => {}
            }
        }
    } else {
        parts.insert(0, "Enter detail");
    }
    let fits = |parts: &[&str]| parts.join("  ").len() <= usize::from(ui.cols);
    if !fits(&parts) {
        parts.retain(|part| !matches!(*part, "1-7 screens" | "j/k rows" | "j/k scroll"));
    }
    for (long, short) in [
        ("p preview  S sync", "p  S"),
        ("Enter detail", "Enter"),
        ("W apply  D delete", "W  D"),
        ("A approve  X reject", "A  X"),
        ("W apply", "W"),
        ("Esc back", "Esc"),
        ("? help", "?"),
    ] {
        if fits(&parts) {
            break;
        }
        for part in &mut parts {
            if *part == long {
                *part = short;
            }
        }
    }
    parts.join("  ")
}

pub(crate) fn render_table(
    frame: &mut Frame<'_>,
    area: Rect,
    ui: &UiModel,
    projection: &Projection,
    sync: SyncState,
    color: bool,
    list_failed: bool,
) {
    if let Some(label) = empty_label(sync, projection, list_failed) {
        frame.render_widget(Paragraph::new(label), area);
        return;
    }
    if projection.rows.is_empty() {
        return;
    }
    let plan = plan_columns(&projection.headers, area.width);
    if plan.is_empty() {
        return;
    }
    let widths: Vec<Constraint> = plan.iter().map(|c| Constraint::Length(c.width)).collect();
    let header = Row::new(plan.iter().map(|c| c.header))
        .style(Style::default().add_modifier(Modifier::BOLD));
    let rows: Vec<Row<'_>> = projection
        .rows
        .iter()
        .map(|row| {
            Row::new(plan.iter().map(|col| {
                let text = row.cells.get(col.index).map(String::as_str).unwrap_or("");
                let cell = Cell::from(align_cell(text, col.width as usize, col.numeric));
                if matches!(col.header, "PHASE" | "READY" | "FACT") {
                    match text {
                        "failed" | "refused" | "missing" => cell.style(if color {
                            Style::default().fg(Color::Red)
                        } else {
                            Style::default().add_modifier(Modifier::BOLD)
                        }),
                        "not-ready"
                        | "scheduling paused"
                        | "encoding paused"
                        | "scheduling/encode paused" => {
                            cell.style(style_status(SyncState::Stale, color))
                        }
                        "ready" | "installed" | "satisfied" => {
                            cell.style(style_status(SyncState::Current, color))
                        }
                        _ => cell,
                    }
                } else {
                    cell
                }
            }))
        })
        .collect();
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(style_focus(color));
    let mut state = TableState::default().with_selected(Some(ui.selected));
    frame.render_stateful_widget(table, area, &mut state);
}

fn empty_label(
    sync: SyncState,
    projection: &Projection,
    list_failed: bool,
) -> Option<&'static str> {
    if list_failed || matches!(projection.listing, ListingKind::Unavailable) {
        return Some("unavailable");
    }
    match projection.listing {
        ListingKind::KnownEmpty(text) if sync == SyncState::Current => Some(text),
        ListingKind::KnownEmpty(_) => Some(sync.label()),
        ListingKind::Rows => None,
        ListingKind::Unavailable => None,
    }
}

pub(crate) fn render_detail(
    frame: &mut Frame<'_>,
    area: Rect,
    ui: &UiModel,
    projection: &Projection,
    _color: bool,
) {
    let caption = projection.hold_caption.then_some(HOLD_CAPTION);
    let (facts, cap) = if caption.is_some() && area.height > 0 {
        let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };
    let value_width = facts.width.saturating_sub(15) as usize;
    let mut wrapped: Vec<Line<'_>> = Vec::new();
    for line in &projection.detail {
        let parts = wrap_text(&line.value, value_width.max(1));
        let mut parts = parts.into_iter();
        let first = parts.next().unwrap_or_default();
        wrapped.push(Line::from(format!("{:<14} {first}", line.label)));
        for cont in parts {
            wrapped.push(Line::from(format!("{:<14} {cont}", "")));
        }
    }
    let skip = ui.detail_offset as usize;
    let visible: Vec<Line<'_>> = wrapped.into_iter().skip(skip).collect();
    frame.render_widget(Paragraph::new(visible), facts);
    if let Some(cap) = cap {
        frame.render_widget(Paragraph::new(HOLD_CAPTION), cap);
    }
}
