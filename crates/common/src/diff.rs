//! # Generic Collection Difference Module
//!
//! Provides set-based collection comparison to detect added, modified, deleted, and unchanged entries.
//!
//! ## Usage Example
//! ```rust
//! use common::diff::{diff_collections, DiffResult};
//!
//! #[derive(Clone, PartialEq, Debug)]
//! struct Item { id: &'static str, value: i32 }
//!
//! let old_items = vec![Item { id: "a", value: 1 }, Item { id: "b", value: 2 }];
//! let new_items = vec![Item { id: "b", value: 99 }, Item { id: "c", value: 3 }];
//!
//! let diff = diff_collections(&old_items, &new_items, |item| item.id, |item| item.value);
//!
//! assert_eq!(diff.added.len(), 1);
//! assert_eq!(diff.added[0].id, "c");
//! assert_eq!(diff.deleted.len(), 1);
//! assert_eq!(diff.deleted[0].id, "a");
//! assert_eq!(diff.modified.len(), 1);
//! assert_eq!(diff.modified[0].0.value, 2);
//! assert_eq!(diff.modified[0].1.value, 99);
//! ```

use std::collections::HashMap;
use std::hash::Hash;

/// The structured result of comparing two keyed collections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffResult<T> {
    /// Items present in `new` but absent in `old`.
    pub added: Vec<T>,
    /// Items present in both whose compared value differs: `(old_item, new_item)`.
    pub modified: Vec<(T, T)>,
    /// Items present in `old` but absent in `new`.
    pub deleted: Vec<T>,
    /// Items present in both with identical compared values.
    pub unchanged: Vec<T>,
}

/// Compute the set differences between two collections `a` (old) and `b` (new).
///
/// # Arguments
/// * `a` — Slice of initial/old collection items.
/// * `b` — Slice of updated/new collection items.
/// * `key_fn` — Key extractor mapping an item to its unique identifier.
/// * `val_fn` — Value extractor mapping an item to its comparable content (e.g. hash or version).
pub fn diff_collections<'a, K, V, T, FK, FV>(
    a: &'a [T],
    b: &'a [T],
    key_fn: FK,
    val_fn: FV,
) -> DiffResult<T>
where
    K: Eq + Hash + 'a,
    V: PartialEq + 'a,
    T: Clone,
    FK: Fn(&'a T) -> K,
    FV: Fn(&'a T) -> V,
{
    let mut map_a: HashMap<K, (usize, V)> = HashMap::with_capacity(a.len());
    for (idx, item) in a.iter().enumerate() {
        map_a.insert(key_fn(item), (idx, val_fn(item)));
    }

    let mut map_b: HashMap<K, (usize, V)> = HashMap::with_capacity(b.len());
    for (idx, item) in b.iter().enumerate() {
        map_b.insert(key_fn(item), (idx, val_fn(item)));
    }

    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();
    let mut unchanged = Vec::new();

    // Check items in b (added, modified, unchanged)
    for item in b {
        let key = key_fn(item);
        let val_b = val_fn(item);

        match map_a.get(&key) {
            None => added.push(item.clone()),
            Some(&(idx_a, ref val_a)) => {
                if *val_a != val_b {
                    modified.push((a[idx_a].clone(), item.clone()));
                } else {
                    unchanged.push(item.clone());
                }
            }
        }
    }

    // Check items in a that were deleted in b
    for item in a {
        let key = key_fn(item);
        if !map_b.contains_key(&key) {
            deleted.push(item.clone());
        }
    }

    DiffResult {
        added,
        modified,
        deleted,
        unchanged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_collections() {
        let old_list = vec![("k1", "v1"), ("k2", "v2"), ("k3", "v3")];
        let new_list = vec![("k2", "v2_updated"), ("k3", "v3"), ("k4", "v4")];

        let res = diff_collections(
            &old_list,
            &new_list,
            |item| item.0,
            |item| item.1,
        );

        assert_eq!(res.added, vec![("k4", "v4")]);
        assert_eq!(res.deleted, vec![("k1", "v1")]);
        assert_eq!(res.modified, vec![(("k2", "v2"), ("k2", "v2_updated"))]);
        assert_eq!(res.unchanged, vec![("k3", "v3")]);
    }
}
