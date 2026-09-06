//! Masthead, table, detail, status, footer.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Row, Table, TableState};

use crate::disk::DiskObservation;
use crate::format::{fmt_age, fmt_bytes};
use crate::model::{Screen, SyncState, UiModel};
use crate::projection::{HOLD_CAPTION, ListingKind, Projection};
use crate::view_text::{align_cell, plan_columns, wrap_text};

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

pub(crate) fn render_help(frame: &mut Frame<'_>, area: Rect, _color: bool) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("1 Overview  2 Wants  3 Jobs  4 Holds  5 Titles  6 Nodes  7 Box"),
            Line::from("Tab / Shift-Tab  next/prev screen"),
            Line::from("j k arrows  PageUp PageDown  Home End  rows"),
            Line::from("Enter  detail   Esc  back   ?  help   q  quit"),
            Line::from("W apply Want   D delete Want   A approve Hold   X reject Hold"),
            Line::from("mutations only in selected detail; Enter never writes"),
        ]),
        area,
    );
}

pub(crate) fn render_status(frame: &mut Frame<'_>, area: Rect, ui: &UiModel) {
    let text = match (ui.mutation_pending, ui.message.as_deref()) {
        (true, Some(msg)) => format!("pending  {msg}"),
        (true, None) => "pending".into(),
        (false, Some(msg)) => msg.to_string(),
        (false, None) => String::new(),
    };
    frame.render_widget(Paragraph::new(crate::sanitize::sanitize(&text)), area);
}

pub(crate) fn render_footer(
    frame: &mut Frame<'_>,
    area: Rect,
    ui: &UiModel,
    sync: SyncState,
    _color: bool,
) {
    let text = if ui.help {
        "Esc dismiss  ? help  q quit".to_string()
    } else {
        footer_keys(ui, sync)
    };
    frame.render_widget(Paragraph::new(text), area);
}

fn footer_keys(ui: &UiModel, sync: SyncState) -> String {
    let mut parts = vec!["1-7 screens", "j/k rows", "? help", "q quit"];
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
    if parts.join("  ").len() > usize::from(ui.cols) {
        parts.retain(|part| !matches!(*part, "1-7 screens" | "j/k rows"));
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
                align_cell(text, col.width as usize, col.numeric)
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
