//! Draw the operations ledger. Tokens from DESIGN.md.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};

use crate::disk::DiskObservation;
use crate::model::{SyncState, UiModel};
use crate::projection::Projection;
use crate::view_chrome::{
    render_detail, render_footer, render_help, render_masthead, render_rule, render_status,
    render_table, render_undersize,
};

pub fn render(
    frame: &mut Frame<'_>,
    ui: &UiModel,
    sync: SyncState,
    projection: &Projection,
    disk: &DiskObservation,
    color: bool,
    list_failed: bool,
) {
    if ui.undersize() {
        render_undersize(frame, color);
        return;
    }
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(frame.area());
    render_masthead(frame, chunks[0], ui, sync, disk, color);
    render_rule(frame, chunks[1]);
    if ui.help {
        render_help(frame, chunks[2], color);
    } else if ui.split_detail() {
        let panes = Layout::horizontal([
            Constraint::Percentage(58),
            Constraint::Length(1),
            Constraint::Min(24),
        ])
        .split(chunks[2]);
        render_table(frame, panes[0], ui, projection, sync, color, list_failed);
        frame.render_widget(
            ratatui::widgets::Paragraph::new("|\n".repeat(usize::from(panes[1].height))),
            panes[1],
        );
        render_detail(frame, panes[2], ui, projection, color);
    } else if ui.in_detail {
        render_detail(frame, chunks[2], ui, projection, color);
    } else {
        render_table(frame, chunks[2], ui, projection, sync, color, list_failed);
    }
    render_rule(frame, chunks[3]);
    render_status(frame, chunks[4], ui);
    render_footer(frame, chunks[5], ui, sync, color);
}
