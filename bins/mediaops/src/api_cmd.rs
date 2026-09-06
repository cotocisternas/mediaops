//! Home API client verbs: get / apply / delete / watch-objects / reconcile.

use std::collections::BTreeSet;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use mediaops_core::{
    Actor, HoldDecisionSpec, HomeObject, Kind, Spec, StatusBody, TitleId, TitleSpec, WantSpec,
};
use mediaops_home_client::{ClientError, HomeApi, default_api_socket};
use serde::Serialize;
use unicode_width::UnicodeWidthStr;

use crate::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    Auto,
    Table,
    Wide,
    Json,
}

impl Output {
    pub fn parse(raw: Option<&str>) -> Result<Self, AppError> {
        match raw {
            None => Ok(Self::Auto),
            Some("json") => Ok(Self::Json),
            Some("wide") => Ok(Self::Wide),
            Some("table" | "") => Ok(Self::Table),
            Some(other) => Err(AppError::Usage(format!("unknown -o `{other}`"))),
        }
    }

    fn for_objects(self, terminal: bool) -> Self {
        match self {
            Self::Auto if terminal => Self::Wide,
            Self::Auto => Self::Table,
            explicit => explicit,
        }
    }

    fn is_json(self) -> bool {
        self == Self::Json
    }
}

fn render_payload<T: Serialize>(
    payload: &T,
    tsv: String,
    output: Output,
) -> Result<String, AppError> {
    match output {
        Output::Json => serde_json::to_string(payload).map_err(|err| AppError::Runtime(err.into())),
        Output::Auto | Output::Table | Output::Wide => Ok(tsv),
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
    let output = output.for_objects(std::io::stdout().is_terminal());
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
        } else if output == Output::Wide {
            format!(
                "{}  {}  {}  {}  {}",
                watch_type(ev.r#type),
                obj.kind.as_str(),
                crate::out::inert(&obj.metadata.name),
                wide_phase(&obj, unix_now()),
                crate::out::inert(&wide_details(&obj, unix_now()))
            )
        } else {
            format!(
                "{}\t{}\t{}",
                watch_type(ev.r#type),
                obj.kind.as_str(),
                crate::out::inert(&obj.metadata.name)
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
    Ok(format_watch_line(&label, &title_id, meta))
}

pub async fn status_pretty(output: Output, socket: Option<PathBuf>) -> Result<String, AppError> {
    let api = connect(socket, Actor::Cli).await?;
    // One snapshot keeps pause, worker readiness and work mutually consistent.
    let items: Vec<_> = api
        .list(None)
        .await
        .map_err(map_client)?
        .into_iter()
        .filter(|obj| {
            matches!(
                obj.kind,
                Kind::Want | Kind::Job | Kind::Node | Kind::Cluster | Kind::Title
            )
        })
        .collect();
    if output.is_json() {
        return Ok(render_list(&items, output));
    }
    let free = items.iter().find_map(|obj| match &obj.spec {
        Spec::Cluster(cs) if !cs.library_root.is_empty() => {
            mediaops_core::free_bytes(std::path::Path::new(&cs.library_root)).ok()
        }
        _ => None,
    });
    Ok(format_status(&items, free))
}

fn format_status(items: &[HomeObject], free: Option<u64>) -> String {
    let mut lines = Vec::new();
    for obj in items {
        match (&obj.spec, &obj.status) {
            (Spec::Want(s), StatusBody::Want(st)) if st.phase == mediaops_core::WantPhase::Open => {
                lines.push(format!(
                    "want      {}",
                    crate::out::inert(&title_label(&s.title_id, items))
                ));
            }
            (Spec::Job(s), StatusBody::Job(st))
                if st.phase != mediaops_core::JobPhase::Installed =>
            {
                lines.extend(job_lines(obj, s, st));
            }
            _ => {}
        }
    }
    if lines.is_empty() {
        lines.push("nothing happening".into());
    }
    lines.push(String::new());
    lines.extend(control_lines(items));
    lines.push(String::new());
    lines.push(match free {
        Some(free) => format!("disk      {} free", crate::out::fmt_bytes(free)),
        None => "disk      free space unavailable".into(),
    });
    lines.join("\n")
}

fn control_lines(items: &[HomeObject]) -> Vec<String> {
    use mediaops_core::WorkerKind;
    let mut lines = Vec::new();
    match items.iter().find_map(|obj| match &obj.spec {
        Spec::Cluster(s) => Some(s),
        _ => None,
    }) {
        Some(cluster) => {
            lines.push(format!(
                "scheduler {}",
                if cluster.lock {
                    "paused (Cluster lock)"
                } else {
                    "enabled"
                }
            ));
            if cluster.encode_pause {
                lines.push("encode    paused".into());
            }
        }
        None => lines.push("config    Cluster unavailable".into()),
    }
    let now = unix_now();
    let mut unavailable = false;
    for worker in [
        WorkerKind::Scheduler,
        WorkerKind::Inventory,
        WorkerKind::Pull,
    ] {
        let status = items.iter().find_map(|obj| match (&obj.spec, &obj.status) {
            (Spec::Node(s), StatusBody::Node(st)) if s.worker_kind == worker => Some(st),
            _ => None,
        });
        let state = match status {
            Some(st) if mediaops_core::node_is_ready(st.ready, st.last_heartbeat_unix, now) => {
                "ready".into()
            }
            Some(st) => {
                unavailable = true;
                if st.last_heartbeat_unix > 0 {
                    format!(
                        "not ready; last heartbeat {} ago",
                        crate::out::fmt_age(
                            now.saturating_sub(st.last_heartbeat_unix).max(0) as u64
                        )
                    )
                } else {
                    "not ready; no heartbeat".into()
                }
            }
            None => {
                unavailable = true;
                "missing".into()
            }
        };
        lines.push(format!("worker    {}  {state}", worker.as_str()));
    }
    if unavailable {
        lines.push("check     mediaops doctor".into());
    }
    lines
}

fn job_lines(
    obj: &HomeObject,
    spec: &mediaops_core::JobSpec,
    status: &mediaops_core::JobStatus,
) -> Vec<String> {
    let mut lines = vec![format!(
        "pull      {}  {}  {}",
        crate::out::inert(&object_label(obj).unwrap_or_else(|| human_title(&spec.title_id))),
        status.phase.as_str(),
        crate::out::fmt_progress(status.bytes_done, spec.file_len),
    )];
    lines.push(format!(
        "          Job {}{}",
        crate::out::inert(&obj.metadata.name),
        if spec.node_name.is_empty() {
            String::new()
        } else {
            format!("  worker {}", crate::out::inert(&spec.node_name))
        }
    ));
    if !spec.dest_rel.is_empty() {
        lines.push(format!("          {}", crate::out::inert(&spec.dest_rel)));
    }
    if !status.message.is_empty() {
        lines.push(format!("          {}", crate::out::inert(&status.message)));
    } else if status.phase == mediaops_core::JobPhase::Pending {
        lines.push(format!(
            "          {}",
            if spec.node_name.is_empty() {
                "waiting for scheduler"
            } else {
                "assigned; waiting for worker"
            }
        ));
    }
    if matches!(
        status.phase,
        mediaops_core::JobPhase::Failed | mediaops_core::JobPhase::Refused
    ) {
        lines.push(format!(
            "inspect   mediaops get Job {} -o json",
            shell_arg(&obj.metadata.name)
        ));
    }
    lines
}

pub(crate) fn shell_arg(raw: &str) -> String {
    if !raw.is_empty()
        && raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/".contains(&byte))
    {
        raw.into()
    } else {
        format!("'{}'", crate::out::inert(raw).replace('\'', "'\"'\"'"))
    }
}

pub async fn why_pretty(
    title: String,
    output: Output,
    socket: Option<PathBuf>,
) -> Result<String, AppError> {
    let api = connect(socket, Actor::Cli).await?;
    let title_id = resolve_title(&api, &title).await?;
    let related: Vec<_> = api
        .list(None)
        .await
        .map_err(map_client)?
        .into_iter()
        .filter(|obj| {
            title_id_of(obj).as_deref() == Some(title_id.as_str())
                || obj.metadata.name == title_id
                || matches!(obj.kind, Kind::Cluster | Kind::Node)
        })
        .collect();
    if output.is_json() {
        return Ok(render_list(&related, output));
    }
    Ok(format_why(&title_id, &related))
}

fn format_why(title_id: &str, related: &[HomeObject]) -> String {
    let label = crate::out::inert(&title_label(title_id, related));
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
                    crate::out::inert(&st.reason),
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
                lines.extend(job_lines(obj, s, st));
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
                        crate::out::inert(&file.path),
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
    if related
        .iter()
        .any(|obj| matches!(obj.kind, Kind::Cluster | Kind::Node))
    {
        lines.push(String::new());
        lines.extend(control_lines(related));
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
    Ok(format_hold_list(&open, unix_now()))
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
    Ok(format_hold_decision(&written))
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

fn format_watch_line(label: &str, title_id: &str, meta: &str) -> String {
    use crate::out::{Style, Tone, finish, indent, inert, row};
    let style = Style::stdout();
    finish(vec![
        row(style, "watching", Tone::Go, &inert(label), meta),
        indent(style, &inert(title_id)),
        row(style, "progress", Tone::Quiet, "", "mediaops status"),
    ])
}

fn hold_label(spec: &mediaops_core::HoldSpec, status: &mediaops_core::HoldStatus) -> String {
    status
        .placement
        .as_ref()
        .map(crate::out::human_placement)
        .unwrap_or_else(|| human_title(&spec.title_id))
}

fn format_hold_list(items: &[HomeObject], now: i64) -> String {
    use crate::out::{Style, Tone, finish, fmt_age, fmt_bytes, inert, row};
    if items.is_empty() {
        return "nothing on hold".into();
    }
    let style = Style::stdout();
    let mut lines = Vec::new();
    for (index, obj) in items.iter().enumerate() {
        let (Spec::Hold(spec), StatusBody::Hold(status)) = (&obj.spec, &obj.status) else {
            continue;
        };
        let label = inert(&hold_label(spec, status));
        lines.push(format!(
            "{}.  {}  {}  {}",
            index + 1,
            style.bold(&label),
            fmt_bytes(status.size),
            fmt_age(now.saturating_sub(status.added_unix).max(0) as u64)
        ));
        lines.push(format!("    {}", style.dim(&inert(&spec.title_id))));
        if !status.reason.is_empty() {
            lines.push(format!("    {}", inert(&status.reason)));
        }
        if !status.release.is_empty() {
            lines.push(format!("    {}", style.dim(&inert(&status.release))));
        }
        lines.push(String::new());
    }
    if let Some(HomeObject {
        spec: Spec::Hold(spec),
        ..
    }) = items.first()
    {
        lines.push(row(
            style,
            "approve",
            Tone::Go,
            "",
            &format!(
                "mediaops hold approve {} {}",
                shell_arg(&spec.title_id),
                shell_arg(&spec.release_id)
            ),
        ));
    }
    finish(lines)
}

fn format_hold_decision(obj: &HomeObject) -> String {
    use crate::out::{Style, Tone, finish, indent, inert, row};
    let (Spec::Hold(spec), StatusBody::Hold(status)) = (&obj.spec, &obj.status) else {
        return String::new();
    };
    let style = Style::stdout();
    let approved = spec.decision == HoldDecisionSpec::Approved;
    let mut lines = vec![
        row(
            style,
            spec.decision.as_str(),
            if approved { Tone::Go } else { Tone::Quiet },
            &inert(&hold_label(spec, status)),
            "",
        ),
        indent(style, &inert(&spec.title_id)),
    ];
    if approved {
        lines.push(indent(
            style,
            "decision recorded; the controller will create a copy job",
        ));
        lines.push(row(
            style,
            "progress",
            Tone::Quiet,
            "",
            "mediaops get Job -o wide",
        ));
    }
    finish(lines)
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
                StatusBody::Title(st) => {
                    for file in st.observed_files() {
                        hints.push(' ');
                        hints.push_str(&file.path);
                    }
                }
                _ => {}
            }
            if let Spec::Job(spec) = &obj.spec {
                hints.push(' ');
                hints.push_str(&spec.dest_rel);
                hints.push(' ');
                hints.push_str(&spec.remote_path);
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

fn title_label(title_id: &str, items: &[HomeObject]) -> String {
    items
        .iter()
        .filter(|obj| title_id_of(obj).as_deref() == Some(title_id))
        .find_map(object_label)
        .unwrap_or_else(|| human_title(title_id))
}

fn object_label(obj: &HomeObject) -> Option<String> {
    match (&obj.spec, &obj.status) {
        (_, StatusBody::Hold(st)) => st.placement.as_ref().map(crate::out::human_placement),
        (_, StatusBody::Title(st)) => st
            .observed_files()
            .iter()
            .find_map(|file| crate::out::human_from_path(&file.path)),
        (Spec::Job(spec), _) => crate::out::human_from_path(&spec.dest_rel)
            .or_else(|| crate::out::human_from_path(&spec.remote_path)),
        (_, StatusBody::RemoteFile(st)) => crate::out::human_from_path(&st.rel_path),
        _ => None,
    }
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
    match output.for_objects(std::io::stdout().is_terminal()) {
        Output::Json => serde_json::to_string(obj).expect("Home object serializes"),
        Output::Auto | Output::Table => format_row(obj),
        Output::Wide => render_wide(std::slice::from_ref(obj)),
    }
}

fn render_list(items: &[HomeObject], output: Output) -> String {
    let output = output.for_objects(std::io::stdout().is_terminal());
    if output.is_json() {
        #[derive(Serialize)]
        struct List<'a> {
            items: &'a [HomeObject],
        }
        let list = List { items };
        return serde_json::to_string(&list).expect("Home list serializes");
    }
    if items.is_empty() {
        return if output == Output::Wide {
            "no objects found".into()
        } else {
            String::new()
        };
    }
    if output == Output::Wide {
        return render_wide(items);
    }
    items.iter().map(format_row).collect::<Vec<_>>().join("\n")
}

fn format_row(obj: &HomeObject) -> String {
    let title = title_id_of(obj).unwrap_or_else(|| obj.metadata.name.clone());
    let phase = phase_of(obj);
    format!(
        "{}\t{}\t{}",
        crate::out::inert(&title),
        obj.kind.as_str(),
        crate::out::inert(&phase)
    )
}

fn render_wide(items: &[HomeObject]) -> String {
    render_wide_at(items, unix_now())
}

fn render_wide_at(items: &[HomeObject], now: i64) -> String {
    let mut rows: Vec<[String; 4]> = vec![[
        "NAME".into(),
        "KIND".into(),
        "STATUS".into(),
        "DETAILS".into(),
    ]];
    rows.extend(items.iter().map(|obj| {
        [
            crate::out::inert(&obj.metadata.name),
            obj.kind.as_str().to_string(),
            crate::out::inert(&wide_phase(obj, now)),
            crate::out::inert(&wide_details(obj, now)),
        ]
    }));
    let widths: [usize; 3] = std::array::from_fn(|column| {
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
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

fn wide_phase(obj: &HomeObject, now: i64) -> String {
    match &obj.spec {
        Spec::Cluster(s) => if s.lock { "paused" } else { "enabled" }.into(),
        Spec::Hold(s) => if s.decision == HoldDecisionSpec::Empty {
            "open"
        } else {
            s.decision.as_str()
        }
        .into(),
        _ => phase_of_at(obj, now),
    }
}

fn wide_details(obj: &HomeObject, now: i64) -> String {
    match (&obj.spec, &obj.status) {
        (Spec::Job(s), StatusBody::Job(st)) => {
            let mut details = format!(
                "{}  {}",
                object_label(obj).unwrap_or_else(|| human_title(&s.title_id)),
                crate::out::fmt_progress(st.bytes_done, s.file_len)
            );
            if !s.node_name.is_empty() {
                details.push_str(&format!("  worker {}", s.node_name));
            }
            if !st.message.is_empty() {
                details.push_str(&format!("  {}", st.message));
            } else if st.phase == mediaops_core::JobPhase::Pending {
                details.push_str(if s.node_name.is_empty() {
                    "  waiting for scheduler"
                } else {
                    "  waiting for worker"
                });
            }
            details
        }
        (Spec::Node(s), StatusBody::Node(st)) => {
            let mut details = if st.last_heartbeat_unix > 0 {
                format!(
                    "heartbeat {} ago",
                    crate::out::fmt_age(now.saturating_sub(st.last_heartbeat_unix).max(0) as u64)
                )
            } else {
                "no heartbeat".into()
            };
            if s.worker_kind == mediaops_core::WorkerKind::Inventory {
                details.push_str(&if st.list_completed_unix > 0 {
                    format!(
                        "  inventory {} ago",
                        crate::out::fmt_age(
                            now.saturating_sub(st.list_completed_unix).max(0) as u64
                        )
                    )
                } else {
                    "  no completed inventory".into()
                });
            }
            details
        }
        (Spec::Cluster(s), _) => format!(
            "{}  reserve {}{}",
            s.library_root,
            crate::out::fmt_bytes(s.min_free.get()),
            if s.encode_pause {
                "  encode paused"
            } else {
                ""
            }
        ),
        (Spec::Title(_), StatusBody::Title(st)) => {
            let files = st.observed_files();
            match files.as_slice() {
                [file] => file.path.clone(),
                _ => format!(
                    "{} files  {} drifted",
                    files.len(),
                    files.iter().filter(|file| file.drifted).count()
                ),
            }
        }
        (Spec::Want(s), _) => human_title(&s.title_id),
        (Spec::Hold(s), StatusBody::Hold(st)) => format!(
            "{}  {}  {}",
            human_title(&s.title_id),
            crate::out::fmt_bytes(st.size),
            st.reason
        ),
        (_, StatusBody::RemoteFile(st)) => format!(
            "{}  {} / {}",
            crate::out::fmt_bytes(st.len),
            st.root_id,
            st.rel_path
        ),
        (_, StatusBody::Sync(st)) => crate::sync_format::summary(st),
        (_, StatusBody::Event(st)) => format!(
            "{} {}  {}  {}",
            st.involved_kind, st.involved_name, st.reason, st.message
        ),
        _ => String::new(),
    }
}

fn phase_of(obj: &HomeObject) -> String {
    phase_of_at(obj, unix_now())
}

fn phase_of_at(obj: &HomeObject, now: i64) -> String {
    match &obj.status {
        StatusBody::Sync(s) => s.phase.as_str().to_string(),
        StatusBody::Job(s) => s.phase.as_str().to_string(),
        StatusBody::Want(s) => s.phase.as_str().to_string(),
        StatusBody::Node(s) => {
            if mediaops_core::node_is_ready(s.ready, s.last_heartbeat_unix, now) {
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

    #[test]
    fn default_object_output_uses_terminal_tables_and_keeps_pipelines_deterministic() {
        let auto = Output::parse(None).expect("automatic output");
        assert_eq!(auto.for_objects(true), Output::Wide);
        assert_eq!(auto.for_objects(false), Output::Table);
        assert_eq!(
            Output::parse(Some("table")).unwrap().for_objects(true),
            Output::Table
        );
        assert_eq!(
            Output::parse(Some("wide")).unwrap().for_objects(false),
            Output::Wide
        );
        assert_eq!(
            Output::parse(Some("json")).unwrap().for_objects(true),
            Output::Json
        );
        assert!(Output::parse(Some("xml")).is_err());
    }

    #[test]
    fn get_sync_table_reports_its_planning_phase() {
        let obj = HomeObject::new(
            Kind::Sync,
            "sync-123",
            Spec::Sync(mediaops_core::SyncSpec::default()),
            StatusBody::Sync(mediaops_core::SyncStatus {
                phase: mediaops_core::SyncPhase::Scheduled,
                ..Default::default()
            }),
        );
        assert_eq!(format_row(&obj), "sync-123\tSync\tscheduled");
    }
    use std::path::Path;
    use std::time::Duration;

    use mediaops_apiserver::{ApiConfig, serve_api};
    use mediaops_core::{
        Bytes, CLUSTER_NAME, ClusterSpec, ClusterStatus, JobPhase, JobStatus, NodeSpec, NodeStatus,
        RemoteFileStatus, TitleKind, TitleStatus, VerifiedStagingHandle, WantPhase, WantStatus,
        WorkerKind, install, parse_placement, parse_remote, staging_path,
    };
    use mediaops_transfer::{
        PullSpec, connect_home, grpc_source, list_entries, pull_file_with_progress,
    };

    fn node_rows(now: i64) -> Vec<HomeObject> {
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
                    last_heartbeat_unix: now,
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
            render_wide_at(&node_rows(1000), 1000),
            concat!(
                "NAME       KIND  STATUS    DETAILS\n",
                "inventory  Node  Ready     heartbeat 0s ago  no completed inventory\n",
                "pull       Node  NotReady  heartbeat 0s ago\n",
                "scheduler  Node  Ready     heartbeat 0s ago"
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
                "NAME                 KIND  STATUS  DETAILS\n",
                "movie:key:東京.2026  Want  open    東京 (2026)\n",
                "movie:key:ab.2026    Want  open    Ab (2026)\n",
                "movie:key:e\u{301}.2026     Want  open    movie:key:e\u{301}.2026"
            )
        );
    }

    #[test]
    fn wide_single_and_empty_screens_preserve_pipeline_and_json_contracts() {
        let now = unix_now();
        let nodes = node_rows(now);
        assert_eq!(render_list(&[], Output::Wide), "no objects found");
        assert_eq!(render_list(&[], Output::Table), "");
        assert_eq!(
            render_wide_at(std::slice::from_ref(&nodes[1]), now),
            "NAME  KIND  STATUS    DETAILS\npull  Node  NotReady  heartbeat 0s ago"
        );
        assert_eq!(
            render_list(&nodes, Output::Table),
            "inventory\tNode\tReady\npull\tNode\tNotReady\nscheduler\tNode\tReady"
        );
        let raw: serde_json::Value =
            serde_json::from_str(&render_list(&nodes, Output::Json)).expect("raw JSON");
        assert_eq!(raw["items"], serde_json::to_value(&nodes).unwrap());
    }

    #[test]
    fn home_human_screens_are_stable_and_do_not_hide_drift() {
        assert_eq!(
            format_status(&[], None),
            concat!(
                "nothing happening\n\nconfig    Cluster unavailable\n",
                "worker    scheduler  missing\nworker    inventory  missing\n",
                "worker    pull  missing\ncheck     mediaops doctor\n\ndisk      free space unavailable"
            )
        );
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
            concat!(
                "want      Matrix (1999)\n\nconfig    Cluster unavailable\n",
                "worker    scheduler  missing\nworker    inventory  missing\n",
                "worker    pull  missing\ncheck     mediaops doctor\n\ndisk      1.0 GiB free"
            )
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
    fn job_screens_expose_progress_failure_and_exact_object_name() {
        let id = "movie:key:matrix.1999";
        let job = HomeObject::new(
            Kind::Job,
            "pull-7",
            Spec::Job(mediaops_core::JobSpec {
                title_id: id.into(),
                file_len: 1 << 30,
                dest_rel: "movies/Matrix.(1999)/Matrix.(1999).mkv".into(),
                node_name: "pull".into(),
                ..Default::default()
            }),
            StatusBody::Job(JobStatus {
                phase: JobPhase::Failed,
                bytes_done: 1 << 29,
                message: "disk reserve reached\ncheck storage".into(),
                ..Default::default()
            }),
        );
        assert_eq!(
            render_one(&job, Output::Wide),
            concat!(
                "NAME    KIND  STATUS  DETAILS\n",
                "pull-7  Job   failed  Matrix (1999)  50%  512 MiB / 1.0 GiB  worker pull  disk reserve reached check storage"
            )
        );
        assert_eq!(
            format_why(id, std::slice::from_ref(&job)),
            concat!(
                "Matrix (1999)\nmovie:key:matrix.1999\n\n",
                "pull      Matrix (1999)  failed  50%  512 MiB / 1.0 GiB\n",
                "          Job pull-7  worker pull\n",
                "          movies/Matrix.(1999)/Matrix.(1999).mkv\n",
                "          disk reserve reached check storage\n",
                "inspect   mediaops get Job pull-7 -o json"
            )
        );
        assert_eq!(
            render_one(&job, Output::Table),
            "movie:key:matrix.1999\tJob\tfailed"
        );
        let json: serde_json::Value =
            serde_json::from_str(&render_one(&job, Output::Json)).unwrap();
        assert_eq!(
            json["status"]["message"],
            "disk reserve reached\ncheck storage"
        );
    }

    #[test]
    fn paused_scheduling_and_unready_workers_are_visible_without_jobs() {
        let mut items = node_rows(unix_now());
        if let StatusBody::Node(status) = &mut items[1].status {
            status.last_heartbeat_unix = 0;
        }
        items.push(HomeObject::new(
            Kind::Cluster,
            CLUSTER_NAME,
            Spec::Cluster(ClusterSpec {
                lock: true,
                encode_pause: true,
                ..Default::default()
            }),
            StatusBody::Cluster(ClusterStatus::default()),
        ));
        assert_eq!(
            format_status(&items, Some(1 << 30)),
            concat!(
                "nothing happening\n\nscheduler paused (Cluster lock)\nencode    paused\n",
                "worker    scheduler  ready\nworker    inventory  ready\n",
                "worker    pull  not ready; no heartbeat\ncheck     mediaops doctor\n\ndisk      1.0 GiB free"
            )
        );
    }

    #[test]
    fn hold_and_watch_screens_explain_the_next_step() {
        assert_eq!(
            format_watch_line("Matrix (1999)", "movie:tmdb:603", ""),
            "watching  Matrix (1999)\n          movie:tmdb:603\nprogress  mediaops status"
        );
        let mut hold = HomeObject::new(
            Kind::Hold,
            "hold-1",
            Spec::Hold(mediaops_core::HoldSpec {
                title_id: "movie:tmdb:603".into(),
                release_id: "deadbeef".into(),
                ..Default::default()
            }),
            StatusBody::Hold(mediaops_core::HoldStatus {
                added_unix: 1000,
                size: 512,
                placement: Some(mediaops_core::Placement::movie("Matrix", 1999, "mkv")),
                reason: "Manual Import required.".into(),
                release: "Matrix.1999.Release".into(),
                ..Default::default()
            }),
        );
        assert_eq!(
            format_hold_list(std::slice::from_ref(&hold), 1012),
            concat!(
                "1.  Matrix (1999)  512 B  12s\n    movie:tmdb:603\n",
                "    Manual Import required.\n    Matrix.1999.Release\n\n",
                "approve   mediaops hold approve movie:tmdb:603 deadbeef"
            )
        );
        if let Spec::Hold(spec) = &mut hold.spec {
            spec.decision = HoldDecisionSpec::Approved;
        }
        assert_eq!(
            format_hold_decision(&hold),
            concat!(
                "approved  Matrix (1999)\n          movie:tmdb:603\n",
                "          decision recorded; the controller will create a copy job\n",
                "progress  mediaops get Job -o wide"
            )
        );
        assert_eq!(format_hold_list(&[], 1012), "nothing on hold");
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
    fn current_file_proofs_resolve_and_display_names_for_provider_ids() {
        let title = HomeObject::new(
            Kind::Title,
            "movie:tmdb:603",
            Spec::Title(TitleSpec {
                title_id: "movie:tmdb:603".into(),
                desired_present: true,
            }),
            StatusBody::Title(TitleStatus {
                files: vec![mediaops_core::TitleFileStatus {
                    path: "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                    install_b3: mediaops_core::Blake3Hex::of_bytes(b"movie"),
                    current_b3: mediaops_core::Blake3Hex::of_bytes(b"movie"),
                    drifted: false,
                }],
                ..Default::default()
            }),
        );
        let items = [title];
        assert_eq!(
            resolve_from_objects(&items, "The Matrix").unwrap(),
            "movie:tmdb:603"
        );
        assert_eq!(
            format_why("movie:tmdb:603", &items),
            "The Matrix (1999)\nmovie:tmdb:603\n\nlibrary   movies/The.Matrix.(1999)/The.Matrix.(1999).mkv"
        );
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
                ..NodeStatus::default()
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
