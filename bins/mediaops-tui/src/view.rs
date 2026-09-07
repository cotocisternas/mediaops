//! Resource browser shell. Geometry is shared with interaction and scrolling.

use ratatui::Frame;

use crate::disk::DiskObservation;
use crate::geometry::Shell;
use crate::model::{InputMode, SyncState, UiModel};
use crate::projection::Projection;
use crate::report::render_report;
use crate::view_chrome::{
    render_detail, render_footer, render_help, render_hints, render_masthead, render_pane,
    render_status, render_table, render_tabs, render_undersize,
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
    let shell = Shell::new(frame.area(), ui.split_detail());
    render_masthead(frame, shell.header, sync, disk, color);
    render_hints(frame, shell.hints, ui, sync, color);
    render_tabs(frame, shell.tabs, ui, color);
    let filter_editing = matches!(ui.input, Some(InputMode::Filter { .. }));
    let focused = ui.input.is_none();
    if ui.help && !filter_editing {
        render_pane(
            frame,
            shell.overlay,
            if focused {
                " Help [focus] "
            } else {
                " Help [preview] "
            },
            focused,
            color,
        );
        render_help(frame, shell.overlay.inner, ui);
    } else if let Some(report) = ui.report.as_ref().filter(|_| !filter_editing) {
        render_pane(
            frame,
            shell.overlay,
            if focused {
                " Sync report [focus] "
            } else {
                " Sync report [preview] "
            },
            focused,
            color,
        );
        render_report(frame, shell.overlay.inner, report, ui.report_offset);
    } else {
        if ui.split_detail() || !ui.in_detail {
            render_table(frame, shell.list, ui, projection, sync, color, list_failed);
        }
        if ui.split_detail() || ui.in_detail {
            render_detail(frame, shell.detail, ui, projection, color);
        }
    }
    render_status(frame, shell.status, ui, sync, projection, list_failed);
    render_footer(frame, shell.footer, ui, sync, color);
}
