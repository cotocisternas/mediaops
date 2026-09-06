//! Best-effort Servarr unmonitor after a committed inventory listing.
//!
//! Failures are logged and never roll back the listing or write Job status.

use std::collections::HashSet;
use std::future::Future;
use std::path::Path;
use std::time::Duration;

use mediaops_core::{
    ClusterSpec, ControlPort, Grabber, HomeObject, Kind, Spec, StatusBody, TitleId, TitleKind,
};

use super::InventoryApi;

const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) trait UnmonitorPort {
    async fn wanted_missing(&self) -> anyhow::Result<Vec<TitleId>>;
    async fn unmonitor(&self, title_id: &TitleId) -> anyhow::Result<()>;
}

impl<C: ControlPort> UnmonitorPort for C {
    async fn wanted_missing(&self) -> anyhow::Result<Vec<TitleId>> {
        Ok(ControlPort::wanted_missing(self).await?)
    }

    async fn unmonitor(&self, title_id: &TitleId) -> anyhow::Result<()> {
        ControlPort::unmonitor(self, title_id).await?;
        Ok(())
    }
}

pub(crate) async fn after_successful_listing(
    api: &impl InventoryApi,
    control: &impl UnmonitorPort,
    cluster: &ClusterSpec,
) {
    match cluster.grabber {
        Grabber::None => return,
        Grabber::Servarr => {}
    }
    let wanted = match timed(control.wanted_missing()).await {
        Ok(ids) => ids,
        Err(err) => {
            tracing::warn!(error = %err, "inventory wanted_missing failed");
            return;
        }
    };
    let titles = match api.list(Some(Kind::Title)).await {
        Ok(titles) => titles,
        Err(err) => {
            tracing::warn!(error = %err, "inventory title list failed");
            return;
        }
    };
    for title_id in eligible_title_ids(&titles, &wanted, Path::new(&cluster.library_root)) {
        if let Err(err) = timed(control.unmonitor(&title_id)).await {
            tracing::warn!(error = %err, title_id = %title_id, "inventory unmonitor failed");
        }
    }
}

fn eligible_title_ids(
    titles: &[HomeObject],
    wanted_missing: &[TitleId],
    library_root: &Path,
) -> Vec<TitleId> {
    let wanted: HashSet<&TitleId> = wanted_missing.iter().collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for object in titles {
        let (Spec::Title(spec), StatusBody::Title(status)) = (&object.spec, &object.status) else {
            continue;
        };
        let Ok(title_id) = TitleId::parse(&spec.title_id) else {
            continue;
        };
        match title_id.kind() {
            TitleKind::Series => continue,
            TitleKind::Movie | TitleKind::Album => {}
        }
        let files = status.observed_files();
        if files.is_empty() || !wanted.contains(&title_id) {
            continue;
        }
        let present = files
            .iter()
            .any(|file| !file.drifted && local_regular_file(library_root, &file.path));
        if present && seen.insert(title_id.clone()) {
            out.push(title_id);
        }
    }
    out
}

fn local_regular_file(library_root: &Path, rel: &str) -> bool {
    if rel.is_empty() {
        return false;
    }
    let path = library_root.join(rel);
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return false;
    };
    !meta.file_type().is_symlink() && meta.is_file()
}

async fn timed<T>(fut: impl Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    match tokio::time::timeout(CONTROL_TIMEOUT, fut).await {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("control timed out")),
    }
}
