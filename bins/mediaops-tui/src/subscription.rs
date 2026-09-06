//! Serial baseline delivery followed by revision-aware watch replay.

use std::path::PathBuf;
use std::time::Duration;

use mediaops_core::Actor;
use mediaops_home_client::{ClientError, HomeApi, WatchEvent};
use tokio::sync::mpsc;

use crate::cache::{ObjectCache, ObjectKey};
use crate::session::SessionEvent;

pub(crate) const RPC_LIMIT: Duration = Duration::from_secs(5);

pub(crate) async fn run(socket: PathBuf, epoch: u64, tx: mpsc::Sender<SessionEvent>) {
    if let Err(message) = subscribe(socket, epoch, &tx).await {
        let _ = tx.send(SessionEvent::WatchFailed { epoch, message }).await;
    }
}

async fn subscribe(
    socket: PathBuf,
    epoch: u64,
    tx: &mpsc::Sender<SessionEvent>,
) -> Result<(), String> {
    let api = match bounded(HomeApi::connect(socket, Actor::Cli)).await {
        Ok(api) => api,
        Err(message) => {
            let _ = tx
                .send(SessionEvent::ConnectFailed { epoch, message })
                .await;
            return Ok(());
        }
    };
    // Establish the snapshot before List, but send List first to the reducer.
    // The server and HTTP/2 stream apply backpressure without blocking the UI.
    let mut watch = bounded(api.watch_home(None, 0)).await?;
    let objects = match bounded(api.list(None)).await {
        Ok(objects) => objects,
        Err(message) => {
            let _ = tx
                .send(SessionEvent::BaselineFailed { epoch, message })
                .await;
            return Ok(());
        }
    };
    let mut observed = ObjectCache::default();
    let local_epoch = observed.bump_epoch();
    observed.install_baseline(local_epoch, objects.clone());
    tx.send(SessionEvent::Connected {
        epoch,
        api: api.clone(),
    })
    .await
    .map_err(|e| e.to_string())?;
    tx.send(SessionEvent::Baseline { epoch, objects })
        .await
        .map_err(|e| e.to_string())?;
    while let Some(event) = watch.message().await.map_err(|e| e.to_string())? {
        let key = ObjectKey::from_object(event.object());
        let event = match event {
            WatchEvent::Added(obj) | WatchEvent::Modified(obj) if observed.get(&key).is_none() => {
                // A snapshot object absent from List could have been deleted
                // before List. Resolve it, rather than briefly resurrecting it.
                match tokio::time::timeout(RPC_LIMIT, api.get(key.kind, &key.name)).await {
                    Ok(Ok(current)) => WatchEvent::Added(current),
                    Ok(Err(err)) if err.is_not_found() => WatchEvent::Deleted(obj),
                    Ok(Err(err)) => return Err(err.to_string()),
                    Err(_) => return Err("snapshot verification timed out".into()),
                }
            }
            other => other,
        };
        if observed.apply_event(local_epoch, event.clone()) {
            tx.send(SessionEvent::Watch {
                epoch,
                event: Box::new(event),
            })
            .await
            .map_err(|e| e.to_string())?;
        }
    }
    tx.send(SessionEvent::WatchEnded { epoch })
        .await
        .map_err(|e| e.to_string())
}

pub(crate) async fn bounded<T>(
    future: impl Future<Output = Result<T, ClientError>>,
) -> Result<T, String> {
    tokio::time::timeout(RPC_LIMIT, future)
        .await
        .map_err(|_| "Home API request timed out".to_string())?
        .map_err(|e| e.to_string())
}
