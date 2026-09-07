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
use crate::interaction::{can_submit, handle_event, project_ui};
use crate::model::UiModel;
use crate::report::SyncReport;
use crate::session::Session;
use crate::update::UpdateEffect;

enum Work {
    Prepared {
        mutation: Mutation,
        target: MutationTarget,
        result: Box<Result<PreparedWrite, MutationOutcome>>,
    },
    Finished(MutationOutcome),
    SyncDone {
        request_id: String,
        dry_run: bool,
        result: Result<Box<mediaops_core::HomeObject>, String>,
    },
}

pub async fn run(socket: Option<&Path>, color: bool) -> anyhow::Result<i32> {
    crate::terminal::install_panic_hook();
    let term_signal = signal(SignalKind::terminate()).context("sigterm")?;
    let (guard, mut terminal) = crate::terminal::TerminalGuard::enter().context("terminal")?;
    let result = run_loop(&mut terminal, socket, color, term_signal).await;
    drop(guard);
    result
}

async fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    socket: Option<&Path>,
    color: bool,
    mut term_signal: tokio::signal::unix::Signal,
) -> anyhow::Result<i32> {
    let mut session = Session::new(socket);
    let mut ui = UiModel {
        keyboard_event_types: crate::terminal::keyboard_event_types_enabled(),
        ..UiModel::default()
    };
    let mut events = EventStream::new();
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
        ui.connection_message = if session.sync.writes_allowed() {
            None
        } else {
            session.message.clone()
        };
        if session.needs_reconnect && reconnect_at.is_none() {
            reconnect_at = Some(tokio::time::Instant::now() + session.backoff());
        }
        tokio::select! {
            _ = term_signal.recv() => return Ok(0),
            event = events.next() => {
                let event = event.context("terminal event stream ended")??;
                let effect = handle_event(&session, &mut ui, &event);
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
                    UpdateEffect::RequestSync { dry_run } => {
                        match session.api.clone() {
                            Some(api) => {
                                let request_id = mediaops_home_client::new_sync_request_id();
                                ui.message = Some(if dry_run {
                                    "previewing eligible copies".into()
                                } else {
                                    "planning a fresh sync".into()
                                });
                                work.spawn(async move {
                                    Work::SyncDone {
                                        request_id: request_id.clone(),
                                        dry_run,
                                        result: api
                                            .sync(&request_id, dry_run)
                                            .await
                                            .map(Box::new)
                                            .map_err(|err| format!("sync `{request_id}`: {err}")),
                                    }
                                });
                            }
                            None => {
                                ui.sync_pending = false;
                                ui.message = Some("Home API unavailable".into());
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
                                ui.message = Some(match mutation {
                                    Mutation::ApplyWant => "applying Want",
                                    Mutation::DeleteWant => "deleting Want",
                                    Mutation::ApproveHold => "recording approval",
                                    Mutation::RejectHold => "recording rejection",
                                }.into());
                                work.spawn(async move { Work::Finished(actions::submit(&api, prepared).await) });
                            }
                            (Err(outcome), _) => finish(&mut session, &mut ui, outcome).await,
                            _ => finish(&mut session, &mut ui, MutationOutcome::Conflict).await,
                        }
                    }
                    Work::Finished(outcome) => finish(&mut session, &mut ui, outcome).await,
                    Work::SyncDone {
                        request_id,
                        dry_run,
                        result,
                    } => {
                        let refresh = result.is_ok() && !dry_run;
                        crate::update::complete_sync(&mut ui, result.map(|obj| {
                            SyncReport::from_object(request_id, dry_run, &obj)
                        }));
                        if refresh {
                            session.bootstrap().await;
                        }
                    }
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
                    if !session.sync.writes_allowed() && !ui.mutation_pending && !ui.sync_pending {
                        display.message = Some(match (&session.message, reconnect_at) {
                            (Some(message), Some(at)) => format!("retry in {}s; actions disabled; {message}", at.saturating_duration_since(tokio::time::Instant::now()).as_secs().saturating_add(1)),
                            (Some(message), None) => format!("reconnecting; actions disabled; {message}"),
                            (None, _) => "loading Home API state; actions disabled".into(),
                        });
                    }
                    terminal.draw(|frame| crate::view::render(frame, &display, session.sync, &projection, &observation, color, session.list_failed))
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    if !ui.undersize() && ui.input.is_none() && !ui.help && display.message == ui.message {
                        ui.message_unseen = false;
                    }
                    dirty = false;
                }
            }
        }
    }
}

async fn finish(session: &mut Session, ui: &mut UiModel, outcome: MutationOutcome) {
    ui.completion_message(Some(outcome.message()));
    ui.mutation_pending = false;
    ui.clear_action_selection();
    session.bootstrap().await;
}
