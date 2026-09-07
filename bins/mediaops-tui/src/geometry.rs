//! One cell geometry contract for rendering, scrolling, and identity gates.

use ratatui::layout::{Constraint, Layout, Rect};

use crate::model::UiModel;

pub const DETAIL_LABEL_WIDTH: u16 = 15;
pub const SELECTION_GUTTER: u16 = 2;

#[derive(Debug, Clone, Copy)]
pub struct Pane {
    pub outer: Rect,
    pub inner: Rect,
}

impl Pane {
    fn new(outer: Rect) -> Self {
        Self {
            outer,
            inner: Rect::new(
                outer.x.saturating_add(1),
                outer.y.saturating_add(1),
                outer.width.saturating_sub(2),
                outer.height.saturating_sub(2),
            ),
        }
    }

    pub fn facts(self, hold_caption: bool) -> Rect {
        Rect {
            height: self.inner.height.saturating_sub(u16::from(hold_caption)),
            ..self.inner
        }
    }

    pub fn value_width(self) -> usize {
        usize::from(self.inner.width.saturating_sub(DETAIL_LABEL_WIDTH)).max(1)
    }

    pub fn table_rows(self) -> usize {
        usize::from(self.inner.height.saturating_sub(1))
    }

    pub fn table_width(self) -> u16 {
        self.inner.width.saturating_sub(SELECTION_GUTTER)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Shell {
    pub header: Rect,
    pub hints: Rect,
    pub tabs: Rect,
    pub status: Rect,
    pub footer: Rect,
    pub overlay: Pane,
    pub list: Pane,
    pub detail: Pane,
}

impl Shell {
    pub fn for_ui(ui: &UiModel) -> Self {
        Self::new(Rect::new(0, 0, ui.cols, ui.rows), ui.split_detail())
    }

    pub fn new(area: Rect, split: bool) -> Self {
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);
        let overlay = Pane::new(rows[3]);
        let (list, detail) = if split {
            let panes = Layout::horizontal([
                Constraint::Percentage(58),
                Constraint::Length(1),
                Constraint::Min(50),
            ])
            .split(rows[3]);
            (Pane::new(panes[0]), Pane::new(panes[2]))
        } else {
            (overlay, overlay)
        };
        Self {
            header: rows[0],
            hints: rows[1],
            tabs: rows[2],
            status: rows[4],
            footer: rows[5],
            overlay,
            list,
            detail,
        }
    }

    pub fn page(self, ui: &UiModel, hold_caption: bool) -> usize {
        if ui.help || ui.report.is_some() {
            usize::from(self.overlay.inner.height)
        } else if ui.in_detail {
            usize::from(self.detail.facts(hold_caption).height)
        } else {
            self.list.table_rows()
        }
        .max(1)
    }
}
