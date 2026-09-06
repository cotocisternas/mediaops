use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Parser;
use mediaops_core::{
    Actor, ControlPort, HoldSpec, HoldStatus, HomeObject, Kind, NODE_HEARTBEAT_SECS,
    RemoteFileStatus, Spec, StatusBody, WorkerKind, classify_remote, is_media_file,
    remote_file_name,
};
use mediaops_home_client::{
    HomeApi, claim_process, default_api_socket, default_gateway_socket, default_tls_dir,
};
use mediaops_proto::ControlPortClient;
use mediaops_proto::control_service_client::ControlServiceClient;
use mediaops_transfer::{connect_home, list_entries};

#[derive(Parser, Debug)]
#[command(name = "mediaops-inventory", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    Serve {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        gateway_socket: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Command::Serve {
            socket,
            gateway_socket,
            tls_dir,
        } => {
            let api_socket = socket.unwrap_or_else(default_api_socket);
            let gw = gateway_socket.unwrap_or_else(default_gateway_socket);
            let tls = tls_dir.unwrap_or_else(default_tls_dir);
            run(&api_socket, &gw, &tls).await?;
        }
    }
    Ok(())
}

async fn run(api_socket: &Path, gw: &Path, tls: &Path) -> anyhow::Result<()> {
    let _owner = claim_process(api_socket, "inventory")?;
    let api = wait_api(api_socket).await?;
    api.heartbeat(WorkerKind::Inventory, false, None).await?;
    let beat = api.clone();
    tokio::spawn(async move {
        loop {
            if let Err(err) = beat.touch_heartbeat(WorkerKind::Inventory).await {
                tracing::warn!(error = %err, "inventory heartbeat failed");
            }
            tokio::time::sleep(Duration::from_secs(NODE_HEARTBEAT_SECS)).await;
        }
    });
    loop {
        let result = async {
            // Invalidate before replacing any row. The final Node write is the commit marker.
            api.heartbeat(WorkerKind::Inventory, false, None).await?;
            refresh(&api, gw, tls).await
        }
        .await;
        if let Err(err) = result {
            tracing::warn!(error = %err, "inventory refresh failed");
        }
        tokio::time::sleep(Duration::from_secs(NODE_HEARTBEAT_SECS)).await;
    }
}

