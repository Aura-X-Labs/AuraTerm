//! Three-way merge for configuration sync (design
//! `docs/plans/sync-merge-rules-design.md` §3).
//!
//! Every device keeps a **base**: a snapshot of what it and the cloud copy
//! agreed on at the end of its last sync. Comparing the local state and the
//! cloud copy against that base tells who changed what:
//!
//! - differs from the base on one side only → that side's change is kept;
//! - in the base but gone on one side → deleted there, so deleted everywhere —
//!   unless the other side edited it meanwhile (**an edit beats a delete**);
//! - changed differently on both sides → a real conflict: **the cloud copy
//!   wins** and the key is reported. The first device to upload sets the cloud
//!   copy and everyone after it yields to that, so all devices converge.
//!
//! Without a base (first sync on a device, or a cloud copy of another lineage —
//! see `cloud_sync`) nothing can be inferred, so the merge falls back to the
//! union with the cloud copy winning by key, and never deletes.
//!
//! Pure functions only: no I/O, no clock.

use crate::connections::SavedConnection;
use crate::encryption::StoredCredential;

use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// A keyed record the merge can compare.
pub(crate) trait Record: Clone {
    fn key(&self) -> &str;
    /// Whether two copies say the same thing. Fields that are not user edits
    /// (a bookmark's `last_used`) do not count.
    fn same_as(&self, other: &Self) -> bool;
    /// Fold those uncompared fields of the other copy into this one.
    fn absorb(&mut self, _other: &Self) {}
}

impl Record for SavedConnection {
    fn key(&self) -> &str {
        &self.id
    }

    fn same_as(&self, other: &Self) -> bool {
        fn content(item: &SavedConnection) -> Option<Value> {
            let mut value = serde_json::to_value(item).ok()?;
            value.as_object_mut()?.remove("lastUsed");
            Some(value)
        }
        matches!((content(self), content(other)), (Some(a), Some(b)) if a == b)
    }

    /// Connecting is not an edit; the most recent use on either side stands.
    fn absorb(&mut self, other: &Self) {
        self.last_used = self.last_used.max(other.last_used);
    }
}

impl Record for StoredCredential {
    fn key(&self) -> &str {
        &self.connection_id
    }

    fn same_as(&self, other: &Self) -> bool {
        self == other
    }
}

/// Deletions the caller decided not to act on yet (the mass-delete guard). A
/// held key stays exactly as it is on both sides.
#[derive(Debug, Default)]
pub(crate) struct Held {
    /// Gone from the cloud copy; kept here and kept out of the upload.
    pub(crate) remote_deletes: HashSet<String>,
    /// Deleted here; the cloud copy's entry stays in the upload.
    pub(crate) local_deletes: HashSet<String>,
}

impl Held {
    pub(crate) fn is_empty(&self) -> bool {
        self.remote_deletes.is_empty() && self.local_deletes.is_empty()
    }
}

#[derive(Debug)]
pub(crate) struct Merged<T> {
    /// The new local state: local order, then what the cloud copy added.
    pub(crate) local: Vec<T>,
    /// The new cloud state. Differs from `local` only by held deletions.
    pub(crate) upload: Vec<T>,
    /// Taken from the cloud copy, new here.
    pub(crate) added: usize,
    /// Replaced here by the cloud copy's version.
    pub(crate) updated: usize,
    /// Keys removed here because the cloud copy dropped them.
    pub(crate) removed: Vec<String>,
    /// Keys deleted here that the upload drops from the cloud copy.
    pub(crate) dropped: Vec<String>,
    /// Keys both sides changed differently; the cloud copy won.
    pub(crate) conflicts: Vec<String>,
}

