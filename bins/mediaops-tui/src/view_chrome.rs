//! Resource navigation, framed panes, status and contextual shortcuts.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::disk::DiskObservation;
use crate::format::{fmt_age, fmt_bytes};
use crate::geometry::{DETAIL_LABEL_WIDTH, Pane, Shell};
use crate::model::{InputMode, Screen, SyncState, UiModel};
use crate::projection::{HOLD_CAPTION, ListingKind, Projection};
use crate::view_text::{align_cell, help_lines, plan_columns, wrap_text};

fn style_focus(color: bool) -> Style {
    style_accent(color).add_modifier(Modifier::REVERSED)
}

fn style_accent(color: bool) -> Style {
    let base = Style::default().add_modifier(Modifier::BOLD);
    if color { base.fg(Color::Cyan) } else { base }
}

fn style_muted(color: bool) -> Style {
    if color {
        Style::default().add_modifier(Modifier::DIM)
    } else {
        Style::default()
    }
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
    sync: SyncState,
    disk: &DiskObservation,
    color: bool,
) {
    let disk_text = match disk {
        DiskObservation::Ready { bytes, age } => {
            format!("disk {} free {}", fmt_bytes(*bytes), fmt_age(age.as_secs()))
        }
        DiskObservation::Unavailable { .. } => "disk unavailable".into(),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("mediaops", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(sync.label(), style_status(sync, color)),
            Span::raw("  |  "),
            Span::styled(disk_text, style_muted(color)),
        ])),
        area,
    );
}

