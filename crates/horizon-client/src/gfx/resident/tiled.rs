//! Test-only incrementally maintained coarse column tile index (4×4/8×8/16×16).
//!
//! This deliberately leaves the production resident-column walker unchanged
//! until insertion/removal costs, visible parity and end-to-end performance
//! are established across real-world streaming workloads.
use super::{Column, Frustum, Resident, SectionKey, Visible, camera_column, in_frustum, origin};
use crate::{fast_hash::FxHashMap, streaming::in_view_distance};
use glam::{DVec3, Vec3};

#[derive(Debug)]
struct Tile {
    key: (i32, i32),
    columns: Vec<u32>,
    /// Tight bounds in section coordinates; inclusive in each dimension.
    min: (i32, i32, i32),
    max: (i32, i32, i32),
}

impl Tile {
    fn from_column(key: (i32, i32), index: u32, c: &Column) -> Self {
        Self {
            key,
            columns: vec![index],
            min: (c.x, c.low, c.z),
            max: (c.x, c.high, c.z),
        }
    }
    fn include(&mut self, c: &Column) {
        self.min.0 = self.min.0.min(c.x);
        self.min.1 = self.min.1.min(c.low);
        self.min.2 = self.min.2.min(c.z);
        self.max.0 = self.max.0.max(c.x);
        self.max.1 = self.max.1.max(c.high);
        self.max.2 = self.max.2.max(c.z);
    }
    fn recompute(&mut self, columns: &[Column]) {
        let &first = self.columns.first().expect("nonempty resident tile");
        let c = &columns[first as usize];
        self.min = (c.x, c.low, c.z);
        self.max = (c.x, c.high, c.z);
        for pos in 1..self.columns.len() {
            let idx = self.columns[pos];
            self.include(&columns[idx as usize]);
        }
    }
}

#[derive(Debug)]
pub(super) struct TileIndex {
    /// Fixed at construction so tile-key identity cannot change mid-stream.
    width: i32,
    tiles: Vec<Tile>,
    lookup: FxHashMap<(i32, i32), u32>,
}