/// Merge `local` and `remote` against `base` (`None`: nothing to compare with,
/// so union with the cloud copy winning, and no deletes).
pub(crate) fn three_way<T: Record>(base: Option<&[T]>, local: Vec<T>, remote: Vec<T>, held: &Held) -> Merged<T> {
    let base_by_key: Option<HashMap<&str, &T>> = base.map(|items| items.iter().map(|item| (item.key(), item)).collect());
    let mut remote_by_key: HashMap<String, T> = remote.iter().map(|item| (item.key().to_string(), item.clone())).collect();
    let mut out = Merged { local: Vec::new(), upload: Vec::new(), added: 0, updated: 0, removed: Vec::new(), dropped: Vec::new(), conflicts: Vec::new() };

    for mine in local {
        let key = mine.key().to_string();
        let theirs = remote_by_key.remove(&key);
        if held.remote_deletes.contains(&key) {
            out.local.push(mine);
            continue;
        }
        let before = base_by_key.as_ref().and_then(|map| map.get(key.as_str()).copied());
        match theirs {
            Some(mut theirs) if mine.same_as(&theirs) => {
                theirs.absorb(&mine);
                out.local.push(theirs.clone());
                out.upload.push(theirs);
            }
            Some(mut theirs) => {
                // Without a base the cloud copy simply wins; with one, it wins
                // unless it is the side that did not change.
                let (mine_changed, theirs_changed) = match (&base_by_key, before) {
                    (None, _) => (false, true),
                    (Some(_), None) => (true, true),
                    (Some(_), Some(before)) => (!mine.same_as(before), !theirs.same_as(before)),
                };
                if theirs_changed {
                    if mine_changed {
                        out.conflicts.push(key);
                    }
                    out.updated += 1;
                    theirs.absorb(&mine);
                    out.local.push(theirs.clone());
                    out.upload.push(theirs);
                } else {
                    let mut mine = mine;
                    mine.absorb(&theirs);
                    out.local.push(mine.clone());
                    out.upload.push(mine);
                }
            }
            // Gone from the cloud copy: deleted there, unless edited here since.
            None => match before {
                Some(before) if mine.same_as(before) => out.removed.push(key),
                _ => {
                    out.local.push(mine.clone());
                    out.upload.push(mine);
                }
            },
        }
    }

    // What is left of the cloud copy has no local counterpart.
    for theirs in remote {
        let key = theirs.key();
        if !remote_by_key.contains_key(key) {
            continue;
        }
        if held.local_deletes.contains(key) {
            out.upload.push(theirs);
            continue;
        }
        let before = base_by_key.as_ref().and_then(|map| map.get(key).copied());
        match before {
            // Deleted here, untouched there.
            Some(before) if theirs.same_as(before) => out.dropped.push(key.to_string()),
            // New there, or edited there after the delete here.
            _ => {
                out.added += 1;
                out.local.push(theirs.clone());
                out.upload.push(theirs);
            }
        }
    }
    out
}

#[derive(Debug, Default)]
pub(crate) struct SettingsMerge {
    /// Keys whose local value must change, with the cloud copy's value.
    pub(crate) apply: Map<String, Value>,
    /// Keys both sides changed differently; the cloud copy won.
    pub(crate) conflicts: Vec<String>,
}

