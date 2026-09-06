mod support;
#[path = "support/sync.rs"]
mod sync_support;

use mediaops_core::{
    Actor, Blake3Hex, HomeObject, Kind, Spec, StatusBody, SyncDisposition, TitleSpec, TitleStatus,
};
use mediaops_home_client::HomeApi;
use sync_support::{MATRIX, configured, publish, scheduled, start};

#[tokio::test]
async fn healthy_imported_authority_proof_counts_as_present() {
    let home = configured("sync-proof").await;
    let relative = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
    let destination = home.dir.join("library").join(relative);
    std::fs::create_dir_all(destination.parent().expect("parent")).expect("directories");
    std::fs::write(&destination, b"good").expect("installed file");
    let digest = Blake3Hex::of_bytes(b"good");
    let importer = HomeApi::connect(&home.socket, Actor::Import)
        .await
        .expect("importer");
    importer
        .apply(HomeObject::new(
            Kind::Title,
            "movie:tmdb:603",
            Spec::Title(TitleSpec {
                title_id: "movie:tmdb:603".into(),
                desired_present: true,
            }),
            StatusBody::Title(TitleStatus {
                path: relative.into(),
                install_b3: Some(digest.clone()),
                current_b3: Some(digest),
                ..Default::default()
            }),
        ))
        .await
        .expect("verified proof");
    let pending = start(&home, "sync-present").await;
    publish(&home, 1, &[MATRIX]).await;
    let result = scheduled(pending).await;
    assert!(
        matches!(result.status, StatusBody::Sync(st) if st.entries[0].disposition == SyncDisposition::Present)
    );
    assert!(
        home.api
            .list(Some(Kind::Job))
            .await
            .expect("jobs")
            .is_empty()
    );
}

#[tokio::test]
async fn symlink_destination_is_blocked_and_target_is_preserved() {
    let home = configured("sync-symlink").await;
    let destination = home.dir.join("library/movies").join(MATRIX);
    std::fs::create_dir_all(destination.parent().expect("parent")).expect("directories");
    let target = home.dir.join("outside");
    std::fs::write(&target, b"keep").expect("target");
    std::os::unix::fs::symlink(&target, &destination).expect("symlink");
    let pending = start(&home, "sync-symlink").await;
    publish(&home, 1, &[MATRIX]).await;
    let result = scheduled(pending).await;
    assert!(
        matches!(result.status, StatusBody::Sync(st) if st.entries[0].disposition == SyncDisposition::Blocked)
    );
    assert!(
        home.api
            .list(Some(Kind::Job))
            .await
            .expect("jobs")
            .is_empty()
    );
    assert_eq!(std::fs::read(target).expect("unchanged"), b"keep");
}

#[tokio::test]
async fn proved_file_under_symlinked_parent_is_not_reported_present() {
    proved_file_under_symlink_is_blocked(false).await;
}

#[tokio::test]
async fn proved_file_under_symlinked_root_is_not_reported_present() {
    proved_file_under_symlink_is_blocked(true).await;
}

async fn proved_file_under_symlink_is_blocked(at_root: bool) {
    let home = configured("sync-proof-parent").await;
    let relative = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
    let root = home.dir.join("library");
    let destination = root.join(relative);
    std::fs::create_dir_all(destination.parent().expect("parent")).expect("directories");
    std::fs::write(&destination, b"good").expect("file");
    let digest = Blake3Hex::of_bytes(b"good");
    let importer = HomeApi::connect(&home.socket, Actor::Import)
        .await
        .expect("importer");
    importer
        .apply(HomeObject::new(
            Kind::Title,
            "movie:tmdb:603",
            Spec::Title(TitleSpec {
                title_id: "movie:tmdb:603".into(),
                desired_present: true,
            }),
            StatusBody::Title(TitleStatus {
                path: relative.into(),
                install_b3: Some(digest.clone()),
                current_b3: Some(digest),
                ..Default::default()
            }),
        ))
        .await
        .expect("proof");
    let directory = if at_root {
        root.as_path()
    } else {
        destination.parent().expect("parent")
    };
    let moved = home.dir.join("outside-movie");
    std::fs::rename(directory, &moved).expect("move parent");
    std::os::unix::fs::symlink(&moved, directory).expect("symlink parent");
    let pending = start(&home, "sync-proof-parent").await;
    publish(&home, 1, &[MATRIX]).await;
    let result = scheduled(pending).await;
    assert!(
        matches!(result.status, StatusBody::Sync(st) if st.entries[0].disposition == SyncDisposition::Blocked)
    );
}
