//! **Not interested**: the suggestion ids each profile dismissed, kept on the television so a
//! dismissed idea never comes back. A small JSON map of profile key → ids, written beside the
//! session (`plx_base::paths::vchannel_dismissed_candidates`) through the storage worker and
//! removed by Delete all data with the app's other files.

use std::collections::BTreeMap;

/// Ids remembered per profile at most (oldest forgotten first).
const PER_PROFILE_MAX: usize = 400;

pub type Map = BTreeMap<String, Vec<String>>;

pub fn parse(raw: &str) -> Map {
    serde_json::from_str(raw).unwrap_or_default()
}

/// The stored map, from the first candidate that reads.
pub fn load() -> Map {
    plx_base::paths::vchannel_dismissed_candidates()
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|raw| parse(&raw))
        .unwrap_or_default()
}

/// Add `id` to `profile`'s list; `true` when it was new.
pub fn add(map: &mut Map, profile: &str, id: &str) -> bool {
    let list = map.entry(profile.to_owned()).or_default();
    if list.iter().any(|x| x == id) {
        return false;
    }
    list.push(id.to_owned());
    if list.len() > PER_PROFILE_MAX {
        let extra = list.len() - PER_PROFILE_MAX;
        list.drain(..extra);
    }
    true
}

/// Write the map on the storage worker (tmp + rename on the first writable candidate).
pub fn save(map: &Map) {
    if crate::stores::tape::active() || cfg!(test) {
        return;
    }
    let body = serde_json::to_string(map).unwrap_or_default();
    let _ = plx_base::storage_worker::submit_retained(move || {
        for path in plx_base::paths::vchannel_dismissed_candidates() {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, &body).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_kept_per_profile_once_and_bounded() {
        let mut m = Map::new();
        assert!(add(&mut m, "p1", "a"));
        assert!(!add(&mut m, "p1", "a"));
        assert!(add(&mut m, "p2", "a"));
        for i in 0..PER_PROFILE_MAX + 5 {
            add(&mut m, "p1", &format!("x{i}"));
        }
        assert_eq!(m["p1"].len(), PER_PROFILE_MAX);
        assert!(!m["p1"].contains(&"a".to_owned()), "the oldest went first");
        let round = parse(&serde_json::to_string(&m).unwrap());
        assert_eq!(round, m);
        assert!(parse("not json").is_empty());
    }
}
