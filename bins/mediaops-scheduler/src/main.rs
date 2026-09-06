use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use mediaops_core::{
    Actor, HomeObject, JobPhase, Kind, NODE_HEARTBEAT_SECS, Spec, StatusBody, TitleId, WorkerKind,
    bind_priority, node_is_ready, pull_fits,
};
use mediaops_home_client::{ClientError, HomeApi, claim_process, default_api_socket};

#[derive(Parser, Debug)]
#[command(name = "mediaops-scheduler", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    Serve {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Command::Serve { socket } => {
            let socket = socket.unwrap_or_else(default_api_socket);
            let _owner = claim_process(&socket, "scheduler")?;
            let api = wait_api(&socket).await?;
            heartbeat_loop(api).await?;
        }
    }
    Ok(())
}

async fn wait_api(socket: &std::path::Path) -> anyhow::Result<HomeApi> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match HomeApi::connect(socket, Actor::Scheduler).await {
            Ok(api) => return Ok(api),
            Err(err) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(anyhow::anyhow!("api: {err}"));
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

async fn heartbeat_loop(api: HomeApi) -> anyhow::Result<()> {
    loop {
        if let Err(err) = api.heartbeat(WorkerKind::Scheduler, true, None).await {
            tracing::warn!(error = %err, "scheduler heartbeat failed");
        }
        if let Err(err) = bind_pending(&api).await {
            tracing::warn!(error = %err, "scheduler bind failed");
        }
        tokio::time::sleep(Duration::from_secs(NODE_HEARTBEAT_SECS)).await;
    }
}

trait SchedulerApi {
    async fn get(&self, kind: Kind, name: &str) -> Result<HomeObject, ClientError>;
    async fn list(&self, kind: Option<Kind>) -> Result<Vec<HomeObject>, ClientError>;
    async fn patch(&self, object: HomeObject, subresource: &str)
    -> Result<HomeObject, ClientError>;
}

impl SchedulerApi for HomeApi {
    async fn get(&self, kind: Kind, name: &str) -> Result<HomeObject, ClientError> {
        HomeApi::get(self, kind, name).await
    }
    async fn list(&self, kind: Option<Kind>) -> Result<Vec<HomeObject>, ClientError> {
        HomeApi::list(self, kind).await
    }
    async fn patch(
        &self,
        object: HomeObject,
        subresource: &str,
    ) -> Result<HomeObject, ClientError> {
        HomeApi::patch(self, object, subresource).await
    }
}

async fn bind_pending(api: &impl SchedulerApi) -> anyhow::Result<()> {
    let pull_ready = match api.get(Kind::Node, WorkerKind::Pull.node_name()).await {
        Ok(n) => match n.status {
            // A killed worker leaves ready=true behind, so the heartbeat age
            // decides. Same rule the Job controller applies to inventory.
            StatusBody::Node(st) => node_is_ready(st.ready, st.last_heartbeat_unix, unix_now()),
            _ => false,
        },
        Err(_) => false,
    };
    if !pull_ready {
        return Ok(());
    }
    let cluster = api.get(Kind::Cluster, mediaops_core::CLUSTER_NAME).await?;
    if !matches!(cluster.spec, Spec::Cluster(ref c) if !c.lock) {
        return Ok(());
    }
    let mut jobs = api.list(Some(Kind::Job)).await?;
    jobs.sort_by_key(|j| match &j.spec {
        Spec::Job(spec) => (
            TitleId::parse(&spec.title_id)
                .map(|t| bind_priority(&t))
                .unwrap_or(9),
            j.metadata.name.clone(),
        ),
        _ => (9, j.metadata.name.clone()),
    });
    let already: u64 = jobs
        .iter()
        .filter_map(|j| match (&j.spec, &j.status) {
            (Spec::Job(spec), StatusBody::Job(st))
                if !spec.node_name.is_empty() && !st.phase.is_terminal() =>
            {
                Some(spec.file_len)
            }
            _ => None,
        })
        .fold(0u64, u64::saturating_add);
    let mut bound = already;
    let mut reserved_disk = 0u64;
    for job in &jobs {
        if let (Spec::Job(s), StatusBody::Job(st)) = (&job.spec, &job.status)
            && !s.node_name.is_empty()
            && !st.phase.is_terminal()
        {
            reserved_disk = reserved_disk.saturating_add(mediaops_core::pull_remaining_bytes(s)?);
        }
    }
    for job in jobs {
        let Spec::Job(spec) = &job.spec else {
            continue;
        };
        let StatusBody::Job(st) = &job.status else {
            continue;
        };
        if st.phase != JobPhase::Pending || !spec.node_name.is_empty() {
            continue;
        }
        let free = mediaops_core::free_bytes(std::path::Path::new(&spec.library_root))?;
        let remaining = mediaops_core::pull_remaining_bytes(spec)?;
        if !pull_fits(u64::MAX, 0, spec.max_copy, bound, spec.file_len)
            || !pull_fits(free, spec.min_free, 0, reserved_disk, remaining)
            || !mediaops_core::install_fits(spec)?
        {
            continue;
        }
        let mut next = job.clone();
        if let Spec::Job(spec) = &mut next.spec {
            spec.node_name = WorkerKind::Pull.node_name().to_string();
            spec.worker_kind = WorkerKind::Pull.as_str().to_string();
        }
        match api.patch(next, "bind").await {
            Ok(_) => {
                bound = bound.saturating_add(spec.file_len);
                reserved_disk = reserved_disk.saturating_add(remaining);
            }
            Err(err) if err.is_denied() || err.is_conflict() || err.is_not_found() => {
                tracing::warn!(job = %job.metadata.name, error = %err, "scheduler bind skipped");
            }
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn init_tracing() {
    let subscriber = tracing_subscriber::fmt().with_writer(io::stderr);
    if io::stderr().is_terminal() {
        subscriber.init();
    } else {
        subscriber.json().init();
    }
}

#[cfg(test)]
mod tests;
