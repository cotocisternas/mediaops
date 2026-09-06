use mediaops_core::{Kind, Spec};
use ratatui::layout::{Constraint, Layout, Rect};

use crate::actions::MutationTarget;
use crate::cache::ObjectKey;
use crate::inventory::committed_inventory_generation;
use crate::model::{Screen, UiModel};
use crate::projection::{Projection, project};
use crate::session::Session;

pub fn project_ui(session: &Session, ui: &mut UiModel) -> Projection {
    let now = crate::clock::unix_now();
    let mut projection = project(&session.cache, ui.screen, ui.selected, now);
    if let Some(key) = &ui.selected_key {
        match projection.rows.iter().position(|row| {
            row.kind == key.kind
                && row.name == key.name
                && Some(&row.uid) == ui.selected_uid.as_ref()
        }) {
            Some(index) => ui.selected = index,
            None => {
                ui.clear_action_selection();
                ui.select_row(ui.selected, projection.rows.len());
            }
        }
    }
    ui.selected = ui.selected.min(projection.rows.len().saturating_sub(1));
    projection = project(&session.cache, ui.screen, ui.selected, now);
    let Some(row) = projection.rows.get(ui.selected) else {
        ui.select_row(0, 0);
        ui.clear_action_selection();
        return projection;
    };
    ui.selected_key = Some(ObjectKey::new(row.kind, &row.name));
    ui.selected_uid = Some(row.uid.clone());
    ui.selected_rv = Some(row.rv);
    let body = Rect::new(0, 0, ui.cols, ui.rows.saturating_sub(5));
    let detail = if ui.split_detail() {
        Layout::horizontal([
            Constraint::Percentage(58),
            Constraint::Length(1),
            Constraint::Min(24),
        ])
        .split(body)[2]
    } else {
        body
    };
    let value_width = usize::from(detail.width.saturating_sub(15)).max(1);
    let total_lines: usize = projection
        .detail
        .iter()
        .map(|line| crate::view_text::wrap_text(&line.value, value_width).len())
        .sum();
    let visible_lines = usize::from(
        detail
            .height
            .saturating_sub(u16::from(projection.hold_caption)),
    );
    ui.detail_offset = ui
        .detail_offset
        .min(u16::try_from(total_lines.saturating_sub(visible_lines)).unwrap_or(u16::MAX));
    let identity_lines = crate::view_text::wrap_text(&row.identity, value_width).len();
    ui.identity_clipped = ui.detail_offset > 0 || identity_lines > visible_lines;
    if !ui.mutation_pending {
        ui.rendered_target = if ui.mutations_enabled(session.sync) {
            match ui.screen {
                Screen::Wants | Screen::Holds => Some(MutationTarget {
                    key: ObjectKey::new(row.kind, &row.name),
                    uid: row.uid.clone(),
                    resource_version: row.rv,
                    epoch: session.cache.epoch(),
                }),
                Screen::Titles => {
                    let key = ObjectKey::new(Kind::Want, &row.name);
                    let want = session
                        .cache
                        .get(&key)
                        .and_then(|entry| entry.object.as_ref());
                    Some(MutationTarget {
                        key,
                        uid: want.map(|o| o.metadata.uid.clone()).unwrap_or_default(),
                        resource_version: want.map(|o| o.metadata.resource_version).unwrap_or(0),
                        epoch: session.cache.epoch(),
                    })
                }
                Screen::Overview | Screen::Jobs | Screen::Nodes | Screen::BoxListing => None,
            }
        } else {
            None
        };
    }
    projection
}

pub fn can_submit(session: &Session, ui: &UiModel, target: &MutationTarget) -> bool {
    if !session.sync.writes_allowed()
        || !ui.in_detail
        || ui.help
        || ui.undersize()
        || ui.identity_clipped
        || ui.rendered_target.as_ref() != Some(target)
        || !target.matches_cache(&session.cache)
    {
        return false;
    }
    if target.key.kind == Kind::Hold {
        let generation =
            committed_inventory_generation(session.cache.live(), crate::clock::unix_now());
        return session.cache.get(&target.key).and_then(|entry| entry.object.as_ref()).is_some_and(|hold| {
            matches!(&hold.spec, Spec::Hold(spec) if spec.decision == mediaops_core::HoldDecisionSpec::Empty)
                && matches!(&hold.status, mediaops_core::StatusBody::Hold(st) if Some(st.list_generation) == generation)
        });
    }
    true
}