/// Merge the synced settings key by key. A key missing from the cloud copy is
/// "no opinion" (an older build does not sync it), never a delete.
pub(crate) fn merge_settings(base: Option<&Value>, local: &Value, remote: &Value) -> SettingsMerge {
    let mut out = SettingsMerge::default();
    let (Some(local), Some(remote)) = (local.as_object(), remote.as_object()) else {
        return out;
    };
    let base = base.map(|value| value.as_object());
    for (key, theirs) in remote {
        let mine = local.get(key);
        if mine == Some(theirs) {
            continue;
        }
        let (mine_changed, theirs_changed) = match base {
            None => (false, true),
            Some(base) => {
                let before = base.and_then(|map| map.get(key));
                (mine != before, Some(theirs) != before)
            }
        };
        if theirs_changed {
            if mine_changed {
                out.conflicts.push(key.clone());
            }
            out.apply.insert(key.clone(), theirs.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_sync::tests::bookmark;
    use serde_json::json;

    fn names(items: &[SavedConnection]) -> Vec<&str> {
        items.iter().map(|c| c.name.as_str()).collect()
    }

    fn merge(base: &[SavedConnection], local: Vec<SavedConnection>, remote: Vec<SavedConnection>) -> Merged<SavedConnection> {
        three_way(Some(base), local, remote, &Held::default())
    }

    #[test]
    fn additions_on_either_side_are_kept() {
        let out = merge(&[], vec![bookmark("a", "mine")], vec![bookmark("b", "theirs")]);
        assert_eq!(names(&out.local), ["mine", "theirs"]);
        assert_eq!(names(&out.upload), ["mine", "theirs"]);
        assert_eq!((out.added, out.updated), (1, 0));
        assert!(out.removed.is_empty() && out.dropped.is_empty() && out.conflicts.is_empty());
    }

    #[test]
    fn untouched_entries_stay_as_they_are() {
        let base = [bookmark("a", "same")];
        let out = merge(&base, vec![bookmark("a", "same")], vec![bookmark("a", "same")]);
        assert_eq!(names(&out.local), ["same"]);
        assert_eq!((out.added, out.updated), (0, 0));
    }

    #[test]
    fn a_change_on_one_side_wins_over_the_unchanged_side() {
        let base = [bookmark("a", "old"), bookmark("b", "old")];
        let out = merge(&base, vec![bookmark("a", "mine"), bookmark("b", "old")], vec![bookmark("a", "old"), bookmark("b", "theirs")]);
        assert_eq!(names(&out.local), ["mine", "theirs"]);
        assert_eq!(names(&out.upload), ["mine", "theirs"]);
        assert_eq!(out.updated, 1, "only the entry the cloud copy changed counts as updated here");
        assert!(out.conflicts.is_empty());
    }

    #[test]
    fn the_same_change_on_both_sides_is_not_a_conflict() {
        let base = [bookmark("a", "old")];
        let out = merge(&base, vec![bookmark("a", "new")], vec![bookmark("a", "new")]);
        assert_eq!(names(&out.local), ["new"]);
        assert!(out.conflicts.is_empty());
        assert_eq!(out.updated, 0);
    }

    #[test]
    fn different_changes_on_both_sides_conflict_and_the_cloud_copy_wins() {
        let base = [bookmark("a", "old")];
        let out = merge(&base, vec![bookmark("a", "mine")], vec![bookmark("a", "theirs")]);
        assert_eq!(names(&out.local), ["theirs"]);
        assert_eq!(out.conflicts, ["a"]);
        assert_eq!(out.updated, 1);
    }

    #[test]
    fn the_same_key_added_differently_on_both_sides_conflicts() {
        let out = merge(&[], vec![bookmark("a", "mine")], vec![bookmark("a", "theirs")]);
        assert_eq!(names(&out.local), ["theirs"]);
        assert_eq!(out.conflicts, ["a"]);
    }

    #[test]
    fn a_local_delete_is_dropped_from_the_upload() {
        let base = [bookmark("a", "keep"), bookmark("b", "gone")];
        let out = merge(&base, vec![bookmark("a", "keep")], vec![bookmark("a", "keep"), bookmark("b", "gone")]);
        assert_eq!(names(&out.local), ["keep"]);
        assert_eq!(names(&out.upload), ["keep"]);
        assert_eq!(out.dropped, ["b"]);
        assert_eq!(out.added, 0);
    }

    #[test]
    fn a_remote_delete_is_removed_here() {
        let base = [bookmark("a", "keep"), bookmark("b", "gone")];
        let out = merge(&base, vec![bookmark("a", "keep"), bookmark("b", "gone")], vec![bookmark("a", "keep")]);
        assert_eq!(names(&out.local), ["keep"]);
        assert_eq!(out.removed, ["b"]);
    }

    #[test]
    fn an_edit_beats_a_delete_in_both_directions() {
        let base = [bookmark("a", "old"), bookmark("b", "old")];
        // a: deleted here, edited there. b: edited here, deleted there.
        let out = merge(&base, vec![bookmark("b", "edited-here")], vec![bookmark("a", "edited-there")]);
        assert_eq!(names(&out.local), ["edited-here", "edited-there"]);
        assert_eq!(names(&out.upload), ["edited-here", "edited-there"]);
        assert!(out.removed.is_empty() && out.dropped.is_empty());
        assert_eq!(out.added, 1, "the entry deleted here comes back");
    }

    #[test]
    fn deleted_on_both_sides_is_simply_gone() {
        let base = [bookmark("a", "gone")];
        let out = merge(&base, vec![], vec![]);
        assert!(out.local.is_empty() && out.upload.is_empty());
        assert!(out.removed.is_empty() && out.dropped.is_empty());
    }

    #[test]
    fn without_a_base_it_is_a_union_where_the_cloud_copy_wins_and_nothing_is_deleted() {
        let local = vec![bookmark("a", "mine"), bookmark("b", "only-here")];
        let remote = vec![bookmark("a", "theirs"), bookmark("c", "only-there")];
        let out = three_way(None, local, remote, &Held::default());
        assert_eq!(names(&out.local), ["theirs", "only-here", "only-there"]);
        assert_eq!((out.added, out.updated), (1, 1));
        assert!(out.conflicts.is_empty(), "without a base a difference is not known to be a conflict");
        assert!(out.removed.is_empty() && out.dropped.is_empty());
    }

    #[test]
    fn last_used_is_not_an_edit_and_the_latest_use_stands() {
        let used = |name: &str, at: Option<u64>| {
            let mut item = bookmark("a", name);
            item.last_used = at;
            item
        };
        let base = [used("old", Some(1))];

        // Connecting here must not shield the entry from the cloud copy's edit…
        let out = merge(&base, vec![used("old", Some(50))], vec![used("renamed", Some(9))]);
        assert_eq!(names(&out.local), ["renamed"]);
        assert_eq!(out.local[0].last_used, Some(50));
        assert_eq!(out.upload[0].last_used, Some(50));
        assert!(out.conflicts.is_empty());

        // …nor make a delete there look like "edited here".
        let out = merge(&base, vec![used("old", Some(50))], vec![]);
        assert_eq!(out.removed, ["a"]);
    }

    #[test]
    fn held_deletes_leave_both_sides_as_they_are() {
        let base = [bookmark("a", "gone-there"), bookmark("b", "gone-here"), bookmark("c", "stays")];
        let local = vec![bookmark("a", "gone-there"), bookmark("c", "stays")];
        let remote = vec![bookmark("b", "gone-here"), bookmark("c", "stays")];
        let held = Held { remote_deletes: HashSet::from(["a".to_string()]), local_deletes: HashSet::from(["b".to_string()]) };

        let out = three_way(Some(&base), local, remote, &held);

        assert_eq!(names(&out.local), ["gone-there", "stays"], "kept here, still deleted here");
        assert_eq!(names(&out.upload), ["stays", "gone-here"], "kept out of the upload, still in the upload");
        assert!(out.removed.is_empty() && out.dropped.is_empty());
        assert_eq!(out.added, 0);
    }

    #[test]
    fn credentials_merge_by_connection_id() {
        let cred = |id: &str, password: &str| {
            let mut credential = StoredCredential::default();
            credential.connection_id = id.to_string();
            credential.password = Some(password.to_string());
            credential
        };
        let base = [cred("a", "old"), cred("b", "old"), cred("c", "old")];
        let local = vec![cred("a", "changed-here"), cred("b", "old"), cred("c", "old")];
        let remote = vec![cred("a", "old"), cred("b", "changed-there")];

        let out = three_way(Some(&base), local, remote, &Held::default());

        let passwords: Vec<_> = out.local.iter().map(|c| c.password.clone().unwrap_or_default()).collect();
        assert_eq!(passwords, ["changed-here", "changed-there"]);
        assert_eq!(out.removed, ["c"]);
    }

    #[test]
    fn settings_merge_key_by_key() {
        let base = json!({"theme": "dark", "fontSize": 13, "scrollback": 1000, "fontFamily": "Menlo"});
        let local = json!({"theme": "light", "fontSize": 13, "scrollback": 5000, "fontFamily": "Menlo"});
        let remote = json!({"theme": "dark", "fontSize": 16, "scrollback": 9000});

        let out = merge_settings(Some(&base), &local, &remote);

        assert_eq!(Value::Object(out.apply), json!({"fontSize": 16, "scrollback": 9000}), "theme changed here only; fontFamily is missing there, not deleted");
        assert_eq!(out.conflicts, ["scrollback"]);
    }

    #[test]
    fn settings_without_a_base_take_the_cloud_copy() {
        let out = merge_settings(None, &json!({"theme": "light", "fontSize": 13}), &json!({"theme": "dark", "fontSize": 13, "new": true}));
        assert_eq!(Value::Object(out.apply), json!({"theme": "dark", "new": true}));
        assert!(out.conflicts.is_empty());
    }
}