async fn wait_api(socket: &Path) -> anyhow::Result<HomeApi> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match HomeApi::connect(socket, Actor::Inventory).await {
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

async fn refresh(api: &HomeApi, gw: &Path, tls: &Path) -> anyhow::Result<()> {
    let channel = connect_home(gw, tls).await?;
    let entries = list_entries(channel.clone()).await?;
    let control = ControlPortClient::new(ControlServiceClient::new(channel));
    let holds = control.hold_list().await?;
    publish_inventory(api, &control, entries, holds).await
}

/// Small persistence port: publication failures are testable without WAN or sqlite.
trait InventoryApi {
    async fn get(
        &self,
        kind: Kind,
        name: &str,
    ) -> Result<HomeObject, mediaops_home_client::ClientError>;
    async fn list(
        &self,
        kind: Option<Kind>,
    ) -> Result<Vec<HomeObject>, mediaops_home_client::ClientError>;
    async fn apply(
        &self,
        object: HomeObject,
    ) -> Result<HomeObject, mediaops_home_client::ClientError>;
    async fn patch(
        &self,
        object: HomeObject,
        subresource: &str,
    ) -> Result<HomeObject, mediaops_home_client::ClientError>;
    async fn delete(
        &self,
        kind: Kind,
        name: &str,
    ) -> Result<HomeObject, mediaops_home_client::ClientError>;
    async fn heartbeat(
        &self,
        worker: WorkerKind,
        ready: bool,
        listing: Option<(i64, i64)>,
    ) -> Result<HomeObject, mediaops_home_client::ClientError>;
}

impl InventoryApi for HomeApi {
    async fn get(
        &self,
        kind: Kind,
        name: &str,
    ) -> Result<HomeObject, mediaops_home_client::ClientError> {
        HomeApi::get(self, kind, name).await
    }
    async fn list(
        &self,
        kind: Option<Kind>,
    ) -> Result<Vec<HomeObject>, mediaops_home_client::ClientError> {
        HomeApi::list(self, kind).await
    }
    async fn apply(
        &self,
        object: HomeObject,
    ) -> Result<HomeObject, mediaops_home_client::ClientError> {
        HomeApi::apply(self, object).await
    }
    async fn patch(
        &self,
        object: HomeObject,
        subresource: &str,
    ) -> Result<HomeObject, mediaops_home_client::ClientError> {
        HomeApi::patch(self, object, subresource).await
    }
    async fn delete(
        &self,
        kind: Kind,
        name: &str,
    ) -> Result<HomeObject, mediaops_home_client::ClientError> {
        HomeApi::delete(self, kind, name).await
    }
    async fn heartbeat(
        &self,
        worker: WorkerKind,
        ready: bool,
        listing: Option<(i64, i64)>,
    ) -> Result<HomeObject, mediaops_home_client::ClientError> {
        HomeApi::heartbeat(self, worker, ready, listing).await
    }
}

trait RejectRelease {
    async fn reject(&self, key: &mediaops_core::HoldKey) -> anyhow::Result<()>;
}

impl<C: ControlPort> RejectRelease for C {
    async fn reject(&self, key: &mediaops_core::HoldKey) -> anyhow::Result<()> {
        self.hold_reject(key).await?;
        Ok(())
    }
}

async fn publish_inventory(
    api: &impl InventoryApi,
    control: &(impl RejectRelease + unmonitor::UnmonitorPort),
    entries: Vec<mediaops_core::RemoteEntry>,
    holds: Vec<mediaops_core::HoldLiveItem>,
) -> anyhow::Result<()> {
    api.heartbeat(WorkerKind::Inventory, false, None).await?;
    reconcile_rejections(api, control, &holds).await?;
    let cluster = api.get(Kind::Cluster, mediaops_core::CLUSTER_NAME).await?;
    let Spec::Cluster(cs) = cluster.spec else {
        anyhow::bail!("Cluster body missing");
    };
    let existing = api.list(Some(Kind::RemoteFile)).await?;
    let existing_holds = api.list(Some(Kind::Hold)).await?;
    let node = api
        .get(Kind::Node, WorkerKind::Inventory.node_name())
        .await?;
    let previous = match node.status {
        StatusBody::Node(st) => st.list_generation,
        _ => 0,
    };
    let list_gen = existing
        .iter()
        .chain(existing_holds.iter())
        .filter_map(|o| match &o.status {
            StatusBody::RemoteFile(st) => Some(st.list_generation),
            StatusBody::Hold(st) => Some(st.list_generation),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .max(previous)
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("list generation overflow"))?;
    let mut root_kinds = mediaops_core::RootKinds::new();
    for root in &cs.roots {
        root_kinds.insert(root.id.clone(), root.kind);
    }
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        if !is_media_file(entry.r#ref()) {
            continue;
        }
        let (parse_ok, title_id) = match classify_remote(&root_kinds, &entry) {
            Ok((id, _)) => (true, id.render()),
            Err(_) => (false, String::new()),
        };
        let name = remote_file_name(
            entry.r#ref().root_id(),
            &entry.r#ref().rel_path().display().to_string(),
        );
        seen.insert(name.clone());
        let mut obj = HomeObject::new(
            Kind::RemoteFile,
            &name,
            Spec::RemoteFile,
            StatusBody::RemoteFile(RemoteFileStatus {
                root_id: entry.r#ref().root_id().to_string(),
                rel_path: entry.r#ref().rel_path().display().to_string(),
                len: entry.len(),
                parse_ok,
                title_id,
                list_generation: list_gen,
            }),
        );
        if let Some(old) = existing.iter().find(|o| o.metadata.name == name) {
            obj.metadata = old.metadata.clone();
        }
        api.apply(obj).await?;
    }
    for old in &existing {
        if !seen.contains(&old.metadata.name) {
            api.delete(Kind::RemoteFile, &old.metadata.name).await?;
        }
    }
    for hold in holds {
        let name = format!("{}-{}", hold.key.title_id.render(), hold.key.release_id);
        let status = StatusBody::Hold(HoldStatus {
            list_generation: list_gen,
            rejection_observed: false,
            reason: hold.reason,
            size: hold.size,
            release: hold.output_path.unwrap_or_default(),
            remote_root: hold
                .remote
                .as_ref()
                .map(|r| r.root_id().to_string())
                .unwrap_or_default(),
            remote_path: hold
                .remote
                .as_ref()
                .map(|r| r.rel_path().display().to_string())
                .unwrap_or_default(),
            placement: hold.placement,
            added_unix: hold.added_unix,
        });
        match api.get(Kind::Hold, &name).await {
            Ok(mut existing) => {
                let mut status = status;
                if let (StatusBody::Hold(before), StatusBody::Hold(after)) =
                    (&existing.status, &mut status)
                {
                    after.rejection_observed = before.rejection_observed;
                }
                existing.status = status;
                api.patch(existing, "status").await?;
            }
            Err(err) if err.is_not_found() => {
                api.apply(HomeObject::new(
                    Kind::Hold,
                    name,
                    Spec::Hold(HoldSpec {
                        title_id: hold.key.title_id.render(),
                        release_id: hold.key.release_id.to_string(),
                        decision: mediaops_core::HoldDecisionSpec::Empty,
                    }),
                    status,
                ))
                .await?;
            }
            Err(err) => return Err(err.into()),
        }
    }
    api.heartbeat(WorkerKind::Inventory, true, Some((list_gen, unix_now())))
        .await?;
    unmonitor::after_successful_listing(api, control, &cs).await;
    Ok(())
}

async fn reconcile_rejections(
    api: &impl InventoryApi,
    control: &impl RejectRelease,
    live: &[mediaops_core::HoldLiveItem],
) -> anyhow::Result<()> {
    for mut obj in api.list(Some(Kind::Hold)).await? {
        let (Spec::Hold(spec), StatusBody::Hold(status)) = (&obj.spec, &obj.status) else {
            continue;
        };
        if spec.decision != mediaops_core::HoldDecisionSpec::Rejected || status.rejection_observed {
            continue;
        }
        let key = mediaops_core::HoldKey::new(
            mediaops_core::TitleId::parse(&spec.title_id)?,
            mediaops_core::ReleaseId::parse(&spec.release_id)?,
        );
        // If the exact release has already disappeared, a previous rejection
        // may have succeeded before the API acknowledgement. Do not repeat it.
        if live.iter().any(|item| item.key == key) {
            control.reject(&key).await?;
        }
        if let StatusBody::Hold(status) = &mut obj.status {
            status.rejection_observed = true;
        }
        api.patch(obj, "status").await?;
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

mod unmonitor;

#[cfg(test)]
mod tests;