impl TileIndex {
    fn tile_key(&self, x: i32, z: i32) -> (i32, i32) {
        (x.div_euclid(self.width), z.div_euclid(self.width))
    }
    pub(super) fn new(width: i32) -> Self {
        assert!(
            matches!(width, 4 | 8 | 16),
            "unsupported tile width {width}"
        );
        Self {
            width,
            tiles: Vec::new(),
            lookup: FxHashMap::default(),
        }
    }
    pub(super) fn clear(&mut self) {
        self.tiles.clear();
        self.lookup.clear();
    }
    pub(super) fn width(&self) -> i32 {
        self.width
    }
    pub(super) fn from_columns(columns: &[Column], width: i32) -> Self {
        let mut t = Self::new(width);
        for (idx, c) in columns.iter().enumerate() {
            t.add_column(c, idx as u32);
        }
        t
    }
    pub(super) fn add_column(&mut self, c: &Column, index: u32) {
        let key = self.tile_key(c.x, c.z);
        if let Some(&slot) = self.lookup.get(&key) {
            let t = &mut self.tiles[slot as usize];
            debug_assert!(!t.columns.contains(&index));
            t.columns.push(index);
            t.include(c);
        } else {
            let slot = self.tiles.len() as u32;
            self.tiles.push(Tile::from_column(key, index, c));
            self.lookup.insert(key, slot);
        }
    }
    pub(super) fn expanded(&mut self, c: &Column) {
        let key = self.tile_key(c.x, c.z);
        self.tiles[self.lookup[&key] as usize].include(c);
    }
    /// Called after the owning column bounds shrink. Tight tiles only need a
    /// rescan when that column used to supply the tile's extreme Y bound.
    pub(super) fn shrunk(&mut self, c: &Column, old_low: i32, old_high: i32, columns: &[Column]) {
        let ti = self.lookup[&self.tile_key(c.x, c.z)] as usize;
        let t = &self.tiles[ti];
        if (c.low != old_low && old_low == t.min.1) || (c.high != old_high && old_high == t.max.1) {
            self.tiles[ti].recompute(columns);
        }
    }
    /// Columns have ALREADY been swap-removed from `columns`. Remove the old
    /// member, fix the unrelated last column that moved into its index, and
    /// only then recompute bounds, so stale indices are never dereferenced.
    pub(super) fn removed_column(
        &mut self,
        removed: (i32, i32),
        removed_index: u32,
        moved: Option<((i32, i32), u32)>,
        columns: &[Column],
    ) {
        let key = self.tile_key(removed.0, removed.1);
        let ti = self.lookup[&key] as usize;
        let members = &mut self.tiles[ti].columns;
        let at = members
            .iter()
            .position(|&i| i == removed_index)
            .expect("column in tile");
        members.swap_remove(at);
        if members.is_empty() {
            self.lookup.remove(&key);
            self.tiles.swap_remove(ti);
            if let Some(swapped) = self.tiles.get(ti) {
                self.lookup.insert(swapped.key, ti as u32);
            }
        }
        if let Some(((x, z), previous_last_index)) = moved {
            let moved_ti = self.lookup[&self.tile_key(x, z)] as usize;
            let member = self.tiles[moved_ti]
                .columns
                .iter_mut()
                .find(|i| **i == previous_last_index)
                .expect("swapped column in tile");
            *member = removed_index;
        }
        if let Some(&slot) = self.lookup.get(&key) {
            self.tiles[slot as usize].recompute(columns);
        }
        // No need to recompute the moved column's tile; it owns exactly the
        // same column with the same bounds, only its index changed.
    }
    pub(super) fn visible<T>(
        &self,
        resident: &Resident<T>,
        camera: DVec3,
        radius: i32,
        frustum: &Frustum,
        reachable: impl Fn(SectionKey) -> bool,
        beyond: impl Fn(i32, i32) -> bool,
        out: &mut Vec<Visible>,
    ) {
        let from = camera_column(camera);
        let mask_columns = frustum.masks_help_for_view();
        for tile in &self.tiles {
            let min = origin(tile.min, camera).as_vec3();
            let max = origin(tile.max, camera).as_vec3() + Vec3::splat(16.0);
            // Conservative 1-block expansion avoids camera-relative rounding
            // at a tile edge. Surviving columns use their exact original box.
            if !frustum.intersects(min - Vec3::splat(1.0), max + Vec3::splat(1.0)) {
                continue;
            }
            for &ci in &tile.columns {
                let c = &resident.columns[ci as usize];
                let near = in_view_distance(from, radius, c.x, c.z);
                if !near && !beyond(c.x, c.z) {
                    continue;
                }
                let min = origin((c.x, c.low, c.z), camera).as_vec3();
                let max = origin((c.x, c.high, c.z), camera).as_vec3() + Vec3::splat(16.0);
                let Some(mask) = (if mask_columns {
                    frustum.column_clip_mask(min, max)
                } else {
                    frustum.intersects(min, max).then_some(0b11_1111)
                }) else {
                    continue;
                };
                for &(y, slot) in &c.sections {
                    let key = (c.x, y, c.z);
                    if near && !reachable(key) {
                        continue;
                    }
                    let visible = if mask == 0 {
                        let rel = origin(key, camera);
                        Some(((rel + DVec3::splat(8.0)).length_squared(), rel.as_vec3()))
                    } else if mask == 0b11_1111 {
                        in_frustum(key, camera, frustum, false)
                    } else {
                        let rel = origin(key, camera);
                        let pos = rel.as_vec3();
                        frustum
                            .intersects_masked(pos, pos + Vec3::splat(16.0), mask)
                            .then(|| ((rel + DVec3::splat(8.0)).length_squared(), pos))
                    };
                    if let Some((distance, rel)) = visible {
                        out.push(Visible {
                            distance: distance.to_bits(),
                            key,
                            slot,
                            origin: rel.to_array(),
                        });
                    }
                }
            }
        }
        out.sort_unstable_by_key(|v| (v.distance, v.key));
    }
    pub(super) fn validate(&self, columns: &[Column]) {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        assert_eq!(self.lookup.len(), self.tiles.len());
        for (tile_idx, tile) in self.tiles.iter().enumerate() {
            assert!(!tile.columns.is_empty());
            assert_eq!(self.lookup[&tile.key], tile_idx as u32);
            assert!(tile.columns.len() <= (self.width * self.width) as usize);
            let mut expected = Tile::from_column(
                tile.key,
                tile.columns[0],
                &columns[tile.columns[0] as usize],
            );
            for &index in &tile.columns {
                assert!(
                    seen.insert(index),
                    "duplicate or stale tile column index {index}"
                );
                let col = &columns[index as usize];
                assert_eq!(self.tile_key(col.x, col.z), tile.key);
                expected.include(col);
            }
            assert_eq!(tile.min, expected.min);
            assert_eq!(tile.max, expected.max);
        }
        assert_eq!(seen.len(), columns.len());
    }
    pub(super) fn tile_count(&self) -> usize {
        self.tiles.len()
    }
    /// Lower-bound size estimate for this index's explicit storage. Excludes
    /// the hash table allocator/control bytes and heap fragmentation.
    pub(super) fn known_allocated_bytes(&self) -> usize {
        self.tiles.capacity() * std::mem::size_of::<Tile>()
            + self
                .tiles
                .iter()
                .map(|t| t.columns.capacity() * std::mem::size_of::<u32>())
                .sum::<usize>()
            + self.lookup.capacity()
                * (std::mem::size_of::<(i32, i32)>() + std::mem::size_of::<u32>())
    }
}
