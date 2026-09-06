use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::EventStream;
use ratatui::Terminal;
use ratatui::backend::Backend;
use tokio::signal::unix::{SignalKind, signal};
use tokio::task::JoinSet;
use tokio_stream::StreamExt;

use crate::actions::{self, Mutation, MutationOutcome, MutationTarget, PreparedWrite};
use crate::disk::DiskWatch;
use crate::interaction::{can_submit, project_ui};
use crate::keys::command_from_event;
use crate::model::UiModel;
use crate::session::Session;
use crate::update::{Update, UpdateEffect, apply};

enum Work {
    Prepared {
        mutation: Mutation,
        target: MutationTarget,
        result: Box<Result<PreparedWrite, MutationOutcome>>,
    },
    Finished(MutationOutcome),
}

pub async fn run(socket: Option<&Path>, color: bool) -> anyhow::Result<i32> {
    crate::terminal::install_panic_hook();
    let (guard, mut terminal) = crate::terminal::TerminalGuard::enter().context("terminal")?;
    let result = run_loop(&mut terminal, socket, color).await;
    drop(guard);
    result
}

async fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    socket: Option<&Path>,
    color: bool,
) -> anyhow::Result<i32> {
    let mut session = Session::new(socket);
    let mut ui = UiModel::default();
    let mut events = EventStream::new();
    let mut term_signal = signal(SignalKind::terminate()).context("sigterm")?;
    let mut work = JoinSet::new();
    let mut disk = DiskWatch::default();
    let mut redraw = tokio::time::interval(Duration::from_millis(100));
    redraw.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut dirty = true;
    let mut reconnect_at = None;
    session.bootstrap().await;
    loop {
        if session.needs_reconnect && reconnect_at.is_none() {
            reconnect_at = Some(tokio::time::Instant::now() + session.backoff());
        }
        tokio::select! {
            _ = term_signal.recv() => return Ok(0),
            event = events.next() => {
                let event = event.context("terminal event stream ended")??;
                let command = command_from_event(&event);
                let row_count = crate::projection::project(&session.cache, ui.screen, ui.selected, crate::clock::unix_now()).rows.len();
                let page = usize::from(ui.rows.saturating_sub(6));
                let effect = apply(Update { ui: &mut ui, sync: session.sync, row_count, page }, command);
                match effect {
                    UpdateEffect::Quit => return Ok(0),
                    UpdateEffect::None => {},
                    UpdateEffect::RequestMutation(mutation) => {
                        match (ui.rendered_target.clone(), session.api.clone()) {
                            (Some(target), Some(api)) if can_submit(&session, &ui, &target) => {
                                ui.message = Some("checking selected object".into());
                                work.spawn(async move {
                                    let result = actions::prepare(&api, mutation, &target).await;
                                    Work::Prepared { mutation, target, result: Box::new(result) }
                                });
                            }
                            _ => {
                                ui.mutation_pending = false;
                                ui.message = Some("selection changed; open its detail again".into());
                                ui.clear_action_selection();
                            }
                        }
                    }
                }
                dirty = true;
            }
            event = session.recv() => {
                if let Some(event) = event {
                    session.apply_event(event);
                    if !session.sync.writes_allowed() { ui.rendered_target = None; }
                    dirty = true;
                }
            }
            Some(result) = work.join_next(), if !work.is_empty() => {
                match result.context("mutation task failed")? {
                    Work::Prepared { mutation, target, result } => {
                        match (*result, session.api.clone()) {
                            (Ok(prepared), Some(api)) if mutation.allowed_on(ui.screen) && can_submit(&session, &ui, &target) => {
                                ui.message = Some("submitting one versioned write".into());
                                work.spawn(async move { Work::Finished(actions::submit(&api, prepared).await) });
                            }
                            (Err(outcome), _) => finish(&mut session, &mut ui, outcome).await,
                            _ => finish(&mut session, &mut ui, MutationOutcome::Conflict).await,
                        }
                    }
                    Work::Finished(outcome) => finish(&mut session, &mut ui, outcome).await,
                }
                dirty = true;
            }
            _ = tick.tick() => {
                dirty = true;
            }
            _ = redraw.tick() => {
                if reconnect_at.is_some_and(|at| tokio::time::Instant::now() >= at) {
                    reconnect_at = None;
                    session.bootstrap().await;
                    ui.clear_action_selection();
                    dirty = true;
                }
                if session.sync.writes_allowed() { reconnect_at = None; }
                let root = session.cache.live_kind(mediaops_core::Kind::Cluster).find_map(|obj| {
                    match &obj.spec {
                        mediaops_core::Spec::Cluster(spec) if !spec.library_root.is_empty() => Some(spec.library_root.as_str()),
                        _ => None,
                    }
                });
                dirty |= disk.update(root);
                if dirty {
                    let size = terminal.size().map_err(|e| anyhow::anyhow!("{e}"))?;
                    ui.cols = size.width;
                    ui.rows = size.height;
                    let projection = project_ui(&session, &mut ui);
                    let observation = disk.observation();
                    let mut display = ui.clone();
                    if !session.sync.writes_allowed() && !ui.mutation_pending && session.message.is_some() {
                        display.message.clone_from(&session.message);
                    }
                    terminal.draw(|frame| crate::view::render(frame, &display, session.sync, &projection, &observation, color, session.list_failed))
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    dirty = false;
                }
            }
        }
    }
}

async fn finish(session: &mut Session, ui: &mut UiModel, outcome: MutationOutcome) {
    ui.message = Some(outcome.message());
    ui.mutation_pending = false;
    ui.clear_action_selection();
    session.bootstrap().await;
}
