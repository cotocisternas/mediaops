use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use mediaops_core::{
    HoldDecisionSpec, HoldSpec, HoldStatus, HomeObject, Kind, NodeSpec, NodeStatus, Spec,
    StatusBody, WantSpec, WorkerKind,
};
use mediaops_tui::actions::Mutation;
use mediaops_tui::geometry::Shell;
use mediaops_tui::interaction::{can_submit, clamp_detail_scroll, project_ui};
use mediaops_tui::keys::{Command, command_for_ui};
use mediaops_tui::model::{InputMode, Screen, SyncState, UiModel};
use mediaops_tui::projection::{HOLD_CAPTION, ListingKind, Projection, project_filtered};
use mediaops_tui::update::{Update, UpdateEffect, apply};
use mediaops_tui::{Session, disk::DiskObservation};
use ratatui::{
    Terminal,
    backend::TestBackend,
    style::{Color, Modifier},
};

fn want(name: &str, uid: &str, rv: i64) -> HomeObject {
    let mut object = HomeObject::new(
        Kind::Want,
        name,
        Spec::Want(WantSpec {
            title_id: name.into(),
        }),
        StatusBody::empty(Kind::Want),
    );
    object.metadata.uid = uid.into();
    object.metadata.resource_version = rv;
    object
}

fn session(objects: Vec<HomeObject>) -> Session {
    let mut session = Session::new(None);
    let epoch = session.cache.bump_epoch();
    session.cache.install_baseline(epoch, objects);
    session.sync = SyncState::Current;
    session
}

fn ui() -> UiModel {
    UiModel {
        screen: Screen::Wants,
        cols: 60,
        rows: 16,
        ..Default::default()
    }
}

fn command(session: &Session, ui: &mut UiModel, command: Command) -> UpdateEffect {
    let projection = project_filtered(&session.cache, ui.screen, ui.selected, now(), &ui.filter);
    let page = Shell::for_ui(ui).page(ui, projection.hold_caption);
    let effect = apply(
        Update {
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

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn press(session: &Session, ui: &mut UiModel, code: KeyCode) -> UpdateEffect {
    let cmd = command_for_ui(&Event::Key(KeyEvent::new(code, KeyModifiers::NONE)), ui);
    let effect = command(session, ui, cmd);
    project_ui(session, ui);
    effect
}

fn type_text(session: &Session, ui: &mut UiModel, text: &str) {
    for c in text.chars() {
        assert!(matches!(
            press(session, ui, KeyCode::Char(c)),
            UpdateEffect::None
        ));
    }
}

fn draw(
    ui: &UiModel,
    sync: SyncState,
    projection: &Projection,
    color: bool,
) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(ui.cols, ui.rows)).unwrap();
    terminal
        .draw(|frame| {
            mediaops_tui::view::render(
                frame,
                ui,
                sync,
                projection,
                &DiskObservation::unavailable(),
                color,
                false,
            )
        })
        .unwrap();
    terminal
}

fn lines(terminal: &Terminal<TestBackend>) -> Vec<String> {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

#[test]
fn all_action_and_navigation_letters_are_unicode_input_until_enter_or_escape() {
    let session = session(vec![want("movie:key:映画.1999", "one", 1)]);
    for opener in [Command::Filter, Command::CommandInput] {
        let mut ui = ui();
        ui.in_detail = true;
        project_ui(&session, &mut ui);
        command(&session, &mut ui, opener);
        type_text(&session, &mut ui, "qWDA XpS1237?gGdj/k:映画é.[]");
        let text = match &ui.input {
            Some(InputMode::Filter { .. }) => &ui.filter,
            Some(InputMode::Command { text }) => text,
            None => panic!("input closed"),
        };
        assert_eq!(text, "qWDA XpS1237?gGdj/k:映画é.[]");
        assert!(!ui.mutation_pending && !ui.sync_pending && !ui.sync_key_held);
        assert!(ui.rendered_target.is_none());
        assert!(matches!(
            press(&session, &mut ui, KeyCode::Enter),
            UpdateEffect::None
        ));
        assert!(!ui.mutation_pending && !ui.sync_pending);
        assert_eq!(ui.screen, Screen::Wants);
    }
}

#[test]
fn input_ignores_paste_repeat_release_but_preserves_sync_release_and_ctrl_c() {
    let session = session(Vec::new());
    let mut ui = ui();
    ui.keyboard_event_types = true;
    ui.sync_key_held = true;
    command(&session, &mut ui, Command::Filter);
    for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
        let event = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('W'),
            KeyModifiers::NONE,
            kind,
        ));
        assert_eq!(command_for_ui(&event, &ui), Command::Ignore);
    }
    assert_eq!(
        command_for_ui(&Event::Paste("S qW\n".into()), &ui),
        Command::Ignore
    );
    let release = Event::Key(KeyEvent::new_with_kind(
        KeyCode::Char('S'),
        KeyModifiers::SHIFT,
        KeyEventKind::Release,
    ));
    let cmd = command_for_ui(&release, &ui);
    assert_eq!(cmd, Command::ReleaseSync);
    command(&session, &mut ui, cmd);
    assert!(!ui.sync_key_held);
    let ctrl_c = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(command_for_ui(&ctrl_c, &ui), Command::Quit);
    assert!(ui.filter.is_empty());
    for cmd in [
        Command::Mutate(Mutation::ApplyWant),
        Command::RunSync,
        Command::PreviewSync,
        Command::Screen(Screen::Jobs),
        Command::EnterDetail,
    ] {
        assert!(matches!(
            command(&session, &mut ui, cmd),
            UpdateEffect::None
        ));
    }
    assert_eq!(ui.screen, Screen::Wants);
    assert!(!ui.sync_pending && !ui.mutation_pending);
}

