//! Home API client verbs: get / apply / delete / watch-objects / reconcile / import-legacy.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;

use mediaops_core::{
    Actor, CLUSTER_NAME, DesiredState, HoldDecisionSpec, HoldSpec, HomeObject, Kind, SECRET_NAME,
    Spec, StatusBody, TitleId, TitleSpec, WantSpec,
};
use mediaops_home_client::{ClientError, HomeApi, default_api_socket};
use mediaops_store::Store;
use serde::Serialize;
use unicode_width::UnicodeWidthStr;

use crate::AppError;
use crate::bootstrap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    Table,
    Wide,
    Json,
    LegacyJson,
}

impl Output {
    pub fn parse(raw: Option<&str>, json_flag: bool) -> Result<Self, AppError> {
        if json_flag {
            if raw.is_some() {
                return Err(AppError::Usage("use either --json or -o, not both".into()));
            }
            return Ok(Self::LegacyJson);
        }
        match raw.unwrap_or("table") {
            "json" => Ok(Self::Json),
            "wide" => Ok(Self::Wide),
            "table" | "" => Ok(Self::Table),
            other => Err(AppError::Usage(format!("unknown -o `{other}`"))),
        }
    }

    fn is_json(self) -> bool {
        matches!(self, Self::Json | Self::LegacyJson)
    }
}

fn render_payload<T: Serialize>(
    payload: &T,
    tsv: String,
    output: Output,
) -> Result<String, AppError> {
    match output {
        Output::Json => serde_json::to_string(payload).map_err(|err| AppError::Runtime(err.into())),
        Output::LegacyJson => serde_json::to_string(&mediaops_core::Envelope::ok(payload))
            .map_err(|err| AppError::Runtime(err.into())),
        Output::Table | Output::Wide => Ok(tsv),
    }
}

pub async fn connect(socket: Option<PathBuf>, actor: Actor) -> Result<HomeApi, AppError> {
    let socket = socket.unwrap_or_else(default_api_socket);
    HomeApi::connect(socket, actor).await.map_err(map_client)
}

pub async fn get(
    kind: String,
    name: String,
    output: Output,
    socket: Option<PathBuf>,
) -> Result<String, AppError> {
    let kind = Kind::parse(&kind).map_err(|e| AppError::Usage(e.to_string()))?;
    let api = connect(socket, Actor::Cli).await?;
    let obj = api.get(kind, &name).await.map_err(map_client)?;
    Ok(render_one(&obj, output))
}

pub async fn apply_file(
    path: PathBuf,
    output: Output,
    socket: Option<PathBuf>,
) -> Result<String, AppError> {
    let raw = std::fs::read(&path).map_err(|e| AppError::Runtime(e.into()))?;
    let obj = HomeObject::from_bytes(&raw).map_err(|e| AppError::Usage(e.to_string()))?;
    obj.validate().map_err(|e| AppError::Usage(e.to_string()))?;
    let api = connect(socket, Actor::Cli).await?;
    let written = api.apply(obj).await.map_err(map_client)?;
    Ok(render_one(&written, output))
}

pub async fn delete(
    kind: String,
    name: String,
    output: Output,
    socket: Option<PathBuf>,
) -> Result<String, AppError> {
    let kind = Kind::parse(&kind).map_err(|e| AppError::Usage(e.to_string()))?;
    let api = connect(socket, Actor::Cli).await?;
    let obj = api.delete(kind, &name).await.map_err(map_client)?;
    Ok(render_one(&obj, output))
}

