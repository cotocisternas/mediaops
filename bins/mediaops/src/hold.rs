use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use mediaops_core::{
    ControlPort, Envelope, HoldDecision, HoldEvent, HoldKey, HoldLiveItem, JobEvent, JobKind,
    PathSchemaError, ReleaseId, TitleId, preflight_approve_placement, title_key,
};
use mediaops_proto::ControlPortClient;
use mediaops_proto::control_service_client::ControlServiceClient;
use mediaops_store::Store;
use mediaops_sync::inbox;
use mediaops_transfer::connect_home;
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;
use crate::out::{Style, Tone, finish, fmt_age, fmt_bytes, humanize_schema_label, indent, row};

#[derive(Debug, Serialize)]
struct HoldJson {
    n: usize,
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    release: String,
    title_id: String,
    release_id: String,
    age_secs: u64,
    size: u64,
    reason: String,
}

#[derive(Debug, Serialize)]
struct HoldListData {
    holds: Vec<HoldJson>,
}

#[derive(Debug, Serialize)]
struct HoldDecideData {
    name: String,
    title_id: String,
    release_id: String,
    decision: String,
}

pub async fn list(
    json: bool,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    state_db: Option<PathBuf>,
) -> Result<String, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let channel = connect_home(&socket, &tls_dir)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let control = ControlPortClient::new(ControlServiceClient::new(channel));
    let live = control.hold_list().await.map_err(map_control)?;
    let decided = store
        .list_decided()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let holds: Vec<HoldJson> = inbox(&live, &decided)
        .into_iter()
        .enumerate()
        .map(|(i, item)| hold_json(i + 1, &item, now))
        .collect();
    let data = HoldListData { holds };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format_hold_list(&data.holds))
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn decide(
    json: bool,
    decision: HoldDecision,
    title_id: String,
    release_id: Option<String>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    state_db: Option<PathBuf>,
) -> Result<String, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let channel = connect_home(&socket, &tls_dir)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let control = ControlPortClient::new(ControlServiceClient::new(channel));
    let live = control.hold_list().await.map_err(map_control)?;
    let decided = store
        .list_decided()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let listed = inbox(&live, &decided);
    let item = resolve_hold(&listed, &title_id, release_id.as_deref())?;
    let key = item.key.clone();
    let name = hold_label(item);
    if decision == HoldDecision::Approved
        && let Some(placement) = &item.placement
    {
        preflight_approve_placement(&key.title_id, placement).map_err(map_pathschema)?;
    }
    persist_decision(&store, &key, decision).await?;
    if decision == HoldDecision::Rejected {
        control.hold_reject(&key).await.map_err(map_control)?;
    }
    let data = HoldDecideData {
        name,
        title_id: key.title_id.render(),
        release_id: key.release_id.as_str().to_string(),
        decision: decision.as_str().to_string(),
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format_hold_decide(&data))
    }
}

fn format_hold_decide(data: &HoldDecideData) -> String {
    let style = Style::stdout();
    let title = humanize_schema_label(&data.name);
    let (verb, tone) = match data.decision.as_str() {
        "approved" => ("approved", Tone::Go),
        _ => ("rejected", Tone::Quiet),
    };
    finish(vec![
        row(style, verb, tone, &title, ""),
        indent(style, &data.title_id),
    ])
}

pub(crate) fn render_live_list(items: &[HoldLiveItem], json: bool) -> Result<String, AppError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs() as i64)
        .unwrap_or(0);
    let holds: Vec<_> = items
        .iter()
        .enumerate()
        .map(|(i, item)| hold_json(i + 1, item, now))
        .collect();
    if json {
        serde_json::to_string(&Envelope::ok(HoldListData { holds }))
            .map_err(|err| AppError::Runtime(err.into()))
    } else {
        Ok(format_hold_list(&holds))
    }
}

pub(crate) fn render_live_decision(
    item: &HoldLiveItem,
    decision: HoldDecision,
    json: bool,
) -> Result<String, AppError> {
    let data = HoldDecideData {
        name: hold_label(item),
        title_id: item.key.title_id.render(),
        release_id: item.key.release_id.as_str().to_string(),
        decision: decision.as_str().to_string(),
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|err| AppError::Runtime(err.into()))
    } else {
        Ok(format_hold_decide(&data))
    }
}