#[test]
fn backspace_ctrl_u_filter_commit_cancel_and_resource_reset() {
    let session = session(vec![want("movie:key:映画.1999", "one", 1)]);
    let mut ui = ui();
    command(&session, &mut ui, Command::Filter);
    type_text(&session, &mut ui, "映画é");
    press(&session, &mut ui, KeyCode::Backspace);
    assert_eq!(ui.filter, "映画");
    press(&session, &mut ui, KeyCode::Enter);
    assert_eq!(ui.filter, "映画");
    assert!(ui.input.is_none());
    press(&session, &mut ui, KeyCode::Char('/'));
    type_text(&session, &mut ui, "zzz");
    press(&session, &mut ui, KeyCode::Esc);
    assert_eq!(ui.filter, "映画");
    press(&session, &mut ui, KeyCode::Char('/'));
    let event = Event::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    let cmd = command_for_ui(&event, &ui);
    command(&session, &mut ui, cmd);
    assert!(ui.filter.is_empty());
    type_text(&session, &mut ui, "映画");
    press(&session, &mut ui, KeyCode::Enter);
    press(&session, &mut ui, KeyCode::Esc);
    assert!(ui.filter.is_empty());
    ui.filter = "映画".into();
    press(&session, &mut ui, KeyCode::Tab);
    assert_eq!(ui.screen, Screen::Jobs);
    assert!(ui.filter.is_empty());
}

#[test]
fn commands_accept_only_closed_resource_help_and_quit_aliases() {
    let session = session(Vec::new());
    for (alias, expected) in [
        ("overview", Screen::Overview),
        ("want", Screen::Wants),
        ("wants", Screen::Wants),
        ("job", Screen::Jobs),
        ("jobs", Screen::Jobs),
        ("hold", Screen::Holds),
        ("holds", Screen::Holds),
        ("title", Screen::Titles),
        ("titles", Screen::Titles),
        ("node", Screen::Nodes),
        ("nodes", Screen::Nodes),
        ("box", Screen::BoxListing),
    ] {
        let mut ui = ui();
        press(&session, &mut ui, KeyCode::Char(':'));
        type_text(&session, &mut ui, alias);
        assert!(matches!(
            press(&session, &mut ui, KeyCode::Enter),
            UpdateEffect::None
        ));
        assert_eq!(ui.screen, expected);
    }
    for alias in ["sync", "p", "S", "W", "apply", "jobs extra", ""] {
        let mut ui = ui();
        press(&session, &mut ui, KeyCode::Char(':'));
        type_text(&session, &mut ui, alias);
        assert!(matches!(
            press(&session, &mut ui, KeyCode::Enter),
            UpdateEffect::None
        ));
        assert!(ui.message.as_ref().unwrap().contains("Unknown command"));
        assert!(ui.message.as_ref().unwrap().contains(":help"));
        assert!(!ui.sync_pending && !ui.mutation_pending);
    }
    for alias in ["help", "quit", "q"] {
        let mut ui = ui();
        press(&session, &mut ui, KeyCode::Char(':'));
        type_text(&session, &mut ui, alias);
        let effect = press(&session, &mut ui, KeyCode::Enter);
        if alias == "help" {
            assert!(ui.help);
        } else {
            assert!(matches!(effect, UpdateEffect::Quit));
        }
    }
}

#[test]
fn filter_matches_case_insensitive_literal_display_cells_and_exact_identity() {
    let session = session(vec![
        want("movie:key:the.matrix.1999", "matrix", 1),
        want("movie:key:matrix.reloaded.2003", "reloaded", 2),
        want("movie:key:映画.1999", "unicode", 3),
        want("movie:key:ΟΣ.1999", "greek", 4),
    ]);
    let query = |text| project_filtered(&session.cache, Screen::Wants, 0, now(), text);
    assert_eq!(query("MATRIX").rows.len(), 2);
    assert_eq!(query("2003").rows[0].uid, "reloaded");
    assert_eq!(query("movie:key:the.matrix.1999").rows[0].uid, "matrix");
    assert_eq!(query("映画").rows[0].uid, "unicode");
    for sigma in ["οσ", "ος", "Σ", "σ", "ς"] {
        let projection = query(sigma);
        assert_eq!(projection.rows[0].uid, "greek", "{sigma}");
        assert_eq!(projection.detail[0].value, "movie:key:ΟΣ.1999");
    }
    assert!(query(".*").rows.is_empty());
}