pub async fn watch_kind(
    kind: Option<String>,
    name: Option<String>,
    output: Output,
    socket: Option<PathBuf>,
) -> Result<(), AppError> {
    if output == Output::LegacyJson {
        return Err(AppError::Usage(
            "streaming output uses -o json (one object per line)".into(),
        ));
    }
    let kind = match kind.as_deref() {
        None | Some("") => None,
        Some(raw) => Some(Kind::parse(raw).map_err(|e| AppError::Usage(e.to_string()))?),
    };
    let api = connect(socket, Actor::Cli).await?;
    let mut stream = api.watch(kind, 0).await.map_err(map_client)?;
    use tokio_stream::StreamExt;
    while let Some(ev) = stream.next().await {
        let ev = ev.map_err(|e| AppError::Runtime(anyhow::anyhow!(e.to_string())))?;
        let Some(obj) = ev.object else {
            continue;
        };
        let obj = mediaops_proto::home_object_from_wire(obj)
            .map_err(|e| AppError::Runtime(anyhow::anyhow!(e.to_string())))?;
        if name.as_ref().is_some_and(|name| obj.metadata.name != *name) {
            continue;
        }
        let line = if output.is_json() {
            render_one(&obj, output)
        } else {
            format!(
                "{}\t{}\t{}",
                watch_type(ev.r#type),
                obj.kind.as_str(),
                obj.metadata.name
            )
        };
        // Flush each event. A quiet watch must not wait for an arbitrary batch
        // size, and a long-running watch must not stop after that batch.
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{line}").map_err(|err| AppError::Runtime(err.into()))?;
        stdout
            .flush()
            .map_err(|err| AppError::Runtime(err.into()))?;
    }
    Ok(())
}

pub async fn reconcile(output: Output, socket: Option<PathBuf>) -> Result<String, AppError> {
    let api = connect(socket, Actor::Cli).await?;
    let generation = api.reconcile().await.map_err(map_client)?;
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ReconcileOut {
        reconcile_generation: i64,
    }
    render_payload(
        &ReconcileOut {
            reconcile_generation: generation,
        },
        format!("reconcileGeneration\t{generation}"),
        output,
    )
}

pub async fn list_kind(
    kind: Option<String>,
    output: Output,
    socket: Option<PathBuf>,
) -> Result<String, AppError> {
    let kind = match kind.as_deref() {
        None | Some("") => None,
        Some(raw) => Some(Kind::parse(raw).map_err(|e| AppError::Usage(e.to_string()))?),
    };
    let api = connect(socket, Actor::Cli).await?;
    let items = api.list(kind).await.map_err(map_client)?;
    Ok(render_list(&items, output))
}

/// Create a Want (and a desired Title) for an already-known name or TitleId.
pub async fn watch_title(
    title: String,
    output: Output,
    socket: Option<PathBuf>,
) -> Result<String, AppError> {
    let api = connect(socket, Actor::Cli).await?;
    let title_id = resolve_title(&api, &title).await?;
    let want = HomeObject::new(
        Kind::Want,
        title_id.clone(),
        Spec::Want(WantSpec {
            title_id: title_id.clone(),
        }),
        StatusBody::Want(mediaops_core::WantStatus::default()),
    );
    let (written, already) = match api.get(Kind::Want, &title_id).await {
        Ok(existing) => (existing, true),
        Err(err) if err.is_not_found() => match api.apply(want).await {
            Ok(written) => (written, false),
            Err(err) if err.is_conflict() => (
                api.get(Kind::Want, &title_id).await.map_err(map_client)?,
                true,
            ),
            Err(err) => return Err(map_client(err)),
        },
        Err(err) => return Err(map_client(err)),
    };
    let title_obj = HomeObject::new(
        Kind::Title,
        title_id.clone(),
        Spec::Title(TitleSpec {
            title_id: title_id.clone(),
            desired_present: true,
        }),
        StatusBody::Title(mediaops_core::TitleStatus::default()),
    );
    match api.get(Kind::Title, &title_id).await {
        Ok(mut existing) => {
            if existing.spec != title_obj.spec {
                existing.spec = title_obj.spec;
                api.patch(existing, "spec").await.map_err(map_client)?;
            }
        }
        Err(err) if err.is_not_found() => {
            if let Err(err) = api.apply(title_obj).await
                && !err.is_conflict()
            {
                return Err(map_client(err));
            }
        }
        Err(err) => return Err(map_client(err)),
    }
    if output.is_json() {
        return Ok(render_one(&written, output));
    }
    let label = TitleId::parse(&title_id)
        .map(|id| crate::out::human_title_id(&id))
        .unwrap_or_else(|_| title_id.clone());
    let meta = if already { "already" } else { "" };
    Ok(crate::watch::format_watch_line(&label, &title_id, meta))
}

pub async fn status_pretty(output: Output, socket: Option<PathBuf>) -> Result<String, AppError> {
    let api = connect(socket, Actor::Cli).await?;
    let mut items = api.list(Some(Kind::Want)).await.map_err(map_client)?;
    items.extend(api.list(Some(Kind::Job)).await.map_err(map_client)?);
    items.extend(api.list(Some(Kind::Node)).await.map_err(map_client)?);
    if output.is_json() {
        return Ok(render_list(&items, output));
    }
    let free = match api.get(Kind::Cluster, CLUSTER_NAME).await {
        Ok(cluster) => match cluster.spec {
            Spec::Cluster(cs) if !cs.library_root.is_empty() => {
                mediaops_core::free_bytes(std::path::Path::new(&cs.library_root)).ok()
            }
            _ => None,
        },
        Err(err) if err.is_not_found() => None,
        Err(err) => return Err(map_client(err)),
    };
    Ok(format_status(&items, free))
}

fn format_status(items: &[HomeObject], free: Option<u64>) -> String {
    let mut lines = Vec::new();
    for obj in items {
        match (&obj.spec, &obj.status) {
            (Spec::Want(s), StatusBody::Want(st)) if st.phase == mediaops_core::WantPhase::Open => {
                lines.push(format!("want      {}", human_title(&s.title_id)));
            }
            (Spec::Job(s), StatusBody::Job(st))
                if st.phase != mediaops_core::JobPhase::Installed =>
            {
                lines.push(format!(
                    "pull      {}  {}",
                    human_title(&s.title_id),
                    st.phase.as_str()
                ));
                if !st.message.is_empty() {
                    lines.push(format!("          {}", st.message));
                }
            }
            _ => {}
        }
    }
    if lines.is_empty() {
        lines.push("nothing happening".into());
    }
    if let Some(free) = free {
        lines.push(String::new());
        lines.push(format!("disk      {} free", crate::out::fmt_bytes(free)));
    }
    lines.join("\n")
}

pub async fn why_pretty(
    title: String,
    output: Output,
    socket: Option<PathBuf>,
) -> Result<String, AppError> {
    let api = connect(socket, Actor::Cli).await?;
    let title_id = resolve_title(&api, &title).await?;
    let mut related = Vec::new();
    for kind in [
        Kind::Title,
        Kind::Want,
        Kind::Job,
        Kind::Hold,
        Kind::RemoteFile,
    ] {
        for obj in api.list(Some(kind)).await.map_err(map_client)? {
            if title_id_of(&obj).as_deref() == Some(title_id.as_str())
                || obj.metadata.name == title_id
            {
                related.push(obj);
            }
        }
    }
    if output.is_json() {
        return Ok(render_list(&related, output));
    }
    Ok(format_why(&title_id, &related))
}

fn format_why(title_id: &str, related: &[HomeObject]) -> String {
    let label = TitleId::parse(title_id)
        .map(|id| crate::out::human_title_id(&id))
        .unwrap_or_else(|_| title_id.to_string());
    let mut lines = vec![label, title_id.to_string(), String::new()];
    let mut facts = 0u32;
    let on_box = related.iter().any(|obj| obj.kind == Kind::RemoteFile);
    for obj in related {
        match (&obj.spec, &obj.status) {
            (Spec::Hold(s), StatusBody::Hold(st))
                if s.decision == HoldDecisionSpec::Empty && !st.reason.is_empty() =>
            {
                lines.push(format!(
                    "hold      {}  {}",
                    st.reason,
                    crate::out::fmt_bytes(st.size)
                ));
                facts += 1;
            }
            (Spec::Want(_), StatusBody::Want(st)) if st.phase == mediaops_core::WantPhase::Open => {
                lines.push(
                    if on_box {
                        "want      open, listed on the box"
                    } else {
                        "grab      wanted, not on the box"
                    }
                    .into(),
                );
                facts += 1;
            }
            (Spec::Job(s), StatusBody::Job(st)) => {
                lines.push(format!(
                    "pull      {}  {}",
                    human_title(&s.title_id),
                    st.phase.as_str()
                ));
                if !st.message.is_empty() {
                    lines.push(format!("          {}", st.message));
                }
                facts += 1;
            }
            (Spec::Title(_), StatusBody::Title(st)) if st.drifted => {
                lines.push("library   drifted".into());
                facts += 1;
            }
            (Spec::Title(_), StatusBody::Title(st)) => {
                for file in st.observed_files() {
                    lines.push(format!(
                        "library   {}{}",
                        file.path,
                        if file.drifted { "  drifted" } else { "" }
                    ));
                    facts += 1;
                }
            }
            _ => {}
        }
    }
    if facts == 0 {
        lines.push("quiet".into());
    }
    lines.join("\n")
}

pub async fn hold_list(output: Output, socket: Option<PathBuf>) -> Result<String, AppError> {
    let api = connect(socket, Actor::Cli).await?;
    let items = published_holds(&api).await?;
    let open: Vec<_> = items
        .into_iter()
        .filter(|o| match &o.spec {
            Spec::Hold(s) => s.decision == HoldDecisionSpec::Empty,
            _ => false,
        })
        .collect();
    if output == Output::Json || output == Output::Wide {
        return Ok(render_list(&open, output));
    }
    let live = open
        .iter()
        .map(hold_live_item)
        .collect::<Result<Vec<_>, _>>()?;
    crate::hold::render_live_list(&live, output == Output::LegacyJson)
}

pub async fn hold_decide(
    title: String,
    release_id: Option<String>,
    decision: HoldDecisionSpec,
    output: Output,
    socket: Option<PathBuf>,
) -> Result<String, AppError> {
    let api = connect(socket, Actor::Cli).await?;
    let holds = published_holds(&api).await?;
    let numbered = title.parse::<usize>().ok().and_then(|n| n.checked_sub(1))
        .and_then(|index| holds.iter().filter(|hold| matches!(&hold.spec, Spec::Hold(s) if s.decision == HoldDecisionSpec::Empty)).nth(index))
        .map(|hold| hold.metadata.name.clone());
    let resolved = if numbered.is_some()
        || TitleId::parse(&title).is_ok()
        || holds.iter().any(|hold| hold.metadata.name == title)
    {
        title.clone()
    } else {
        resolve_from_objects(&holds, &title)?
    };
    let matches: Vec<_> = holds
        .into_iter()
        .filter(|o| match &o.spec {
            Spec::Hold(s) => {
                (s.title_id == resolved
                    || o.metadata.name == resolved
                    || numbered.as_ref() == Some(&o.metadata.name))
                    && release_id.as_deref().is_none_or(|rid| s.release_id == rid)
            }
            _ => false,
        })
        .collect();
    let mut hold = match matches.len() {
        1 => matches.into_iter().next().expect("one"),
        0 => {
            return Err(AppError::Usage(format!(
                "hold `{title}` is not in the inbox"
            )));
        }
        _ => {
            return Err(AppError::Usage(format!(
                "hold `{title}` is ambiguous; pass a release id"
            )));
        }
    };
    if let Spec::Hold(spec) = &mut hold.spec {
        spec.decision = decision;
    }
    let written = api.patch(hold, "spec").await.map_err(map_client)?;
    if output == Output::Json || output == Output::Wide {
        return Ok(render_one(&written, output));
    }
    crate::hold::render_live_decision(
        &hold_live_item(&written)?,
        match decision {
            HoldDecisionSpec::Approved => mediaops_core::HoldDecision::Approved,
            HoldDecisionSpec::Rejected => mediaops_core::HoldDecision::Rejected,
            HoldDecisionSpec::Empty => {
                return Err(AppError::Usage("hold decision required".into()));
            }
        },
        output == Output::LegacyJson,
    )
}

async fn published_holds(api: &HomeApi) -> Result<Vec<HomeObject>, AppError> {
    // Node and Hold observations must come from one snapshot, including for
    // the numbered inbox. Archived rows remain available through `get Hold`.
    let objects = api.list(None).await.map_err(map_client)?;
    let generation = objects
        .iter()
        .find_map(|obj| match (&obj.spec, &obj.status) {
            (Spec::Node(spec), StatusBody::Node(status))
                if spec.worker_kind == mediaops_core::WorkerKind::Inventory
                    && status.list_generation > 0
                    && mediaops_core::node_is_ready(
                        status.ready,
                        status.last_heartbeat_unix,
                        unix_now(),
                    )
                    && mediaops_core::node_is_ready(
                        true,
                        status.list_completed_unix,
                        unix_now(),
                    ) =>
            {
                Some(status.list_generation)
            }
            _ => None,
        })
        .ok_or_else(|| {
            AppError::Runtime(anyhow::anyhow!(
                "hold inbox unavailable: wait for a fresh completed inventory listing"
            ))
        })?;
    Ok(objects
        .into_iter()
        .filter(|obj| {
            matches!(&obj.status,
        StatusBody::Hold(status) if status.list_generation == generation)
        })
        .collect())
}

fn hold_live_item(obj: &HomeObject) -> Result<mediaops_core::HoldLiveItem, AppError> {
    let (Spec::Hold(spec), StatusBody::Hold(status)) = (&obj.spec, &obj.status) else {
        return Err(AppError::Usage("expected a Hold".into()));
    };
    let key = mediaops_core::HoldKey::new(
        TitleId::parse(&spec.title_id).map_err(|err| AppError::Usage(err.to_string()))?,
        mediaops_core::ReleaseId::parse(&spec.release_id)
            .map_err(|err| AppError::Usage(err.to_string()))?,
    );
    Ok(mediaops_core::HoldLiveItem {
        key,
        added_unix: status.added_unix,
        size: status.size,
        reason: status.reason.clone(),
        remote: None,
        placement: status.placement.clone(),
        output_path: (!status.release.is_empty()).then(|| status.release.clone()),
    })
}

pub async fn doctor_nodes(socket: Option<PathBuf>) -> Result<(), AppError> {
    let api = connect(socket, Actor::Cli).await?;
    let nodes = api.list(Some(Kind::Node)).await.map_err(map_client)?;
    let now = unix_now();
    let missing: Vec<_> = ["scheduler", "inventory", "pull"]
        .into_iter()
        .filter(|name| {
            !nodes.iter().any(|node| {
                node.metadata.name == *name
                    && matches!(&node.status, StatusBody::Node(st)
                if mediaops_core::node_is_ready(st.ready, st.last_heartbeat_unix, now))
            })
        })
        .collect();
    if !missing.is_empty() {
        return Err(AppError::Runtime(anyhow::anyhow!(
            "home workers not ready: {}",
            missing.join(", ")
        )));
    }
    Ok(())
}

pub async fn import_legacy(
    config: Option<PathBuf>,
    state_db: Option<PathBuf>,
    output: Output,
    socket: Option<PathBuf>,
) -> Result<String, AppError> {
    let config_dir = bootstrap::default_config_dir();
    let config = config.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let maintenance_db = if socket.is_none() {
        bootstrap::default_state_db()
    } else {
        state_db.clone()
    };
    let api = connect(socket, Actor::Import).await?;
    let _lock = bootstrap::exclusive_lock(&bootstrap::lock_path(&maintenance_db)).map_err(
        |err| match err.exit_code() {
            mediaops_core::ExitCode::LockConflict => AppError::LockConflict(err.to_string()),
            _ => AppError::Runtime(anyhow::anyhow!(err.to_string())),
        },
    )?;
    let source_lock = bootstrap::lock_path(&state_db);
    let maintenance_lock = bootstrap::lock_path(&maintenance_db);
    let _source_lock = if source_lock != maintenance_lock
        && source_lock.canonicalize().ok() != maintenance_lock.canonicalize().ok()
    {
        Some(
            bootstrap::exclusive_lock(&source_lock).map_err(|err| match err.exit_code() {
                mediaops_core::ExitCode::LockConflict => AppError::LockConflict(err.to_string()),
                _ => AppError::Runtime(anyhow::anyhow!(err.to_string())),
            })?,
        )
    } else {
        None
    };
    let store = if state_db.is_file() {
        Some(
            Store::open(&state_db)
                .await
                .map_err(crate::api_legacy::error)?,
        )
    } else {
        None
    };
    let root = match &store {
        Some(store) => store
            .get_machine("library_root")
            .await
            .map_err(crate::api_legacy::error)?
            .unwrap_or_default(),
        None => String::new(),
    };
    let mut objects = Vec::new();

    if config.is_file() {
        let raw = std::fs::read(&config).map_err(|e| AppError::Runtime(e.into()))?;
        let ds = DesiredState::from_toml_bytes(&raw).map_err(|e| AppError::Usage(e.to_string()))?;
        let mut cluster = cluster_from_desired(&ds);
        if let Spec::Cluster(spec) = &mut cluster.spec {
            spec.library_root = root.clone();
            if let Some(store) = &store {
                spec.encode_pause = store
                    .get_machine("encode_pause")
                    .await
                    .map_err(crate::api_legacy::error)?
                    .is_some_and(|v| v == "1" || v == "true");
            }
        }
        objects.push(cluster);
        if let Some(addr) = ds.seedbox_address() {
            let secret = HomeObject::new(
                Kind::Secret,
                SECRET_NAME,
                Spec::Secret(mediaops_core::SecretSpec {
                    seedbox_address: addr.to_string(),
                    ca_sha256: ds
                        .tls()
                        .map(|tls| tls.ca_sha256.clone())
                        .unwrap_or_default(),
                    server_sha256: ds
                        .tls()
                        .map(|tls| tls.server_sha256.clone())
                        .unwrap_or_default(),
                    client_sha256: ds
                        .tls()
                        .map(|tls| tls.client_sha256.clone())
                        .unwrap_or_default(),
                }),
                StatusBody::Secret,
            );
            objects.push(secret);
        }
    }

    if let Some(store) = &store {
        let mut titles: BTreeMap<String, Vec<mediaops_core::TitleFileStatus>> = BTreeMap::new();
        for row in store
            .list_titles()
            .await
            .map_err(|e| AppError::Runtime(anyhow::anyhow!(e.to_string())))?
        {
            let path = std::path::Path::new(row.path());
            let path = if path.is_absolute() {
                path.strip_prefix(&root).map_err(|_| {
                    AppError::Usage(format!(
                        "legacy title is outside library root: {}",
                        path.display()
                    ))
                })?
            } else {
                path
            };
            mediaops_core::parse_placement(path).map_err(|err| AppError::Usage(err.to_string()))?;
            let drifted = match std::fs::File::open(std::path::Path::new(&root).join(path))
                .and_then(mediaops_core::Blake3Hex::of_reader)
            {
                Ok(digest) => &digest != row.current_b3(),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
                Err(err) => return Err(AppError::Runtime(err.into())),
            };
            titles.entry(row.title_id().render()).or_default().push(
                mediaops_core::TitleFileStatus {
                    path: path.to_string_lossy().into_owned(),
                    install_b3: row.install_b3().clone(),
                    current_b3: row.current_b3().clone(),
                    drifted,
                },
            );
        }
        for (title_id, files) in titles {
            let obj = HomeObject::new(
                Kind::Title,
                title_id.clone(),
                Spec::Title(TitleSpec {
                    title_id: title_id.clone(),
                    desired_present: true,
                }),
                StatusBody::Title(mediaops_core::TitleStatus {
                    drifted: files.iter().any(|file| file.drifted),
                    files,
                    ..mediaops_core::TitleStatus::default()
                }),
            );
            objects.push(obj);
        }
        for job in store
            .list_jobs()
            .await
            .map_err(|e| AppError::Runtime(anyhow::anyhow!(e.to_string())))?
        {
            if !matches!(
                job.state(),
                mediaops_core::JobState::Want(mediaops_core::WantState::Open)
            ) {
                continue;
            }
            let title_id = job.title_id().render();
            let obj = HomeObject::new(
                Kind::Want,
                title_id.clone(),
                Spec::Want(WantSpec {
                    title_id: title_id.clone(),
                }),
                StatusBody::Want(mediaops_core::WantStatus {
                    phase: mediaops_core::WantPhase::Open,
                }),
            );
            objects.push(obj);
        }
        for key in store
            .list_decided()
            .await
            .map_err(|e| AppError::Runtime(anyhow::anyhow!(e.to_string())))?
        {
            let name = format!("{}-{}", key.title_id.render(), key.release_id);
            let decision = store
                .get_hold(&key)
                .await
                .map_err(|e| AppError::Runtime(anyhow::anyhow!(e.to_string())))?;
            let decision = match decision {
                Some(mediaops_core::HoldDecision::Approved) => HoldDecisionSpec::Approved,
                Some(mediaops_core::HoldDecision::Rejected) => HoldDecisionSpec::Rejected,
                None => HoldDecisionSpec::Empty,
            };
            let obj = HomeObject::new(
                Kind::Hold,
                name,
                Spec::Hold(HoldSpec {
                    title_id: key.title_id.render(),
                    release_id: key.release_id.to_string(),
                    decision,
                }),
                StatusBody::Hold(mediaops_core::HoldStatus::default()),
            );
            objects.push(obj);
        }
    }

    if objects.is_empty() {
        return Err(AppError::Usage(
            "no legacy config or state to import".into(),
        ));
    }
    // Decode and validate the complete input before the first API mutation.
    // Repeating the import only fills missing objects; current runtime settings
    // and decisions always win over the old snapshot.
    for obj in &objects {
        obj.validate()
            .map_err(|err| AppError::Usage(err.to_string()))?;
    }
    let mut maintenance = match api.get(Kind::Cluster, CLUSTER_NAME).await {
        Ok(cluster) => Some(crate::api_legacy::HomeLibrary {
            api: api.clone(),
            cluster,
        }),
        Err(err) if err.is_not_found() => None,
        Err(err) => return Err(map_client(err)),
    };
    let previous_lock = match &mut maintenance {
        Some(home) => Some(home.begin_maintenance().await?),
        None => None,
    };
    let result = async {
        let mut applied: u64 = 0;
        let mut unlock = None;
        for mut obj in objects {
            match api.get(obj.kind, &obj.metadata.name).await {
                Ok(mut existing) => {
                    if let (StatusBody::Title(previous), StatusBody::Title(incoming)) =
                        (&existing.status, &obj.status)
                    {
                        let mut files = previous.observed_files();
                        let mut changed = false;
                        for file in incoming.observed_files() {
                            let (_, placement) =
                                mediaops_core::parse_placement(std::path::Path::new(&file.path))
                                    .map_err(|err| AppError::Usage(err.to_string()))?;
                            let present = files.iter().any(|current| {
                                mediaops_core::parse_placement(std::path::Path::new(&current.path))
                                    .is_ok_and(|(_, current)| {
                                        current.file_key() == placement.file_key()
                                    })
                            });
                            if !present {
                                files.push(file);
                                changed = true;
                            }
                        }
                        if changed {
                            existing.status = StatusBody::Title(mediaops_core::TitleStatus {
                                drifted: files.iter().any(|file| file.drifted),
                                files,
                                ..mediaops_core::TitleStatus::default()
                            });
                            api.patch(existing, "status").await.map_err(map_client)?;
                            applied += 1;
                        }
                    } else if let (Spec::Hold(previous), Spec::Hold(incoming)) =
                        (&mut existing.spec, &obj.spec)
                        && previous.decision == HoldDecisionSpec::Empty
                        && incoming.decision != HoldDecisionSpec::Empty
                    {
                        previous.decision = incoming.decision;
                        api.patch(existing, "spec").await.map_err(map_client)?;
                        applied += 1;
                    }
                    continue;
                }
                Err(err) if err.is_not_found() => {}
                Err(err) => return Err(map_client(err)),
            }
            if let Spec::Cluster(spec) = &mut obj.spec
                && !spec.lock
            {
                spec.lock = true;
                unlock = Some(false);
            }
            api.apply(obj).await.map_err(map_client)?;
            applied += 1;
        }
        if unlock.is_some() {
            let mut cluster = api
                .get(Kind::Cluster, CLUSTER_NAME)
                .await
                .map_err(map_client)?;
            if let Spec::Cluster(spec) = &mut cluster.spec {
                spec.lock = false;
            }
            api.patch(cluster, "spec").await.map_err(map_client)?;
        }
        Ok::<_, AppError>(applied)
    }
    .await;
    let applied = result.map_err(crate::api_legacy::maintenance_failure)?;
    if let (Some(home), Some(previous)) = (&mut maintenance, previous_lock) {
        home.finish_maintenance(previous).await?;
    }
    #[derive(Serialize)]
    struct ImportOut {
        imported: u64,
    }
    render_payload(
        &ImportOut { imported: applied },
        format!("imported\t{applied}"),
        output,
    )
}

fn cluster_from_desired(ds: &DesiredState) -> HomeObject {
    HomeObject::new(
        Kind::Cluster,
        CLUSTER_NAME,
        Spec::Cluster(mediaops_core::ClusterSpec {
            max_copy: ds.max_copy(),
            min_free: ds.min_free(),
            range_len: ds.range_len(),
            range_concurrency: ds.range_concurrency(),
            grabber: ds.grabber(),
            lock: ds.lock(),
            encode_pause: false,
            library_root: String::new(),
            roots: ds.paths().to_vec(),
        }),
        StatusBody::Cluster(mediaops_core::ClusterStatus::default()),
    )
}

async fn resolve_title(api: &HomeApi, raw: &str) -> Result<String, AppError> {
    if mediaops_core::TitleId::parse(raw).is_ok() {
        return Ok(raw.to_string());
    }
    let items = api.list(None).await.map_err(map_client)?;
    resolve_from_objects(&items, raw)
}

fn resolve_from_objects(items: &[HomeObject], raw: &str) -> Result<String, AppError> {
    let needle = mediaops_core::title_key(raw);
    let matches: BTreeSet<String> = items
        .iter()
        .filter_map(|obj| {
            let id = title_id_of(obj)?;
            let mut hints = format!("{id} {}", human_title(&id));
            match &obj.status {
                StatusBody::Hold(st) => {
                    hints.push_str(&st.release);
                    if let Some(placement) = &st.placement {
                        hints.push_str(&placement.label());
                    }
                }
                StatusBody::RemoteFile(st) => hints.push_str(&st.rel_path),
                StatusBody::Title(st) => hints.push_str(&st.path),
                _ => {}
            }
            (!needle.is_empty() && mediaops_core::title_key(&hints).contains(&needle)).then_some(id)
        })
        .collect();
    match matches.len() {
        1 => Ok(matches.into_iter().next().expect("one title")),
        0 => Err(AppError::Usage(format!(
            "`{raw}` is not a TitleId and is not already known"
        ))),
        _ => Err(AppError::Usage(format!(
            "name `{raw}` is ambiguous; use a TitleId"
        ))),
    }
}

fn human_title(title_id: &str) -> String {
    TitleId::parse(title_id)
        .map(|id| crate::out::human_title_id(&id))
        .unwrap_or_else(|_| title_id.to_string())
}

fn title_id_of(obj: &HomeObject) -> Option<String> {
    match &obj.spec {
        Spec::Title(s) => Some(s.title_id.clone()),
        Spec::Want(s) => Some(s.title_id.clone()),
        Spec::Job(s) => Some(s.title_id.clone()),
        Spec::Hold(s) => Some(s.title_id.clone()),
        _ => match &obj.status {
            StatusBody::RemoteFile(s) if !s.title_id.is_empty() => Some(s.title_id.clone()),
            _ => None,
        },
    }
}

fn render_one(obj: &HomeObject, output: Output) -> String {
    match output {
        Output::Json => serde_json::to_string(obj).expect("Home object serializes"),
        Output::LegacyJson => serde_json::to_string(&mediaops_core::Envelope::ok(obj))
            .expect("Home object envelope serializes"),
        Output::Table => format_row(obj),
        Output::Wide => render_wide(std::slice::from_ref(obj)),
    }
}

fn render_list(items: &[HomeObject], output: Output) -> String {
    if output.is_json() {
        #[derive(Serialize)]
        struct List<'a> {
            items: &'a [HomeObject],
        }
        let list = List { items };
        return if output == Output::LegacyJson {
            serde_json::to_string(&mediaops_core::Envelope::ok(list))
                .expect("Home list envelope serializes")
        } else {
            serde_json::to_string(&list).expect("Home list serializes")
        };
    }
    if items.is_empty() {
        return String::new();
    }
    if output == Output::Wide {
        return render_wide(items);
    }
    items.iter().map(format_row).collect::<Vec<_>>().join("\n")
}