fn hold_json(n: usize, item: &HoldLiveItem, now: i64) -> HoldJson {
    HoldJson {
        n,
        name: hold_label(item),
        release: hold_release_name(item),
        title_id: item.key.title_id.render(),
        release_id: item.key.release_id.as_str().to_string(),
        age_secs: item.age_secs(now),
        size: item.size,
        reason: item.reason.clone(),
    }
}

fn hold_label(item: &HoldLiveItem) -> String {
    if let Some(placement) = &item.placement {
        return placement.label();
    }
    let release = hold_release_name(item);
    if !release.is_empty() {
        return release;
    }
    item.key.title_id.render()
}

fn hold_release_name(item: &HoldLiveItem) -> String {
    if let Some(path) = &item.output_path
        && let Some(name) = path.rsplit(['/', '\\']).find(|s| !s.is_empty())
    {
        return name.to_string();
    }
    if let Some(remote) = &item.remote
        && let Some(first) = remote.rel_path().iter().next()
    {
        return first.to_string_lossy().into_owned();
    }
    String::new()
}

fn format_hold_list(holds: &[HoldJson]) -> String {
    let style = Style::stdout();
    if holds.is_empty() {
        return "nothing on hold".into();
    }
    let mut lines = Vec::new();
    for h in holds {
        let title = humanize_schema_label(&h.name);
        let meta = format!("{}  {}", fmt_bytes(h.size), fmt_age(h.age_secs));
        lines.push(format!("{}.  {}  {meta}", h.n, style.bold(&title)));
        lines.push(format!("    {}", style.dim(&h.title_id)));
        if !h.reason.is_empty() {
            lines.push(format!("    {}", h.reason));
        }
        if !h.release.is_empty() && h.release != h.name {
            lines.push(format!("    {}", style.dim(&h.release)));
        }
        lines.push(String::new());
    }
    if let Some(first) = holds.first() {
        lines.push(row(
            style,
            "approve",
            Tone::Go,
            "",
            &format!("mediaops hold approve {}", first.title_id),
        ));
    }
    finish(lines)
}

fn resolve_hold<'a>(
    listed: &'a [HoldLiveItem],
    target: &str,
    release_id: Option<&str>,
) -> Result<&'a HoldLiveItem, AppError> {
    let target = target.trim();
    if target.is_empty() {
        return Err(AppError::Usage(
            "say which hold: a number from `hold list`, or a title".into(),
        ));
    }
    if let Some(rel) = release_id {
        let title_id = TitleId::parse(target).map_err(|err| AppError::Usage(err.to_string()))?;
        let release_id = ReleaseId::parse(rel).map_err(|err| AppError::Usage(err.to_string()))?;
        let key = HoldKey::new(title_id, release_id);
        return listed.iter().find(|item| item.key == key).ok_or_else(|| {
            AppError::Usage("hold is not in the inbox (unknown key or grabber=none)".into())
        });
    }
    if let Ok(n) = target.parse::<usize>()
        && n >= 1
        && n <= listed.len()
    {
        return Ok(&listed[n - 1]);
    }
    if let Ok(title_id) = TitleId::parse(target) {
        return unique_hold(
            listed
                .iter()
                .filter(|item| item.key.title_id == title_id)
                .collect(),
            target,
        );
    }
    let needle = title_key(target);
    unique_hold(
        listed
            .iter()
            .filter(|item| {
                !needle.is_empty()
                    && title_key(&format!("{} {}", hold_label(item), hold_release_name(item)))
                        .contains(&needle)
            })
            .collect(),
        target,
    )
}

fn unique_hold<'a>(
    hits: Vec<&'a HoldLiveItem>,
    target: &str,
) -> Result<&'a HoldLiveItem, AppError> {
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(AppError::Usage(format!(
            "no hold matches `{target}` — `mediaops hold list`"
        ))),
        _ => Err(AppError::Usage(format!(
            "`{target}` matches {} holds; use the number from `hold list`",
            hits.len()
        ))),
    }
}