#[test]
fn filtered_detail_and_guarded_target_follow_exact_selected_source_row() {
    let session = session(vec![
        want("movie:key:alpha.1999", "a", 1),
        want("movie:key:beta.1999", "b", 2),
        want("movie:key:beta.2000", "c", 3),
    ]);
    let mut ui = ui();
    press(&session, &mut ui, KeyCode::Char('/'));
    type_text(&session, &mut ui, "beta");
    press(&session, &mut ui, KeyCode::Enter);
    press(&session, &mut ui, KeyCode::Char('G'));
    assert_eq!(ui.selected, 1);
    press(&session, &mut ui, KeyCode::Char('d'));
    let projection = project_ui(&session, &mut ui);
    let target = ui.rendered_target.as_ref().unwrap();
    assert_eq!(target.uid, "c");
    assert_eq!(target.key.name, projection.rows[ui.selected].name);
    assert_eq!(projection.detail[0].value, target.key.name);
    assert!(can_submit(&session, &ui, target));
    press(&session, &mut ui, KeyCode::Esc);
    press(&session, &mut ui, KeyCode::Char('g'));
    assert_eq!(ui.selected, 0);
    press(&session, &mut ui, KeyCode::Enter);
    assert_eq!(ui.rendered_target.as_ref().unwrap().uid, "b");
}

#[test]
fn filtered_selection_survives_insertions_and_disarms_on_uid_replacement() {
    let mut session = session(vec![want("movie:key:beta.2000", "old", 1)]);
    let mut ui = ui();
    ui.filter = "beta".into();
    ui.in_detail = true;
    project_ui(&session, &mut ui);
    let target = ui.rendered_target.clone().unwrap();
    let epoch = session.cache.epoch();
    session.cache.apply_event(
        epoch,
        mediaops_home_client::WatchEvent::Added(want("movie:key:beta.1999", "first", 2)),
    );
    project_ui(&session, &mut ui);
    assert_eq!(ui.selected, 1);
    assert_eq!(ui.rendered_target.as_ref().unwrap().uid, "old");
    session.cache.apply_event(
        epoch,
        mediaops_home_client::WatchEvent::Deleted(want("movie:key:beta.2000", "old", 3)),
    );
    session.cache.apply_event(
        epoch,
        mediaops_home_client::WatchEvent::Added(want("movie:key:beta.2000", "new", 4)),
    );
    project_ui(&session, &mut ui);
    assert!(!ui.in_detail);
    assert!(ui.rendered_target.is_none());
    assert!(!can_submit(&session, &ui, &target));
}

#[test]
fn preparation_cannot_submit_after_input_navigation_resize_or_disconnect() {
    for change in [
        Command::Filter,
        Command::CommandInput,
        Command::Screen(Screen::Titles),
        Command::Back,
        Command::Resize { cols: 59, rows: 16 },
    ] {
        let session = session(vec![want("movie:key:alpha.1999", "a", 1)]);
        let mut ui = ui();
        ui.in_detail = true;
        project_ui(&session, &mut ui);
        let target = ui.rendered_target.clone().unwrap();
        assert!(matches!(
            command(&session, &mut ui, Command::Mutate(Mutation::ApplyWant)),
            UpdateEffect::RequestMutation(_)
        ));
        assert!(can_submit(&session, &ui, &target));
        command(&session, &mut ui, change);
        project_ui(&session, &mut ui);
        assert!(!can_submit(&session, &ui, &target));
        if ui.input.is_some() {
            command(&session, &mut ui, Command::InputCancel);
        }
        ui.in_detail = true;
        project_ui(&session, &mut ui);
        assert!(
            !can_submit(&session, &ui, &target),
            "cancelled preparation rearmed"
        );
    }
    let mut session = session(vec![want("movie:key:alpha.1999", "a", 1)]);
    let mut ui = ui();
    ui.in_detail = true;
    project_ui(&session, &mut ui);
    let target = ui.rendered_target.clone().unwrap();
    session.sync = SyncState::Stale;
    assert!(!can_submit(&session, &ui, &target));
}

#[test]
fn zero_matches_never_turn_unavailable_or_stale_into_known_empty() {
    let mut session = session(vec![want("movie:key:alpha.1999", "a", 1)]);
    let mut ui = ui();
    ui.filter = "missing".into();
    let projection = project_ui(&session, &mut ui);
    assert_eq!(projection.listing, ListingKind::NoMatches);
    let text = lines(&draw(&ui, session.sync, &projection, false)).join("\n");
    assert!(
        text.contains("no matching resources")
            && text.contains("0 matching")
            && text.contains("/missing")
    );
    assert!(ui.rendered_target.is_none());
    assert!(matches!(
        press(&session, &mut ui, KeyCode::Enter),
        UpdateEffect::None
    ));
    assert!(!ui.in_detail);
    session.sync = SyncState::Stale;
    let text = lines(&draw(&ui, session.sync, &projection, false)).join("\n");
    assert!(text.contains("NOT CURRENT") && text.contains("/missing"));
    assert!(!text.contains("no matching resources") && !text.contains("nothing happening"));
    session.sync = SyncState::Current;
    ui.screen = Screen::Holds;
    let projection = project_ui(&session, &mut ui);
    assert_eq!(projection.listing, ListingKind::Unavailable);
    let text = lines(&draw(&ui, session.sync, &projection, false)).join("\n");
    assert!(text.contains("unavailable"));
    assert!(!text.contains("0 matching") && !text.contains("nothing on hold"));
}