fn format_row(obj: &HomeObject) -> String {
    let title = title_id_of(obj).unwrap_or_else(|| obj.metadata.name.clone());
    let phase = phase_of(obj);
    format!("{title}\t{}\t{phase}", obj.kind.as_str())
}

fn render_wide(items: &[HomeObject]) -> String {
    let rows: Vec<[String; 5]> = items
        .iter()
        .map(|obj| {
            [
                title_id_of(obj).unwrap_or_else(|| obj.metadata.name.clone()),
                obj.kind.as_str().to_string(),
                obj.metadata.name.clone(),
                phase_of(obj),
                obj.metadata.resource_version.to_string(),
            ]
        })
        .collect();
    // Measure the whole result once: tabs align to terminal stops, not to the
    // longest cell. The final column needs no padding or trailing whitespace.
    let widths: [usize; 4] = std::array::from_fn(|column| {
        rows.iter()
            .map(|row| row[column].width())
            .max()
            .unwrap_or(0)
    });
    let mut lines = Vec::with_capacity(rows.len());
    for row in rows {
        let mut line = String::new();
        for (column, cell) in row.iter().enumerate() {
            line.push_str(cell);
            if let Some(width) = widths.get(column) {
                line.push_str(&" ".repeat(width - cell.width() + 2));
            }
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn phase_of(obj: &HomeObject) -> String {
    match &obj.status {
        StatusBody::Job(s) => s.phase.as_str().to_string(),
        StatusBody::Want(s) => s.phase.as_str().to_string(),
        StatusBody::Node(s) => {
            if mediaops_core::node_is_ready(s.ready, s.last_heartbeat_unix, unix_now()) {
                "Ready".into()
            } else {
                "NotReady".into()
            }
        }
        StatusBody::Title(s) if s.drifted => "drifted".into(),
        StatusBody::Title(s) if !s.observed_files().is_empty() => "installed".into(),
        StatusBody::Hold(s) => s.reason.clone(),
        _ => String::new(),
    }
}

fn watch_type(n: i32) -> &'static str {
    match n {
        1 => "ADDED",
        2 => "MODIFIED",
        3 => "DELETED",
        _ => "UNKNOWN",
    }
}

pub(crate) fn map_client(err: ClientError) -> AppError {
    match err {
        ClientError::Home(mediaops_core::HomeError::NotFound { .. }) => {
            AppError::Usage(err.to_string())
        }
        ClientError::Home(mediaops_core::HomeError::Denied(_)) => AppError::Policy(err.to_string()),
        ClientError::Home(mediaops_core::HomeError::Invalid(msg)) => AppError::Usage(msg),
        ClientError::Rpc {
            code: tonic::Code::NotFound | tonic::Code::InvalidArgument,
            message,
        } => AppError::Usage(message),
        ClientError::Rpc {
            code: tonic::Code::PermissionDenied,
            message,
        } => AppError::Policy(message),
        other => AppError::Runtime(anyhow::anyhow!(other.to_string())),
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::Duration;

    use mediaops_apiserver::{ApiConfig, serve_api};
    use mediaops_core::{
        Bytes, ClusterSpec, ClusterStatus, JobPhase, JobStatus, NodeSpec, NodeStatus,
        RemoteFileStatus, TitleKind, TitleStatus, VerifiedStagingHandle, WantPhase, WantStatus,
        WorkerKind, install, parse_placement, parse_remote, staging_path,
    };
    use mediaops_transfer::{
        PullSpec, connect_home, grpc_source, list_entries, pull_file_with_progress,
    };

    fn node_rows() -> Vec<HomeObject> {
        [
            (WorkerKind::Inventory, true, 13409),
            (WorkerKind::Pull, false, 3),
            (WorkerKind::Scheduler, true, 13360),
        ]
        .into_iter()
        .map(|(worker_kind, ready, version)| {
            let mut node = HomeObject::new(
                Kind::Node,
                worker_kind.node_name(),
                Spec::Node(NodeSpec { worker_kind }),
                StatusBody::Node(NodeStatus {
                    ready,
                    last_heartbeat_unix: unix_now(),
                    ..NodeStatus::default()
                }),
            );
            node.metadata.resource_version = version;
            node
        })
        .collect()
    }

    #[test]
    fn wide_node_screen_aligns_every_column_without_tabs() {
        assert_eq!(
            render_list(&node_rows(), Output::Wide),
            concat!(
                "inventory  Node  inventory  Ready     13409\n",
                "pull       Node  pull       NotReady  3\n",
                "scheduler  Node  scheduler  Ready     13360"
            )
        );
    }

    #[test]
    fn wide_columns_use_terminal_width_for_international_titles() {
        let titles: Vec<_> = [
            "movie:key:東京.2026",
            "movie:key:ab.2026",
            "movie:key:e\u{301}.2026",
        ]
        .into_iter()
        .map(|id| {
            HomeObject::new(
                Kind::Want,
                id,
                Spec::Want(WantSpec {
                    title_id: id.into(),
                }),
                StatusBody::Want(WantStatus::default()),
            )
        })
        .collect();
        assert_eq!(
            render_list(&titles, Output::Wide),
            concat!(
                "movie:key:東京.2026  Want  movie:key:東京.2026  open  0\n",
                "movie:key:ab.2026    Want  movie:key:ab.2026    open  0\n",
                "movie:key:e\u{301}.2026     Want  movie:key:e\u{301}.2026     open  0"
            )
        );
    }

    #[test]
    fn wide_single_and_empty_screens_preserve_pipeline_and_json_contracts() {
        let nodes = node_rows();
        assert_eq!(render_list(&[], Output::Wide), "");
        assert_eq!(
            render_one(&nodes[1], Output::Wide),
            "pull  Node  pull  NotReady  3"
        );
        assert_eq!(
            render_list(&nodes, Output::Table),
            "inventory\tNode\tReady\npull\tNode\tNotReady\nscheduler\tNode\tReady"
        );
        let raw: serde_json::Value =
            serde_json::from_str(&render_list(&nodes, Output::Json)).expect("raw JSON");
        let legacy: serde_json::Value =
            serde_json::from_str(&render_list(&nodes, Output::LegacyJson)).expect("envelope");
        assert_eq!(raw["items"], serde_json::to_value(&nodes).unwrap());
        assert_eq!(legacy["data"], raw);
        assert_eq!(legacy["ok"], true);
    }

    #[test]
    fn home_human_screens_are_stable_and_do_not_hide_drift() {
        assert_eq!(format_status(&[], None), "nothing happening");
        let id = "movie:key:matrix.1999";
        let want = HomeObject::new(
            Kind::Want,
            id,
            Spec::Want(WantSpec {
                title_id: id.into(),
            }),
            StatusBody::Want(WantStatus::default()),
        );
        assert_eq!(
            format_status(std::slice::from_ref(&want), Some(1 << 30)),
            "want      Matrix (1999)\n\ndisk      1.0 GiB free"
        );
        assert_eq!(
            format_why(id, std::slice::from_ref(&want)),
            "Matrix (1999)\nmovie:key:matrix.1999\n\ngrab      wanted, not on the box"
        );
        let remote = HomeObject::new(
            Kind::RemoteFile,
            "box/file",
            Spec::RemoteFile,
            StatusBody::RemoteFile(RemoteFileStatus::default()),
        );
        let title = HomeObject::new(
            Kind::Title,
            id,
            Spec::Title(TitleSpec {
                title_id: id.into(),
                desired_present: true,
            }),
            StatusBody::Title(TitleStatus {
                path: "movies/Matrix.(1999)/Matrix.(1999).mkv".into(),
                drifted: true,
                ..TitleStatus::default()
            }),
        );
        assert_eq!(
            format_why(id, &[title, want, remote]),
            "Matrix (1999)\nmovie:key:matrix.1999\n\nlibrary   drifted\nwant      open, listed on the box"
        );
    }

    #[test]
    fn spoken_names_ignore_case_spacing_and_duplicate_files() {
        let mut items = Vec::new();
        for episode in ["one", "two"] {
            items.push(HomeObject::new(
                Kind::RemoteFile,
                episode,
                Spec::RemoteFile,
                StatusBody::RemoteFile(RemoteFileStatus {
                    title_id: "series:key:mrrobot.2015".into(),
                    ..RemoteFileStatus::default()
                }),
            ));
        }
        assert_eq!(
            resolve_from_objects(&items, "Mr Robot").expect("spoken name"),
            "series:key:mrrobot.2015"
        );
        items.push(HomeObject::new(
            Kind::Want,
            "series:key:mrrobot.2025",
            Spec::Want(WantSpec {
                title_id: "series:key:mrrobot.2025".into(),
            }),
            StatusBody::Want(WantStatus::default()),
        ));
        assert!(
            resolve_from_objects(&items, "mr robot").is_err(),
            "different titles are ambiguous"
        );
    }

    #[test]
    fn json_flag_and_output_json_have_distinct_contracts() {
        let obj = HomeObject::new(
            Kind::Want,
            "movie:tmdb:603",
            Spec::Want(WantSpec {
                title_id: "movie:tmdb:603".into(),
            }),
            StatusBody::Want(WantStatus::default()),
        );
        let raw: serde_json::Value = serde_json::from_str(&render_one(
            &obj,
            Output::parse(Some("json"), false).unwrap(),
        ))
        .unwrap();
        let legacy: serde_json::Value =
            serde_json::from_str(&render_one(&obj, Output::parse(None, true).unwrap())).unwrap();
        assert_eq!(raw["kind"], "Want");
        assert!(raw.get("ok").is_none());
        assert_eq!(legacy["ok"], true);
        assert_eq!(legacy["data"]["kind"], "Want");
        assert!(Output::parse(Some("json"), true).is_err());
    }

    #[tokio::test]
    async fn legacy_import_merges_each_file_and_preserves_runtime_settings() {
        let dir = crate::test_support::scratch("legacy-api-import");
        let library = crate::test_support::library_root(&dir);
        let socket = dir.join("api.sock");
        let api_task = tokio::spawn(serve_api(ApiConfig {
            socket: socket.clone(),
            api_db: dir.join("api.db"),
        }));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let api = loop {
            if let Ok(api) = HomeApi::connect(&socket, Actor::Import).await {
                break api;
            }
            assert!(tokio::time::Instant::now() < deadline, "API startup");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let cluster = ClusterSpec {
            library_root: library.display().to_string(),
            encode_pause: true,
            ..ClusterSpec::default()
        };
        api.apply(HomeObject::new(
            Kind::Cluster,
            CLUSTER_NAME,
            Spec::Cluster(cluster.clone()),
            StatusBody::Cluster(ClusterStatus::default()),
        ))
        .await
        .expect("runtime cluster");
        let id = TitleId::parse("album:key:yes.relayer").expect("album");
        api.apply(HomeObject::new(
            Kind::Title,
            id.render(),
            Spec::Title(TitleSpec {
                title_id: id.render(),
                desired_present: true,
            }),
            StatusBody::Title(TitleStatus::default()),
        ))
        .await
        .expect("existing empty title");
        let paths = [
            "music/Yes/Relayer.(1974)/Relayer.(1974).01.The.Gates.Of.Delirium.flac",
            "music/Yes/Relayer.(1974)/Relayer.(1974).02.Sound.Chaser.flac",
        ];
        std::fs::create_dir_all(library.join(paths[0]).parent().expect("parent"))
            .expect("album folder");
        std::fs::write(library.join(paths[0]), b"audio").expect("present file");
        let digest = mediaops_core::Blake3Hex::of_reader(&b"audio"[..]).expect("digest");
        let state_db = dir.join("state.db");
        let store = Store::open(&state_db).await.expect("legacy store");
        store
            .put_machine("library_root", &library.display().to_string())
            .await
            .expect("root");
        for path in paths {
            store
                .record_install(&id, &digest, path)
                .await
                .expect("legacy proof");
        }
        let config = Some(dir.join("absent-config.toml"));
        assert_eq!(
            import_legacy(
                config.clone(),
                Some(state_db.clone()),
                Output::Table,
                Some(socket.clone()),
            )
            .await
            .expect("import"),
            "imported\t1"
        );
        let title = api.get(Kind::Title, &id.render()).await.expect("title");
        let StatusBody::Title(status) = title.status else {
            panic!("title status")
        };
        let files = status.observed_files();
        assert_eq!(
            files.len(),
            2,
            "same-title files must not overwrite each other"
        );
        assert!(
            !files
                .iter()
                .find(|f| f.path == paths[0])
                .expect("first")
                .drifted
        );
        assert!(
            files
                .iter()
                .find(|f| f.path == paths[1])
                .expect("missing")
                .drifted
        );
        assert_eq!(
            import_legacy(config, Some(state_db), Output::Table, Some(socket))
                .await
                .expect("repeat"),
            "imported\t0"
        );
        let current = api.get(Kind::Cluster, CLUSTER_NAME).await.expect("cluster");
        assert_eq!(
            current.spec,
            Spec::Cluster(cluster),
            "maintenance restores lock without resetting runtime settings"
        );
        api_task.abort();
        let _ = api_task.await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn one_pull_reaches_installed_and_get_json_is_raw_object() {
        let _serial = crate::test_support::serial_net();
        let lb =
            crate::test_support::start_pair(Some(crate::test_support::MOVIE_REL), &[7u8; 64]).await;
        let dir = crate::test_support::scratch("home-e2e");
        let library = crate::test_support::library_root(&dir);
        let api_sock = dir.join("api.sock");
        let api_task = tokio::spawn(serve_api(ApiConfig {
            socket: api_sock.clone(),
            api_db: dir.join("api.db"),
        }));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let api = loop {
            match HomeApi::connect(&api_sock, Actor::Cli).await {
                Ok(api) => break api,
                Err(err) => {
                    if tokio::time::Instant::now() >= deadline {
                        panic!("api: {err}");
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        };

        api.apply(HomeObject::new(
            Kind::Cluster,
            CLUSTER_NAME,
            Spec::Cluster(ClusterSpec {
                max_copy: Bytes::new(1 << 30),
                min_free: Bytes::new(0),
                range_len: Bytes::new(64),
                library_root: library.display().to_string(),
                roots: vec![mediaops_core::PathRoot {
                    id: "seedbox".into(),
                    path: "/data".into(),
                    kind: Some(TitleKind::Movie),
                }],
                ..ClusterSpec::default()
            }),
            StatusBody::Cluster(ClusterStatus::default()),
        ))
        .await
        .expect("cluster");

        let inv = HomeApi::connect(&api_sock, Actor::Inventory)
            .await
            .expect("inv");
        inv.apply(HomeObject::new(
            Kind::Node,
            WorkerKind::Inventory.node_name(),
            Spec::Node(NodeSpec {
                worker_kind: WorkerKind::Inventory,
            }),
            StatusBody::Node(NodeStatus {
                ready: true,
                last_heartbeat_unix: unix_now(),
                list_generation: 1,
                list_completed_unix: unix_now(),
            }),
        ))
        .await
        .expect("inv node");

        let channel = connect_home(&lb.sock, &lb.tls_dir).await.expect("list");
        let entries = list_entries(channel).await.expect("entries");
        let entry = entries
            .iter()
            .find(|e| mediaops_core::is_media_file(e.r#ref()))
            .expect("media");
        let (title_id, _) =
            parse_remote(Some(TitleKind::Movie), entry.r#ref().rel_path()).expect("classify");
        inv.apply(HomeObject::new(
            Kind::RemoteFile,
            format!(
                "{}/{}",
                entry.r#ref().root_id(),
                entry.r#ref().rel_path().display()
            ),
            Spec::RemoteFile,
            StatusBody::RemoteFile(RemoteFileStatus {
                root_id: entry.r#ref().root_id().to_string(),
                rel_path: entry.r#ref().rel_path().display().to_string(),
                len: entry.len(),
                parse_ok: true,
                title_id: title_id.render(),
                list_generation: 1,
            }),
        ))
        .await
        .expect("remote");
        api.apply(HomeObject::new(
            Kind::Want,
            title_id.render(),
            Spec::Want(WantSpec {
                title_id: title_id.render(),
            }),
            StatusBody::Want(WantStatus {
                phase: WantPhase::Open,
            }),
        ))
        .await
        .expect("want");

        let sched = HomeApi::connect(&api_sock, Actor::Scheduler)
            .await
            .expect("sched");
        let pull = HomeApi::connect(&api_sock, Actor::Pull)
            .await
            .expect("pull");
        pull.apply(HomeObject::new(
            Kind::Node,
            WorkerKind::Pull.node_name(),
            Spec::Node(NodeSpec {
                worker_kind: WorkerKind::Pull,
            }),
            StatusBody::Node(NodeStatus {
                ready: true,
                last_heartbeat_unix: unix_now(),
                ..NodeStatus::default()
            }),
        ))
        .await
        .expect("pull node");

        let job = wait_job(&api).await;
        let Spec::Job(spec) = job.spec.clone() else {
            panic!("job");
        };
        let mut bound = job.clone();
        if let Spec::Job(s) = &mut bound.spec {
            s.node_name = WorkerKind::Pull.node_name().to_string();
        }
        let mut bound = sched.patch(bound, "bind").await.expect("bind");
        bound.status = StatusBody::Job(JobStatus {
            phase: JobPhase::Pulling,
            attempts: 1,
            started_unix: unix_now(),
            ..JobStatus::default()
        });
        let mut bound = pull.patch(bound, "status").await.expect("claim job");
        let tid = mediaops_core::TitleId::parse(&spec.title_id).expect("tid");
        let final_name = Path::new(&spec.dest_rel)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file.bin")
            .to_string();
        let ch = connect_home(&lb.sock, &lb.tls_dir).await.expect("pull ch");
        pull_file_with_progress(
            grpc_source(ch),
            &PullSpec {
                library_root: library.clone(),
                title_id: tid.clone(),
                final_name: final_name.clone(),
                remote: mediaops_core::RemoteRef::from_wire_parts(
                    spec.remote_root.clone(),
                    PathBuf::from(&spec.remote_path),
                )
                .expect("ref"),
                file_len: spec.file_len,
                range_len: spec.range_len.max(1),
                concurrency: 1,
            },
            |_, _| {},
        )
        .await
        .expect("ranges");
        let staged = library.join(staging_path(&tid, &final_name).expect("stage"));
        let digest =
            mediaops_core::Blake3Hex::of_reader(std::fs::File::open(&staged).expect("staged file"))
                .expect("digest");
        if let StatusBody::Job(st) = &mut bound.status {
            st.phase = JobPhase::Verifying;
            st.bytes_done = spec.file_len;
            st.verified_b3 = Some(digest);
        }
        let bound = pull
            .patch(bound, "status")
            .await
            .expect("verification proof");
        let (_, placement) = parse_placement(Path::new(&spec.dest_rel)).expect("place");
        let handle = VerifiedStagingHandle::verify(&library, &tid, staged, &placement).expect("h");
        let installed = install(&library, &tid, &handle).expect("install");
        if let Ok(mut title) = pull.get(Kind::Title, &spec.title_id).await {
            title.status = StatusBody::Title(TitleStatus {
                files: vec![mediaops_core::TitleFileStatus {
                    path: installed
                        .path
                        .strip_prefix(&library)
                        .expect("relative")
                        .display()
                        .to_string(),
                    install_b3: installed.whole_file_b3.clone(),
                    current_b3: installed.whole_file_b3,
                    drifted: false,
                }],
                ..TitleStatus::default()
            });
            pull.patch(title, "status").await.expect("title");
        }
        let mut done = bound;
        if let StatusBody::Job(status) = &mut done.status {
            status.phase = JobPhase::Installed;
        }
        pull.patch(done, "status")
            .await
            .expect("job status after title proof");

        let got = api.get(Kind::Job, &job.metadata.name).await.expect("get");
        match got.status {
            StatusBody::Job(ref st) => assert_eq!(st.phase, JobPhase::Installed),
            other => panic!("{other:?}"),
        }
        let title = api
            .get(Kind::Title, &spec.title_id)
            .await
            .expect("title get");
        match title.status {
            StatusBody::Title(st) => assert_eq!(st.observed_files().len(), 1),
            other => panic!("{other:?}"),
        }

        let rendered = render_one(&got, Output::Json);
        let value: serde_json::Value = serde_json::from_str(&rendered).expect("raw json");
        assert_eq!(value["kind"], "Job");
        assert_eq!(value["apiVersion"], "mediaops.home.v1");
        assert!(
            value.get("ok").is_none(),
            "raw object, not envelope: {value}"
        );

        api_task.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

    async fn wait_job(api: &HomeApi) -> HomeObject {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        loop {
            if let Ok(jobs) = api.list(Some(Kind::Job)).await
                && let Some(job) = jobs.into_iter().next()
            {
                return job;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("controller did not create a Pull Job");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn unix_now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}