async fn persist_decision(
    store: &Store,
    key: &HoldKey,
    decision: HoldDecision,
) -> Result<(), AppError> {
    store
        .put_hold(key, decision)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let job = store
        .create_job(JobKind::Hold, &key.title_id, None)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let event = match decision {
        HoldDecision::Approved => HoldEvent::Approve,
        HoldDecision::Rejected => HoldEvent::Reject,
    };
    store
        .advance(job.id(), JobEvent::Hold(event))
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    Ok(())
}

fn map_pathschema(err: PathSchemaError) -> AppError {
    match err {
        PathSchemaError::SpaceRefused(_) | PathSchemaError::LeftoverSceneTag(_) => {
            AppError::Policy(err.to_string())
        }
        other => AppError::Policy(other.to_string()),
    }
}

fn map_control(err: mediaops_core::ControlError) -> AppError {
    match err.exit_code {
        mediaops_core::ExitCode::PolicyRefusal => AppError::Policy(err.message),
        mediaops_core::ExitCode::DriftVerify => AppError::DriftVerify(err.message),
        mediaops_core::ExitCode::LockConflict => AppError::LockConflict(err.message),
        mediaops_core::ExitCode::Usage => AppError::Usage(err.message),
        _ => AppError::Runtime(anyhow::anyhow!("{}", err.message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{
        BoxFuture, ControlError, DesiredState, EdgeApiReport, GrabApplyReport, GrabOps, Grabber,
        HoldDecision, HoldKey, HoldLiveItem, HoldState, HoldsRepo, JobState, KeyPresence,
        Placement, ReleaseId, TitleId,
    };
    use mediaops_sync::{SCHEMA_DIRS, ensure_layout};
    use std::sync::{Arc, Mutex};

    struct FakeGrabOps {
        items: Vec<HoldLiveItem>,
        rejected: Mutex<Vec<HoldKey>>,
    }

    impl GrabOps for FakeGrabOps {
        fn grab_apply<'a>(
            &'a self,
            _: &'a DesiredState,
        ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>> {
            Box::pin(async {
                Ok(GrabApplyReport {
                    noop: true,
                    diff: String::new(),
                })
            })
        }
        fn key_discovery(&self) -> BoxFuture<'_, Result<KeyPresence, ControlError>> {
            Box::pin(async { Ok(KeyPresence::default()) })
        }
        fn edge_api_check(&self) -> BoxFuture<'_, Result<EdgeApiReport, ControlError>> {
            Box::pin(async {
                Ok(EdgeApiReport {
                    fingerprint: String::new(),
                    invariant_ok: true,
                    drift: String::new(),
                })
            })
        }
        fn edge_apply<'a>(
            &'a self,
            _: &'a DesiredState,
        ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>> {
            Box::pin(async {
                Ok(GrabApplyReport {
                    noop: true,
                    diff: String::new(),
                })
            })
        }
        fn hold_list(&self) -> BoxFuture<'_, Result<Vec<HoldLiveItem>, ControlError>> {
            let items = self.items.clone();
            Box::pin(async move { Ok(items) })
        }
        fn hold_reject<'a>(&'a self, key: &'a HoldKey) -> BoxFuture<'a, Result<(), ControlError>> {
            self.rejected.lock().expect("lock").push(key.clone());
            Box::pin(async { Ok(()) })
        }
        fn wanted_missing(&self) -> BoxFuture<'_, Result<Vec<TitleId>, ControlError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn unmonitor<'a>(&'a self, _: &'a TitleId) -> BoxFuture<'a, Result<(), ControlError>> {
            Box::pin(async { Ok(()) })
        }
        fn qbit_snapshot(
            &self,
        ) -> BoxFuture<'_, Result<Vec<mediaops_core::GuardPreviewItem>, ControlError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    fn live_item(
        title: &str,
        release: &str,
        added_unix: i64,
        size: u64,
        reason: &str,
    ) -> HoldLiveItem {
        HoldLiveItem::new(
            HoldKey::new(
                TitleId::parse(title).expect("title"),
                ReleaseId::parse(release).expect("release"),
            ),
            added_unix,
            size,
            reason,
        )
    }

    #[tokio::test]
    async fn loopback_grabber_none_lists_empty_inbox() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("hold-list-empty");
        let lb = crate::test_support::start_pair(None, b"").await;
        let json = list(
            true,
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect("list");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["holds"], serde_json::json!([]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn decided_key_drops_out_of_inbox() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("hold-list-join");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let decided = live_item(
            "movie:tmdb:603",
            "deadbeef",
            now - 180,
            111,
            "No files found are eligible for import",
        );
        let keep = live_item(
            "series:tvdb:79126",
            "cafebabe",
            now - 90,
            999,
            "Sample file only",
        );
        let lb = crate::test_support::start_pair_with_grab_ops(
            Grabber::Servarr,
            Some(Arc::new(FakeGrabOps {
                items: vec![decided.clone(), keep.clone()],
                rejected: Mutex::new(Vec::new()),
            })),
        )
        .await;
        let store = crate::test_support::open_store(&dir).await;
        store
            .put(&decided.key, HoldDecision::Rejected)
            .await
            .expect("put");
        let json = list(
            true,
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect("list");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        let holds = value["data"]["holds"].as_array().expect("holds");
        assert_eq!(holds.len(), 1, "{holds:?}");
        assert_eq!(holds[0]["title_id"], "series:tvdb:79126");
        assert_eq!(holds[0]["release_id"], "cafebabe");
        assert_eq!(holds[0]["size"], 999);
        assert_eq!(holds[0]["reason"], "Sample file only");
        let age = holds[0]["age_secs"].as_u64().expect("age");
        assert!((80..=110).contains(&age), "age_secs={age} should be ~90s");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn exclusive_flock_does_not_block_list_or_write_schema() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("hold-list-lockfree");
        let library = dir.join("library");
        ensure_layout(&library).expect("layout");
        let lb = crate::test_support::start_pair(None, b"").await;
        let lock_path = dir.join("mediaops.lock");
        let file = std::fs::File::create(&lock_path).expect("lock file");
        fs4::FileExt::try_lock(&file).expect("hold lock");
        let json = list(
            true,
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect("lock-free list");
        drop(file);
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["holds"], serde_json::json!([]));
        for name in SCHEMA_DIRS {
            let dir = library.join(name);
            let empty = std::fs::read_dir(&dir).expect("read").next().is_none();
            assert!(empty, "{name} must stay empty (not a hold folder)");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    fn movie_hold(placement: Option<Placement>) -> HoldLiveItem {
        let mut item = live_item(
            "movie:tmdb:603",
            "deadbeef",
            1_577_836_800,
            111,
            "No files found are eligible for import",
        );
        item.placement = placement;
        item
    }

    #[tokio::test]
    async fn approve_json_persists_decision_and_job_without_schema_writes() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("hold-approve");
        let library = dir.join("library");
        ensure_layout(&library).expect("layout");
        let item = movie_hold(Some(Placement::movie("The.Matrix", 1999, "mkv")));
        let ops = Arc::new(FakeGrabOps {
            items: vec![item.clone()],
            rejected: Mutex::new(Vec::new()),
        });
        let lb = crate::test_support::start_pair_with_grab_ops(Grabber::Servarr, Some(ops.clone()))
            .await;
        let lock_path = dir.join("mediaops.lock");
        let file = std::fs::File::create(&lock_path).expect("lock file");
        fs4::FileExt::try_lock(&file).expect("hold lock");
        let json = decide(
            true,
            HoldDecision::Approved,
            "movie:tmdb:603".into(),
            Some("deadbeef".into()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect("approve");
        drop(file);
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["title_id"], "movie:tmdb:603");
        assert_eq!(value["data"]["release_id"], "deadbeef");
        assert_eq!(value["data"]["decision"], "approved");
        let store = crate::test_support::open_store(&dir).await;
        assert_eq!(
            store.get(&item.key).await.expect("get"),
            Some(HoldDecision::Approved)
        );
        let jobs = store
            .list_jobs_by_title(&item.key.title_id)
            .await
            .expect("jobs");
        assert!(
            jobs.iter()
                .any(|j| matches!(j.state(), JobState::Hold(HoldState::Approved))),
            "{jobs:?}"
        );
        assert!(
            ops.rejected.lock().expect("lock").is_empty(),
            "approve must not call hold_reject"
        );
        let listed = list(
            true,
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect("list");
        let listed: serde_json::Value = serde_json::from_str(&listed).expect("json");
        assert_eq!(listed["data"]["holds"], serde_json::json!([]));
        for name in SCHEMA_DIRS {
            let empty = std::fs::read_dir(library.join(name))
                .expect("read")
                .next()
                .is_none();
            assert!(empty, "{name} must not be written by hold approve");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn approve_leftover_scene_tag_is_policy_and_writes_no_row() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("hold-approve-tag");
        let item = movie_hold(Some(Placement::movie("The.Matrix.REPACK", 1999, "mkv")));
        let lb = crate::test_support::start_pair_with_grab_ops(
            Grabber::Servarr,
            Some(Arc::new(FakeGrabOps {
                items: vec![item.clone()],
                rejected: Mutex::new(Vec::new()),
            })),
        )
        .await;
        let err = decide(
            true,
            HoldDecision::Approved,
            "movie:tmdb:603".into(),
            Some("deadbeef".into()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect_err("leftover");
        assert!(matches!(err, crate::AppError::Policy(_)), "{err}");
        let store = crate::test_support::open_store(&dir).await;
        assert!(store.get(&item.key).await.expect("get").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn approve_spaces_are_policy_and_write_no_row() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("hold-approve-space");
        let item = movie_hold(Some(Placement::movie("The Matrix", 1999, "mkv")));
        let lb = crate::test_support::start_pair_with_grab_ops(
            Grabber::Servarr,
            Some(Arc::new(FakeGrabOps {
                items: vec![item.clone()],
                rejected: Mutex::new(Vec::new()),
            })),
        )
        .await;
        let err = decide(
            true,
            HoldDecision::Approved,
            "movie:tmdb:603".into(),
            Some("deadbeef".into()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect_err("spaces");
        assert!(matches!(err, crate::AppError::Policy(_)), "{err}");
        let store = crate::test_support::open_store(&dir).await;
        assert!(store.get(&item.key).await.expect("get").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn reject_json_persists_and_calls_hold_reject() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("hold-reject");
        let item = movie_hold(Some(Placement::movie("The.Matrix", 1999, "mkv")));
        let ops = Arc::new(FakeGrabOps {
            items: vec![item.clone()],
            rejected: Mutex::new(Vec::new()),
        });
        let lb = crate::test_support::start_pair_with_grab_ops(Grabber::Servarr, Some(ops.clone()))
            .await;
        let lock_path = dir.join("mediaops.lock");
        let file = std::fs::File::create(&lock_path).expect("lock file");
        fs4::FileExt::try_lock(&file).expect("hold lock");
        let json = decide(
            true,
            HoldDecision::Rejected,
            "movie:tmdb:603".into(),
            Some("deadbeef".into()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect("reject");
        drop(file);
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["decision"], "rejected");
        assert_eq!(*ops.rejected.lock().expect("lock"), vec![item.key.clone()]);
        let store = crate::test_support::open_store(&dir).await;
        assert_eq!(
            store.get(&item.key).await.expect("get"),
            Some(HoldDecision::Rejected)
        );
        let jobs = store
            .list_jobs_by_title(&item.key.title_id)
            .await
            .expect("jobs");
        assert!(
            jobs.iter()
                .any(|j| matches!(j.state(), JobState::Hold(HoldState::Rejected))),
            "{jobs:?}"
        );
        let listed = list(
            true,
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect("list");
        let listed: serde_json::Value = serde_json::from_str(&listed).expect("json");
        assert_eq!(listed["data"]["holds"], serde_json::json!([]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn unknown_key_and_grabber_none_are_usage_with_no_row() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("hold-usage");
        let lb = crate::test_support::start_pair(None, b"").await;
        for decision in [HoldDecision::Approved, HoldDecision::Rejected] {
            let err = decide(
                true,
                decision,
                "movie:tmdb:603".into(),
                Some("deadbeef".into()),
                Some(lb.sock.clone()),
                Some(lb.tls_dir.clone()),
                Some(dir.clone()),
                Some(dir.join("state.db")),
            )
            .await
            .expect_err("usage");
            assert!(matches!(err, crate::AppError::Usage(_)), "{err}");
        }
        let store = crate::test_support::open_store(&dir).await;
        let key = HoldKey::new(
            TitleId::movie_key("The.Matrix", 1999).expect("title"),
            ReleaseId::parse("deadbeef").expect("id"),
        );
        assert!(store.get(&key).await.expect("get").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hold_list_text_is_a_name_and_a_number() {
        let text = format_hold_list(&[HoldJson {
            n: 1,
            name: "Hearts.of.Darkness.A.Filmmaker's.Apocalypse.(1991)".into(),
            release: "Hearts.of.Darkness-Reise.ins.Herz.der.Finsternis.GERMAN.DL".into(),
            title_id: "movie:tmdb:4539".into(),
            release_id: "deadbeef".into(),
            age_secs: 748,
            size: 7_588_856_506,
            reason: "Manual Import required.".into(),
        }]);
        assert_eq!(
            text,
            "\
1.  Hearts of Darkness A Filmmaker's Apocalypse (1991)  7.1 GiB  12m
    movie:tmdb:4539
    Manual Import required.
    Hearts.of.Darkness-Reise.ins.Herz.der.Finsternis.GERMAN.DL

approve   mediaops hold approve movie:tmdb:4539"
        );
        assert!(!text.contains("deadbeef"));
    }

    #[test]
    fn hold_list_empty_is_english() {
        assert_eq!(format_hold_list(&[]), "nothing on hold");
    }

    #[test]
    fn hold_decide_text_is_a_title_and_an_id() {
        assert_eq!(
            format_hold_decide(&HoldDecideData {
                name: "Hearts.of.Darkness.A.Filmmaker's.Apocalypse.(1991)".into(),
                title_id: "movie:tmdb:4539".into(),
                release_id: "deadbeef".into(),
                decision: "approved".into(),
            }),
            "\
approved  Hearts of Darkness A Filmmaker's Apocalypse (1991)
          movie:tmdb:4539"
        );
    }

    #[test]
    fn resolve_hold_accepts_number_and_unique_name() {
        let mut item = live_item("movie:tmdb:4539", "deadbeef", 0, 1, "blocked");
        item.placement = Some(Placement::movie(
            "Hearts.of.Darkness.A.Filmmaker's.Apocalypse",
            1991,
            "mkv",
        ));
        let listed = [item];
        assert_eq!(
            resolve_hold(&listed, "1", None)
                .expect("n")
                .key
                .title_id
                .render(),
            "movie:tmdb:4539"
        );
        assert_eq!(
            resolve_hold(&listed, "hearts of darkness", None)
                .expect("name")
                .key
                .release_id
                .as_str(),
            "deadbeef"
        );
        assert!(resolve_hold(&listed, "Silo", None).is_err());
    }

    #[test]
    fn no_auto_approve_agent_approve_or_confidence_floor() {
        let apply = include_str!("../../../crates/sync/src/apply.rs");
        let plan = include_str!("../../../crates/sync/src/plan.rs");
        let controllers = include_str!("../../../crates/api/src/controllers.rs");
        let needles = [
            concat!("auto", "-approve"),
            concat!("auto", "_approve"),
            concat!("agent", "-approve"),
            concat!("agent", "_approve"),
            concat!("confidence", "_floor"),
            concat!("confidence", " floor"),
        ];
        for hay in [apply, plan, controllers] {
            let lower = hay.to_ascii_lowercase();
            for needle in needles {
                assert!(
                    !lower.contains(needle),
                    "agent auto-approve / confidence floor must not exist: {needle}"
                );
            }
        }
    }
}