#[test]
fn tabs_panes_and_selection_express_focus_with_color_and_monochrome() {
    let session = session(vec![want("movie:key:alpha.1999", "a", 1)]);
    for (cols, rows) in [(60, 16), (80, 24), (140, 40)] {
        for color in [false, true] {
            for screen in Screen::ALL {
                let mut ui = UiModel {
                    screen,
                    cols,
                    rows,
                    ..Default::default()
                };
                let projection = project_ui(&session, &mut ui);
                let terminal = draw(&ui, session.sync, &projection, color);
                let text = lines(&terminal);
                assert!(text[0].contains("Current"));
                for resource in Screen::ALL {
                    assert!(text[2].contains(&format!(
                        "{} {}",
                        resource.number(),
                        resource.title()
                    )));
                }
                let active = text[2]
                    .find(&format!("{} {}", screen.number(), screen.title()))
                    .unwrap() as u16;
                let tab = &terminal.backend().buffer()[(active, 2)];
                assert!(tab.modifier.contains(Modifier::REVERSED | Modifier::BOLD));
                assert_eq!(tab.fg, if color { Color::Cyan } else { Color::Reset });
                assert!(text[3].contains("[list]"));
                assert!(
                    text.last().unwrap().contains("? help")
                        && text.last().unwrap().contains("q quit")
                );
                assert!(
                    terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .all(|cell| cell.bg == Color::Reset)
                );
            }
            let mut ui = UiModel { cols, rows, ..ui() };
            let projection = project_ui(&session, &mut ui);
            let terminal = draw(&ui, session.sync, &projection, color);
            let row = &terminal.backend().buffer()[(1, 5)];
            assert_eq!(row.symbol(), ">");
            assert!(row.modifier.contains(Modifier::REVERSED));
            press(&session, &mut ui, KeyCode::Enter);
            let projection = project_ui(&session, &mut ui);
            let terminal = draw(&ui, session.sync, &projection, color);
            let shell = Shell::for_ui(&ui);
            assert!(lines(&terminal)[3].contains("Detail [focus]"));
            assert!(
                terminal.backend().buffer()[(shell.detail.outer.x, shell.detail.outer.y)]
                    .modifier
                    .contains(Modifier::BOLD)
            );
            if cols >= 120 {
                assert!(
                    !terminal.backend().buffer()[(shell.list.outer.x, shell.list.outer.y)]
                        .modifier
                        .contains(Modifier::BOLD)
                );
            }
        }
    }
}

#[test]
fn command_suggestions_and_unicode_editor_keep_text_inside_status_cells() {
    let session = session(Vec::new());
    let mut ui = ui();
    press(&session, &mut ui, KeyCode::Char(':'));
    type_text(&session, &mut ui, "jo");
    let projection = project_ui(&session, &mut ui);
    let text = lines(&draw(&ui, session.sync, &projection, false));
    assert!(text[1].contains("job(s)"));
    assert!(text[14].contains(":jo"));
    press(&session, &mut ui, KeyCode::Esc);
    press(&session, &mut ui, KeyCode::Char('/'));
    type_text(&session, &mut ui, &format!("{}終", "映画é".repeat(40)));
    let projection = project_ui(&session, &mut ui);
    let mut terminal = draw(&ui, session.sync, &projection, false);
    let text = lines(&terminal);
    assert!(text[14].contains('終') && text[14].contains("0 matching"));
    assert!(terminal.get_cursor_position().unwrap().x < 60);
    assert!(!text.join("\n").contains('\u{1b}'));
}