pub(crate) fn render_tabs(frame: &mut Frame<'_>, area: Rect, ui: &UiModel, color: bool) {
    let mut spans = Vec::new();
    for screen in Screen::ALL {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("{} {}", screen.number(), screen.title()),
            if screen == ui.screen {
                style_focus(color)
            } else {
                style_muted(color)
            },
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn mutation_hints(ui: &UiModel, sync: SyncState) -> &'static str {
    if !ui.mutations_enabled(sync) {
        return "";
    }
    match ui.screen {
        Screen::Wants => "W apply  D delete",
        Screen::Titles => "W apply",
        Screen::Holds => "A approve  X reject",
        _ => "",
    }
}

pub(crate) fn render_hints(
    frame: &mut Frame<'_>,
    area: Rect,
    ui: &UiModel,
    sync: SyncState,
    color: bool,
) {
    let text = match &ui.input {
        Some(InputMode::Command { text }) => {
            let prefix = text.trim().to_lowercase();
            let aliases: Vec<_> = [
                ("overview", "overview"),
                ("want(s)", "wants"),
                ("job(s)", "jobs"),
                ("hold(s)", "holds"),
                ("title(s)", "titles"),
                ("node(s)", "nodes"),
                ("box", "box"),
                ("help", "help"),
                ("quit", "quit"),
            ]
            .into_iter()
            .filter_map(|(label, alias)| alias.starts_with(&prefix).then_some(label))
            .collect();
            if aliases.is_empty() {
                "No command matches; :help lists resource aliases".into()
            } else {
                format!(": {}", aliases.join(" "))
            }
        }
        Some(InputMode::Filter { .. }) => "Live filter: literal text, case-insensitive".into(),
        None => {
            let action = mutation_hints(ui, sync);
            if !action.is_empty() {
                format!("{action}  |  Enter never writes")
            } else if ui.help || ui.report.is_some() || ui.in_detail {
                "j/k scroll  PgUp/Dn page  g/G first/last".into()
            } else {
                "Tab resource  j/k rows  g/G first/last  d detail".into()
            }
        }
    };
    frame.render_widget(
        Paragraph::new(crate::sanitize::clip(&text, usize::from(area.width)).0)
            .style(style_muted(color)),
        area,
    );
}

pub(crate) fn render_pane(
    frame: &mut Frame<'_>,
    pane: Pane,
    title: &str,
    focused: bool,
    color: bool,
) {
    let style = if focused {
        style_accent(color)
    } else {
        style_muted(color)
    };
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(style)
            .title(Span::styled(
                crate::sanitize::clip(title, usize::from(pane.outer.width.saturating_sub(2))).0,
                style,
            )),
        pane.outer,
    );
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
    if let Some(input) = &ui.input {
        let (prefix, query) = match input {
            InputMode::Filter { .. } => ("/", &ui.filter),
            InputMode::Command { text } => (":", text),
        };
        let suffix = if matches!(input, InputMode::Filter { .. }) {
            format!(" | {}", filter_count(sync, projection, list_failed))
        } else {
            String::new()
        };
        let width = usize::from(area.width).saturating_sub(prefix.width() + suffix.width() + 1);
        let tail = input_tail(query, width);
        let cursor = prefix.width() + tail.width();
        frame.render_widget(Paragraph::new(format!("{prefix}{tail} {suffix}")), area);
        frame.set_cursor_position((area.x + u16::try_from(cursor).unwrap_or(0), area.y));
        return;
    }
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

fn input_tail(text: &str, width: usize) -> String {
    let clean = crate::sanitize::sanitize(text);
    let mut used = 0;
    let chars: Vec<char> = clean
        .chars()
        .rev()
        .take_while(|c| {
            used += UnicodeWidthChar::width(*c).unwrap_or(0);
            used <= width
        })
        .collect();
    chars.into_iter().rev().collect()
}

fn filter_count(sync: SyncState, projection: &Projection, list_failed: bool) -> String {
    if list_failed || projection.listing == ListingKind::Unavailable {
        "unavailable".into()
    } else if sync != SyncState::Current {
        format!("{} cached matches; {}", projection.rows.len(), sync.label())
    } else {
        format!("{} matching", projection.rows.len())
    }
}

fn position_status(
    ui: &UiModel,
    sync: SyncState,
    projection: &Projection,
    list_failed: bool,
) -> String {
    let shell = Shell::for_ui(ui);
    let visible = usize::from(shell.overlay.inner.height);
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
            report.lines(usize::from(shell.overlay.inner.width)).len(),
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
        let total = projection
            .detail
            .iter()
            .map(|line| wrap_text(&line.value, shell.detail.value_width()).len())
            .sum();
        let position = line_position(
            usize::from(ui.detail_offset),
            total,
            usize::from(shell.detail.facts(projection.hold_caption).height),
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
    color: bool,
) {
    let text = if let Some(input) = &ui.input {
        if matches!(input, InputMode::Filter { .. }) {
            "Enter keep  Esc cancel  Ctrl-U clear  ? text  Ctrl-C quit".into()
        } else {
            "Enter go  Esc cancel  Ctrl-U clear  :help help  Ctrl-C quit".into()
        }
    } else if ui.help {
        "Esc dismiss  j/k scroll  ? help  q quit".into()
    } else {
        let mut parts = vec![
            if ui.in_detail || ui.report.is_some() {
                "Esc back"
            } else {
                "Enter detail"
            },
            "/ filter",
            ": resource",
        ];
        if ui.sync_actions_enabled(sync) {
            parts.push("p preview");
            if !ui.sync_key_held {
                parts.push("S sync");
            }
        }
        parts.extend(["? help", "q quit"]);
        if parts.join("  ").width() > usize::from(area.width) {
            // Preserve full action, help and quit labels. Navigation remains
            // visible in the header and can use its familiar one-cell keys.
            for part in &mut parts {
                *part = match *part {
                    "/ filter" => "/",
                    ": resource" => ":",
                    other => other,
                };
            }
        }
        parts.join("  ")
    };
    frame.render_widget(Paragraph::new(text).style(style_muted(color)), area);
}

pub(crate) fn render_table(
    frame: &mut Frame<'_>,
    pane: Pane,
    ui: &UiModel,
    projection: &Projection,
    sync: SyncState,
    color: bool,
    list_failed: bool,
) {
    let count = if ui.filter.is_empty() && projection.listing != ListingKind::NoMatches {
        if list_failed || projection.listing == ListingKind::Unavailable {
            "unavailable".into()
        } else if sync != SyncState::Current {
            format!("{} cached", projection.rows.len())
        } else {
            projection.rows.len().to_string()
        }
    } else {
        filter_count(sync, projection, list_failed)
    };
    let focused = !ui.in_detail && ui.input.is_none();
    let label = format!(
        " {} ({count}) {} ",
        ui.screen.title(),
        if focused { "[list]" } else { "" }
    );
    render_pane(frame, pane, &label, focused, color);
    render_filter_scope(frame, pane, ui, color);
    if let Some(label) = empty_label(sync, projection, list_failed) {
        frame.render_widget(Paragraph::new(label), pane.inner);
        return;
    }
    if projection.rows.is_empty() {
        return;
    }
    let plan = plan_columns(&projection.headers, pane.table_width());
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
        .highlight_symbol("> ")
        .row_highlight_style(if focused {
            style_focus(color)
        } else {
            Style::default().add_modifier(Modifier::REVERSED)
        });
    let mut state = TableState::default()
        .with_selected(Some(ui.selected))
        .with_offset(ui.table_offset);
    frame.render_stateful_widget(table, pane.inner, &mut state);
}

fn empty_label(
    sync: SyncState,
    projection: &Projection,
    list_failed: bool,
) -> Option<&'static str> {
    if list_failed || projection.listing == ListingKind::Unavailable {
        return Some("unavailable");
    }
    match projection.listing {
        ListingKind::KnownEmpty(text) if sync == SyncState::Current => Some(text),
        ListingKind::NoMatches if sync == SyncState::Current => Some("no matching resources"),
        ListingKind::KnownEmpty(_) | ListingKind::NoMatches => Some(sync.label()),
        ListingKind::Rows | ListingKind::Unavailable => None,
    }
}

pub(crate) fn render_detail(
    frame: &mut Frame<'_>,
    pane: Pane,
    ui: &UiModel,
    projection: &Projection,
    color: bool,
) {
    let focused = ui.in_detail && ui.input.is_none();
    render_pane(
        frame,
        pane,
        if focused {
            " Detail [focus] "
        } else {
            " Detail [preview] "
        },
        focused,
        color,
    );
    if !ui.split_detail() {
        render_filter_scope(frame, pane, ui, color);
    }
    let facts = pane.facts(projection.hold_caption);
    let mut wrapped: Vec<Line<'_>> = Vec::new();
    for line in &projection.detail {
        for (index, part) in wrap_text(&line.value, pane.value_width())
            .into_iter()
            .enumerate()
        {
            let label = if index == 0 { line.label } else { "" };
            wrapped.push(Line::from(vec![
                Span::styled(
                    format!(
                        "{label:<width$} ",
                        width = usize::from(DETAIL_LABEL_WIDTH - 1)
                    ),
                    style_muted(color),
                ),
                Span::raw(part),
            ]));
        }
    }
    let offset =
        usize::from(ui.detail_offset).min(wrapped.len().saturating_sub(usize::from(facts.height)));
    if wrapped.is_empty() {
        frame.render_widget(Paragraph::new("no selected resource"), facts);
    } else {
        frame.render_widget(
            Paragraph::new(wrapped.into_iter().skip(offset).collect::<Vec<_>>()),
            facts,
        );
    }
    if projection.hold_caption && pane.inner.height > 0 {
        frame.render_widget(
            Paragraph::new(HOLD_CAPTION).style(style_muted(color)),
            Rect::new(pane.inner.x, pane.inner.bottom() - 1, pane.inner.width, 1),
        );
    }
}

fn render_filter_scope(frame: &mut Frame<'_>, pane: Pane, ui: &UiModel, color: bool) {
    if ui.filter.is_empty() {
        return;
    }
    let filter = format!(
        " /{} ",
        crate::sanitize::clip(&ui.filter, usize::from(pane.outer.width.saturating_sub(6))).0
    );
    frame.render_widget(
        Paragraph::new(filter).style(style_muted(color)),
        Rect::new(
            pane.outer.x + 1,
            pane.outer.bottom() - 1,
            pane.outer.width.saturating_sub(2),
            1,
        ),
    );
}
