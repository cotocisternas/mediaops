use mediaops_core::{Kind, Spec};

use crate::actions::MutationTarget;
use crate::cache::ObjectKey;
use crate::inventory::committed_inventory_generation;
use crate::model::{Screen, UiModel};
use crate::projection::{Projection, filtered_rows, select_detail};
use crate::session::Session;

/// Reconcile live selection and geometry without arming an undrawn target.
pub fn reconcile_ui(session: &Session, ui: &mut UiModel) -> Projection {
    let now = crate::clock::unix_now();
    let mut projection = filtered_rows(&session.cache, ui.screen, now, &ui.filter);
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
    select_detail(&session.cache, ui.screen, &mut projection, ui.selected, now);
    let shell = crate::geometry::Shell::for_ui(ui);
    let visible_rows = shell.list.table_rows().max(1);
    ui.table_offset = ui.table_offset.min(ui.selected);
    if ui.selected >= ui.table_offset.saturating_add(visible_rows) {
        ui.table_offset = ui.selected.saturating_sub(visible_rows - 1);
    }
    ui.table_offset = ui
        .table_offset
        .min(projection.rows.len().saturating_sub(visible_rows));
    crate::update::clamp_overlays(ui);
    let Some(row) = projection.rows.get(ui.selected) else {
        ui.select_row(0, 0);
        ui.clear_action_selection();
        return projection;
    };
    ui.selected_key = Some(ObjectKey::new(row.kind, &row.name));
    ui.selected_uid = Some(row.uid.clone());
    ui.selected_rv = Some(row.rv);
    clamp_detail_scroll(ui, &projection);
    let value_width = shell.detail.value_width();
    let visible_lines = usize::from(shell.detail.facts(projection.hold_caption).height);
    let identity_lines = crate::view_text::wrap_text(&row.identity, value_width).len();
    ui.identity_clipped = ui.detail_offset > 0 || identity_lines > visible_lines;
    projection
}

/// Capture mutation eligibility only for the projection about to be drawn.
pub fn project_ui(session: &Session, ui: &mut UiModel) -> Projection {
    let projection = reconcile_ui(session, ui);
    let Some(row) = projection.rows.get(ui.selected) else {
        return projection;
    };
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

/// Production event path: reconcile before navigation, but never recapture a
/// target before a draw. Multiple key events can arrive between redraw ticks.
pub fn handle_event(
    session: &Session,
    ui: &mut UiModel,
    event: &crossterm::event::Event,
) -> crate::update::UpdateEffect {
    let projection = reconcile_ui(session, ui);
    let command = crate::keys::command_for_ui(event, ui);
    let page = crate::geometry::Shell::for_ui(ui).page(ui, projection.hold_caption);
    let effect = crate::update::apply(
        crate::update::Update {
            ui,
            sync: session.sync,
            row_count: projection.rows.len(),
            page,
        },
        command,
    );
    clamp_detail_scroll(ui, &projection);
    effect
}

/// Also called on key events, so End then Up works even between redraws.
pub fn clamp_detail_scroll(ui: &mut UiModel, projection: &Projection) {
    let shell = crate::geometry::Shell::for_ui(ui);
    let value_width = shell.detail.value_width();
    let total_lines: usize = projection
        .detail
        .iter()
        .map(|line| crate::view_text::wrap_text(&line.value, value_width).len())
        .sum();
    let visible_lines = usize::from(shell.detail.facts(projection.hold_caption).height);
    ui.detail_offset = ui
        .detail_offset
        .min(u16::try_from(total_lines.saturating_sub(visible_lines)).unwrap_or(u16::MAX));
}

pub fn can_submit(session: &Session, ui: &UiModel, target: &MutationTarget) -> bool {
    if !session.sync.writes_allowed()
        || !ui.in_detail
        || ui.help
        || ui.input.is_some()
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