#[test]
fn geometry_pins_hold_caption_and_clamps_detail_after_end_up_and_resize() {
    let inventory = HomeObject::new(
        Kind::Node,
        "inventory",
        Spec::Node(NodeSpec {
            worker_kind: WorkerKind::Inventory,
        }),
        StatusBody::Node(NodeStatus {
            ready: true,
            last_heartbeat_unix: now(),
            list_completed_unix: now(),
            list_generation: 1,
            ..Default::default()
        }),
    );
    let hold = HomeObject::new(
        Kind::Hold,
        "movie:key:alpha.1999-release-b",
        Spec::Hold(HoldSpec {
            title_id: "movie:key:alpha.1999".into(),
            release_id: "release-b".into(),
            decision: HoldDecisionSpec::Empty,
        }),
        StatusBody::Hold(HoldStatus {
            list_generation: 1,
            reason: "A long reason with words and Unicode 映画 ".repeat(60),
            ..Default::default()
        }),
    );
    let session = session(vec![inventory, hold]);
    let mut ui = UiModel {
        screen: Screen::Holds,
        in_detail: true,
        ..ui()
    };
    for (cols, rows) in [(60, 16), (120, 16), (140, 40), (80, 24), (60, 16)] {
        command(&session, &mut ui, Command::Resize { cols, rows });
        project_ui(&session, &mut ui);
        command(&session, &mut ui, Command::RowEnd);
        let end = ui.detail_offset;
        assert!(end > 0 && end < u16::MAX);
        // No intervening redraw is needed for Up to leave the end.
        command(&session, &mut ui, Command::RowDelta(-1));
        assert_eq!(ui.detail_offset, end - 1);
        let projection = project_ui(&session, &mut ui);
        let terminal = draw(&ui, session.sync, &projection, false);
        let shell = Shell::for_ui(&ui);
        assert!(
            lines(&terminal)[usize::from(shell.detail.inner.bottom() - 1)].contains(HOLD_CAPTION)
        );
        assert!(ui.identity_clipped && ui.rendered_target.is_none());
        command(&session, &mut ui, Command::RowHome);
        project_ui(&session, &mut ui);
        assert!(!ui.identity_clipped);
        assert!(ui.rendered_target.is_some());
    }
    let shell = Shell::for_ui(&ui);
    assert_eq!(shell.list.table_rows(), 8);
    assert_eq!(shell.detail.facts(true).height, 8);
    assert_eq!(shell.detail.value_width(), 43);
}

#[test]
fn undersize_notice_quit_works_even_when_an_editor_was_open() {
    let session = session(Vec::new());
    for opener in [Command::Filter, Command::CommandInput] {
        let mut ui = ui();
        command(&session, &mut ui, opener);
        command(&session, &mut ui, Command::Resize { cols: 40, rows: 10 });
        assert!(matches!(
            press(&session, &mut ui, KeyCode::Char('q')),
            UpdateEffect::Quit
        ));
        for key in ['S', 'W', 'D', 'A', 'X', 'p'] {
            assert!(matches!(
                press(&session, &mut ui, KeyCode::Char(key)),
                UpdateEffect::None
            ));
        }
        assert!(!ui.mutation_pending && !ui.sync_pending);
    }
}

#[test]
fn two_holds_for_one_title_filter_and_target_the_exact_release() {
    let inventory = HomeObject::new(
        Kind::Node,
        "inventory",
        Spec::Node(NodeSpec {
            worker_kind: WorkerKind::Inventory,
        }),
        StatusBody::Node(NodeStatus {
            ready: true,
            last_heartbeat_unix: now(),
            list_completed_unix: now(),
            list_generation: 1,
            ..Default::default()
        }),
    );
    let hold = |release: &str| {
        let mut object = HomeObject::new(
            Kind::Hold,
            format!("movie:key:alpha.1999-{release}"),
            Spec::Hold(HoldSpec {
                title_id: "movie:key:alpha.1999".into(),
                release_id: release.into(),
                decision: HoldDecisionSpec::Empty,
            }),
            StatusBody::Hold(HoldStatus {
                list_generation: 1,
                ..Default::default()
            }),
        );
        object.metadata.uid = release.into();
        object
    };
    let session = session(vec![inventory, hold("release-a"), hold("release-b")]);
    let mut ui = UiModel {
        screen: Screen::Holds,
        ..ui()
    };
    press(&session, &mut ui, KeyCode::Char('/'));
    type_text(&session, &mut ui, "release-b");
    press(&session, &mut ui, KeyCode::Enter);
    press(&session, &mut ui, KeyCode::Enter);
    let projection = project_ui(&session, &mut ui);
    assert_eq!(projection.rows.len(), 1);
    assert_eq!(projection.detail[0].value, "movie:key:alpha.1999-release-b");
    assert!(
        projection
            .detail
            .iter()
            .any(|line| line.label == "release_id" && line.value == "release-b")
    );
    let target = ui.rendered_target.as_ref().unwrap();
    assert_eq!(target.uid, "release-b");
    assert_eq!(target.key.name, projection.detail[0].value);
    assert!(can_submit(&session, &ui, target));
}

#[test]
fn borders_reduce_identity_space_and_disable_actions_when_name_cannot_fit() {
    let name = format!("movie:key:{}", "a".repeat(400));
    let session = session(vec![want(&name, "a", 1)]);
    let mut ui = UiModel {
        in_detail: true,
        ..ui()
    };
    project_ui(&session, &mut ui);
    assert!(ui.identity_clipped);
    assert!(ui.rendered_target.is_none());
    command(&session, &mut ui, Command::Resize { cols: 80, rows: 24 });
    project_ui(&session, &mut ui);
    assert!(!ui.identity_clipped);
    assert!(ui.rendered_target.is_some());
}

