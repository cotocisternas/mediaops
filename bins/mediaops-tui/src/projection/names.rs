use std::collections::BTreeMap;
use std::path::Path;

use mediaops_core::{
    Placement, Spec, StatusBody, TitleId, TitleKind, TitleSource, parse_placement, parse_remote,
};

use crate::cache::ObjectCache;
use crate::inventory::committed_inventory_generation;
use crate::sanitize::sanitize;

#[derive(Default)]
pub(super) struct Names(BTreeMap<String, (u8, String)>);

impl Names {
    pub(super) fn from_cache(cache: &ObjectCache, now: i64) -> Self {
        let mut names = Self::default();
        let generation = committed_inventory_generation(cache.live(), now);
        for obj in cache.live() {
            match (&obj.spec, &obj.status) {
                (Spec::Title(spec), StatusBody::Title(status)) => {
                    names.path(&spec.title_id, &status.path, 0);
                    for file in &status.files {
                        names.path(&spec.title_id, &file.path, 0);
                    }
                }
                (Spec::Hold(spec), StatusBody::Hold(status))
                    if Some(status.list_generation) == generation =>
                {
                    if let Some(placement) = &status.placement {
                        names.placement(&spec.title_id, placement, 1);
                    }
                    names.path(&spec.title_id, &status.remote_path, 2);
                    if !status.release.trim().is_empty() {
                        names.insert(&spec.title_id, 4, words(&status.release));
                    }
                }
                (Spec::Job(spec), _) => {
                    names.path(&spec.title_id, &spec.dest_rel, 2);
                    names.path(&spec.title_id, &spec.remote_path, 3);
                }
                (_, StatusBody::RemoteFile(status))
                    if status.parse_ok && Some(status.list_generation) == generation =>
                {
                    names.path(&status.title_id, &status.rel_path, 3);
                }
                _ => {}
            }
        }
        names
    }

    pub(super) fn get(&self, id: &str) -> String {
        self.0
            .get(id)
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| fallback(id))
    }

    fn path(&mut self, id: &str, path: &str, priority: u8) {
        if let Ok((_, placement)) =
            parse_placement(Path::new(path)).or_else(|_| parse_remote(None, Path::new(path)))
        {
            self.placement(id, &placement, priority);
        }
    }

    fn placement(&mut self, id: &str, placement: &Placement, priority: u8) {
        let label = match placement {
            Placement::Movie { title, year, .. } | Placement::Episode { title, year, .. } => {
                format!("{} ({year})", words(title))
            }
            Placement::Track {
                artist,
                album,
                year,
                ..
            } => format!("{} / {} ({year})", words(artist), words(album)),
        };
        self.insert(id, priority, label.clone());
        if let Ok(key) = placement.key_title_id() {
            self.insert(&key.render(), priority, label);
        }
    }

    fn insert(&mut self, id: &str, priority: u8, label: String) {
        let candidate = (priority, sanitize(&label));
        if self.0.get(id).is_none_or(|current| candidate < *current) {
            self.0.insert(id.into(), candidate);
        }
    }
}

pub(super) fn hold_name(status: &mediaops_core::HoldStatus, id: &str, names: &Names) -> String {
    match &status.placement {
        Some(placement) => words(&placement.label().replace(".(", " (")).replace('/', " / "),
        None => names.get(id),
    }
}

fn words(value: &str) -> String {
    sanitize(value)
        .split(['.', '_'])
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn capitalized(value: &str) -> String {
    let text = words(value);
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => text,
    }
}

fn fallback(raw: &str) -> String {
    let Ok(id) = TitleId::parse(raw) else {
        return sanitize(raw);
    };
    if id.source() != TitleSource::Key {
        return id.render();
    }
    match id.kind() {
        TitleKind::Movie | TitleKind::Series => match id.id().rsplit_once('.') {
            Some((title, year)) => format!("{} ({year})", capitalized(title)),
            None => id.render(),
        },
        TitleKind::Album => match id.id().split_once('.') {
            Some((artist, album)) => format!("{} / {}", capitalized(artist), capitalized(album)),
            None => id.render(),
        },
    }
}