#[test]
fn help_and_report_end_up_and_resize_keep_scroll_in_the_actual_frame() {
    let session = session(Vec::new());
    for help in [false, true] {
        let mut ui = ui();
        ui.help = help;
        ui.connection_message = Some(format!(
            "{} FINAL_DETAIL",
            "connection diagnostics ".repeat(150)
        ));
        if !help {
            let object = HomeObject::new(
                Kind::Sync,
                "sync-test",
                Spec::Sync(Default::default()),
                StatusBody::Sync(mediaops_core::SyncStatus {
                    entries: vec![mediaops_core::SyncEntry {
                        remote_path: "a/very/long/path/".repeat(600),
                        reason: "FINAL_DETAIL".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            );
            ui.report = Some(mediaops_tui::report::SyncReport::from_object(
                "sync-test".into(),
                true,
                &object,
            ));
        }
        for (cols, rows) in [(60, 16), (140, 40), (80, 24), (60, 16)] {
            command(&session, &mut ui, Command::Resize { cols, rows });
            command(&session, &mut ui, Command::RowEnd);
            let end = if help {
                ui.help_offset
            } else {
                ui.report_offset
            };
            assert!(end > 0 && end < u16::MAX);
            let projection = project_ui(&session, &mut ui);
            let screen = lines(&draw(&ui, session.sync, &projection, false));
            assert!(screen.join("\n").contains("FINAL_DETAIL"));
            assert!(screen[usize::from(rows - 2)].starts_with("lines "));
            command(&session, &mut ui, Command::RowDelta(-1));
            assert_eq!(
                if help {
                    ui.help_offset
                } else {
                    ui.report_offset
                },
                end - 1
            );
            assert_eq!(ui.selected, 0);
        }
    }
}

fn event_without_draw(session: &Session, ui: &mut UiModel, code: KeyCode) -> UpdateEffect {
    mediaops_tui::interaction::handle_event(
        session,
        ui,
        &Event::Key(KeyEvent::new(code, KeyModifiers::NONE)),
    )
}

#[test]
fn rapid_filter_then_detail_reconciles_identity_without_rearming_before_draw() {
    let session = session(vec![
        want("movie:key:beta.1999", "beta", 1),
        want("movie:key:delta.1999", "delta", 2),
    ]);
    let mut ui = UiModel {
        selected: 1,
        in_detail: true,
        ..ui()
    };
    project_ui(&session, &mut ui);
    let old_target = ui.rendered_target.clone().unwrap();
    assert_eq!(old_target.uid, "delta");
    for code in [
        KeyCode::Char('/'),
        KeyCode::Char('b'),
        KeyCode::Char('e'),
        KeyCode::Char('t'),
        KeyCode::Char('a'),
        KeyCode::Enter,
        KeyCode::Enter,
    ] {
        assert!(matches!(
            event_without_draw(&session, &mut ui, code),
            UpdateEffect::None
        ));
        assert!(ui.rendered_target.is_none());
        assert!(!can_submit(&session, &ui, &old_target));
    }
    assert!(ui.in_detail);
    assert_eq!(ui.selected_uid.as_deref(), Some("beta"));
    let projection = project_ui(&session, &mut ui);
    assert!(
        ui.in_detail,
        "redraw closed the newly opened filtered detail"
    );
    assert_eq!(projection.detail[0].value, "movie:key:beta.1999");
    assert_eq!(ui.rendered_target.as_ref().unwrap().uid, "beta");
}

#[test]
fn cancelling_command_editor_preserves_the_scrolled_viewport() {
    let session = session(
        (0..50)
            .map(|n| want(&format!("movie:key:title.{n:02}"), &n.to_string(), n))
            .collect(),
    );
    let mut ui = ui();
    for _ in 0..3 {
        event_without_draw(&session, &mut ui, KeyCode::PageDown);
    }
    project_ui(&session, &mut ui);
    let original = (ui.selected, ui.table_offset);
    assert!(original.1 > 0);
    for key in [
        KeyCode::Char(':'),
        KeyCode::Char('j'),
        KeyCode::Char('o'),
        KeyCode::Backspace,
    ] {
        event_without_draw(&session, &mut ui, key);
        assert_eq!((ui.selected, ui.table_offset), original);
    }
    mediaops_tui::interaction::handle_event(
        &session,
        &mut ui,
        &Event::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
    );
    assert_eq!((ui.selected, ui.table_offset), original);
    event_without_draw(&session, &mut ui, KeyCode::Esc);
    assert_eq!((ui.selected, ui.table_offset), original);
}

fn completed_report() -> mediaops_tui::report::SyncReport {
    let object = HomeObject::new(
        Kind::Sync,
        "finished-preview",
        Spec::Sync(Default::default()),
        StatusBody::Sync(mediaops_core::SyncStatus {
            phase: mediaops_core::SyncPhase::Captured,
            entries: vec![mediaops_core::SyncEntry {
                remote_path: "a/long/path/".repeat(200),
                ..Default::default()
            }],
            ..Default::default()
        }),
    );
    mediaops_tui::report::SyncReport::from_object("finished-preview".into(), true, &object)
}

#[test]
fn sync_completion_keeps_live_filter_body_until_editor_finishes() {
    let session = session(vec![want("movie:key:beta.1999", "beta", 1)]);
    for finish_key in [KeyCode::Enter, KeyCode::Esc] {
        let mut ui = ui();
        assert!(matches!(
            event_without_draw(&session, &mut ui, KeyCode::Char('p')),
            UpdateEffect::RequestSync { dry_run: true }
        ));
        event_without_draw(&session, &mut ui, KeyCode::Char('/'));
        mediaops_tui::update::complete_sync(&mut ui, Ok(completed_report()));
        assert!(!ui.sync_pending);
        assert!(ui.report.is_none());
        assert!(ui.deferred_report.is_some());
        for key in "beta".chars() {
            event_without_draw(&session, &mut ui, KeyCode::Char(key));
        }
        let projection = project_ui(&session, &mut ui);
        let text = lines(&draw(&ui, session.sync, &projection, false)).join("\n");
        assert!(text.contains("Beta (1999)"));
        assert!(!text.contains("Sync report"));
        assert!(ui.input.is_some());
        event_without_draw(&session, &mut ui, finish_key);
        assert!(ui.input.is_none());
        assert!(ui.deferred_report.is_none());
        assert_eq!(ui.report.as_ref().unwrap().request_id, "finished-preview");
        let projection = project_ui(&session, &mut ui);
        assert!(
            lines(&draw(&ui, session.sync, &projection, false))
                .join("\n")
                .contains("Sync report [focus]")
        );
    }
}

#[test]
fn completion_feedback_survives_input_edits_accept_cancel_and_command_navigation() {
    let session = session(Vec::new());
    for opener in ['/', ':'] {
        for finish_key in [KeyCode::Enter, KeyCode::Esc] {
            let mut ui = ui();
            event_without_draw(&session, &mut ui, KeyCode::Char(opener));
            mediaops_tui::update::complete_sync(
                &mut ui,
                Err("preview failed: fixture unavailable".into()),
            );
            event_without_draw(&session, &mut ui, KeyCode::Char('x'));
            event_without_draw(&session, &mut ui, KeyCode::Backspace);
            event_without_draw(&session, &mut ui, KeyCode::Char('y'));
            mediaops_tui::interaction::handle_event(
                &session,
                &mut ui,
                &Event::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            );
            for key in "jobs".chars() {
                event_without_draw(&session, &mut ui, KeyCode::Char(key));
            }
            assert_eq!(
                ui.message.as_deref(),
                Some("preview failed: fixture unavailable")
            );
            event_without_draw(&session, &mut ui, finish_key);
            assert_eq!(
                ui.message.as_deref(),
                Some("preview failed: fixture unavailable")
            );
            assert!(ui.message_unseen);
            let projection = project_ui(&session, &mut ui);
            assert!(
                lines(&draw(&ui, session.sync, &projection, false))[14]
                    .contains("preview failed: fixture unavailable")
            );
        }
    }
    // A command result must not overwrite a concurrently arriving completion.
    let mut ui = ui();
    event_without_draw(&session, &mut ui, KeyCode::Char(':'));
    mediaops_tui::update::complete_sync(&mut ui, Err("preview failed".into()));
    for key in "invalid".chars() {
        event_without_draw(&session, &mut ui, KeyCode::Char(key));
    }
    event_without_draw(&session, &mut ui, KeyCode::Enter);
    assert!(
        ui.message
            .as_ref()
            .unwrap()
            .starts_with("preview failed; Unknown command")
    );
}

#[test]
fn deferred_report_survives_command_resource_navigation() {
    let session = session(Vec::new());
    let mut ui = ui();
    event_without_draw(&session, &mut ui, KeyCode::Char(':'));
    mediaops_tui::update::complete_sync(&mut ui, Ok(completed_report()));
    for key in "jobs".chars() {
        event_without_draw(&session, &mut ui, KeyCode::Char(key));
    }
    event_without_draw(&session, &mut ui, KeyCode::Enter);
    assert_eq!(ui.screen, Screen::Jobs);
    assert_eq!(ui.report.as_ref().unwrap().request_id, "finished-preview");
    assert!(ui.deferred_report.is_none());
}

#[test]
fn filtered_list_keeps_row_position_and_narrow_detail_keeps_filter_scope() {
    let session = session(vec![
        want("movie:key:beta.1999", "one", 1),
        want("movie:key:beta.2000", "two", 2),
    ]);
    let mut ui = UiModel {
        filter: "beta".into(),
        selected: 1,
        ..ui()
    };
    let projection = project_ui(&session, &mut ui);
    let text = lines(&draw(&ui, session.sync, &projection, false));
    assert!(text[14].contains("row 2 of 2"));
    assert!(text[3].contains("2 matching") && text[13].contains("/beta"));
    event_without_draw(&session, &mut ui, KeyCode::Enter);
    let projection = project_ui(&session, &mut ui);
    let text = lines(&draw(&ui, session.sync, &projection, false));
    assert!(text[3].contains("Detail [focus]"));
    assert!(text[13].contains("/beta"));
}

#[test]
fn help_and_report_focus_defer_to_command_editor_in_color_and_monochrome() {
    let session = session(Vec::new());
    for help in [false, true] {
        for color in [false, true] {
            let mut ui = UiModel { help, ..ui() };
            if !help {
                ui.report = Some(completed_report());
            }
            event_without_draw(&session, &mut ui, KeyCode::Char(':'));
            let projection = project_ui(&session, &mut ui);
            let terminal = draw(&ui, session.sync, &projection, color);
            assert!(!lines(&terminal)[3].contains("[focus]"));
            let border = &terminal.backend().buffer()[(0, 3)];
            assert!(!border.modifier.contains(Modifier::BOLD));
            assert_ne!(border.fg, Color::Cyan);
            event_without_draw(&session, &mut ui, KeyCode::Esc);
            let projection = project_ui(&session, &mut ui);
            assert!(lines(&draw(&ui, session.sync, &projection, color))[3].contains("[focus]"));
        }
    }
}

fn long_hold_session() -> Session {
    let inventory = HomeObject::new(
        Kind::Node,
        "inventory",
        Spec::Node(NodeSpec {
            worker_kind: WorkerKind::Inventory,
        }),
        StatusBody::Node(NodeStatus {
            ready: true,
            last_heartbeat_unix: now(),
            list_completed_unix: now(),
            list_generation: 1,
            ..Default::default()
        }),
    );
    let hold = HomeObject::new(
        Kind::Hold,
        "movie:key:beta.1999-release-b",
        Spec::Hold(HoldSpec {
            title_id: "movie:key:beta.1999".into(),
            release_id: "release-b".into(),
            decision: HoldDecisionSpec::Empty,
        }),
        StatusBody::Hold(HoldStatus {
            list_generation: 1,
            reason: "long release reason ".repeat(100),
            ..Default::default()
        }),
    );
    session(vec![inventory, hold])
}

#[test]
fn split_boundary_caption_and_minimum_pages_use_actual_visible_cells() {
    let holds = long_hold_session();
    for cols in [60, 120] {
        let mut ui = UiModel {
            cols,
            screen: Screen::Holds,
            in_detail: true,
            ..ui()
        };
        let projection = project_ui(&holds, &mut ui);
        let shell = Shell::for_ui(&ui);
        assert!(shell.detail.inner.width >= 48);
        if cols == 120 {
            assert_eq!(shell.detail.inner.width, 48);
        }
        assert_eq!(shell.detail.facts(true).height, 8);
        let text = lines(&draw(&ui, holds.sync, &projection, false));
        assert!(text[usize::from(shell.detail.inner.bottom() - 1)].contains(HOLD_CAPTION));
        event_without_draw(&holds, &mut ui, KeyCode::PageDown);
        assert_eq!(ui.detail_offset, 8);
        event_without_draw(&holds, &mut ui, KeyCode::PageUp);
        assert_eq!(ui.detail_offset, 0);
    }
    let session = session(
        (0..40)
            .map(|n| want(&format!("movie:key:title.{n:02}"), &n.to_string(), n))
            .collect(),
    );
    let mut ui = ui();
    event_without_draw(&session, &mut ui, KeyCode::PageDown);
    assert_eq!(ui.selected, 8);
    event_without_draw(&session, &mut ui, KeyCode::PageUp);
    assert_eq!(ui.selected, 0);
    for help in [false, true] {
        ui.help = help;
        ui.report = (!help).then(completed_report);
        event_without_draw(&session, &mut ui, KeyCode::PageDown);
        assert_eq!(
            if help {
                ui.help_offset
            } else {
                ui.report_offset
            },
            9
        );
        event_without_draw(&session, &mut ui, KeyCode::PageUp);
        assert_eq!(
            if help {
                ui.help_offset
            } else {
                ui.report_offset
            },
            0
        );
    }
}

#[test]
fn filtered_projection_keeps_missing_worker_and_synthetic_title_details() {
    let session = session(vec![
        HomeObject::new(
            Kind::Cluster,
            "home",
            Spec::Cluster(Default::default()),
            StatusBody::Cluster(Default::default()),
        ),
        want("movie:key:beta.1999", "one", 7),
    ]);
    let missing = project_filtered(&session.cache, Screen::Overview, 0, now(), "inventory");
    assert_eq!(missing.rows.len(), 1);
    assert_eq!(missing.detail[0].value, "inventory");
    assert!(
        missing
            .detail
            .iter()
            .any(|line| line.label == "ready" && line.value.contains("missing"))
    );
    let title = project_filtered(&session.cache, Screen::Titles, 0, now(), "beta");
    assert_eq!(title.rows.len(), 1);
    assert_eq!(title.detail[0].value, "movie:key:beta.1999");
    assert_eq!(title.detail[1].label, "title");
    let mut ui = UiModel {
        screen: Screen::Titles,
        filter: "beta".into(),
        in_detail: true,
        ..ui()
    };
    project_ui(&session, &mut ui);
    let target = ui.rendered_target.as_ref().unwrap();
    assert_eq!(target.uid, "one");
    assert_eq!(target.resource_version, 7);
}
