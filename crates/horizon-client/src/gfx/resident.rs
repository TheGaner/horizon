//! Resident sections, stored densely and grouped by chunk column for the
//! per-frame culling walk: a column whose box misses the frustum or lies past
//! the render distance rejects all of its sections with one test, and the
//! sections that remain are reached by index rather than by hashing keys.
use super::{Frustum, SectionKey};
use crate::{fast_hash::FxHashMap, streaming::in_view_distance};
use glam::{DVec3, Vec3};

#[cfg(test)]
mod tile_tests;
#[cfg(test)]
mod tiled;

/// One chunk column's resident sections.
struct Column {
    x: i32,
    z: i32,
    /// Section y (from the world's bottom) and slot, in no particular order.
    sections: Vec<(i32, u32)>,
    /// The lowest and highest section y of `sections`.
    low: i32,
    high: i32,
}

impl Column {
    fn bounds(&mut self) {
        self.low = self.sections.iter().map(|s| s.0).min().unwrap_or(0);
        self.high = self.sections.iter().map(|s| s.0).max().unwrap_or(0);
    }
}

/// Values of type `T` per section key, in a dense list with stable keys and
/// slots that move only when a section is removed.
pub struct Resident<T> {
    items: Vec<(SectionKey, T)>,
    slots: FxHashMap<SectionKey, u32>,
    columns: Vec<Column>,
    column_index: FxHashMap<(i32, i32), u32>,
    #[cfg(test)]
    tiled_index: Option<tiled::TileIndex>,
}

impl<T> Default for Resident<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            slots: FxHashMap::default(),
            columns: Vec::new(),
            column_index: FxHashMap::default(),
            #[cfg(test)]
            tiled_index: None,
        }
    }
}

impl<T> Resident<T> {
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn contains(&self, key: SectionKey) -> bool {
        self.slots.contains_key(&key)
    }
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.items.iter().map(|(_, v)| v)
    }
    /// The section in `slot` (`Visible::slot`).
    pub fn get(&self, slot: u32) -> &T {
        &self.items[slot as usize].1
    }
    pub fn get_mut(&mut self, slot: u32) -> &mut T {
        &mut self.items[slot as usize].1
    }
    pub fn clear(&mut self) {
        self.items.clear();
        self.slots.clear();
        self.columns.clear();
        self.column_index.clear();
        #[cfg(test)]
        if let Some(index) = &mut self.tiled_index {
            *index = tiled::TileIndex::default();
        }
    }
    /// Adds or replaces `key`'s value.
    pub fn insert(&mut self, key: SectionKey, value: T) {
        if let Some(&slot) = self.slots.get(&key) {
            self.items[slot as usize].1 = value;
            return;
        }
        let slot = self.items.len() as u32;
        self.items.push((key, value));
        self.slots.insert(key, slot);
        #[cfg(test)]
        let new_column = !self.column_index.contains_key(&(key.0, key.2));
        let column = match self.column_index.get(&(key.0, key.2)) {
            Some(&c) => &mut self.columns[c as usize],
            None => {
                self.column_index
                    .insert((key.0, key.2), self.columns.len() as u32);
                self.columns.push(Column {
                    x: key.0,
                    z: key.2,
                    sections: Vec::new(),
                    low: key.1,
                    high: key.1,
                });
                self.columns.last_mut().expect("just pushed")
            }
        };
        column.sections.push((key.1, slot));
        column.low = column.low.min(key.1);
        column.high = column.high.max(key.1);
        #[cfg(test)]
        if let Some(index) = &mut self.tiled_index {
            let col = &self.columns[self.column_index[&(key.0, key.2)] as usize];
            if new_column {
                index.add_column(col, self.column_index[&(key.0, key.2)]);
            } else {
                index.expanded(col);
            }
        }
    }
    pub fn remove(&mut self, key: SectionKey) -> Option<T> {
        let slot = self.slots.remove(&key)?;
        let c = self.column_index[&(key.0, key.2)] as usize;
        let column = &mut self.columns[c];
        #[cfg(test)]
        let (old_low, old_high) = (column.low, column.high);
        let at = column
            .sections
            .iter()
            .position(|s| s.1 == slot)
            .expect("section in its column");
        column.sections.swap_remove(at);
        if column.sections.is_empty() {
            #[cfg(test)]
            let previous_last = self.columns.len() as u32 - 1;
            self.column_index.remove(&(key.0, key.2));
            self.columns.swap_remove(c);
            if let Some(moved) = self.columns.get(c) {
                self.column_index.insert((moved.x, moved.z), c as u32);
            }
            #[cfg(test)]
            if let Some(index) = &mut self.tiled_index {
                let moved = self
                    .columns
                    .get(c)
                    .map(|col| ((col.x, col.z), previous_last));
                index.removed_column((key.0, key.2), c as u32, moved, &self.columns);
            }
        } else {
            column.bounds();
            #[cfg(test)]
            if let Some(index) = &mut self.tiled_index {
                index.shrunk(&self.columns[c], old_low, old_high, &self.columns);
            }
        }
        let (_, value) = self.items.swap_remove(slot as usize);
        if let Some(&(moved, _)) = self.items.get(slot as usize) {
            let last = self.items.len() as u32;
            self.slots.insert(moved, slot);
            let column = &mut self.columns[self.column_index[&(moved.0, moved.2)] as usize];
            let entry = column
                .sections
                .iter_mut()
                .find(|s| s.1 == last)
                .expect("moved section in its column");
            entry.1 = slot;
        }
        Some(value)
    }
}

/// A section in view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visible {
    /// Its centre's squared distance from the camera, as bits: non-negative
    /// floats order like their bits, so sorting compares integers.
    pub distance: u64,
    pub key: SectionKey,
    pub slot: u32,
    /// Its camera-relative origin.
    pub origin: [f32; 3],
}

/// The camera-relative origin of the section at `key`.
fn origin(key: SectionKey, camera: DVec3) -> DVec3 {
    DVec3::new(
        f64::from(key.0 * 16),
        f64::from(crate::chunk::MIN_Y + key.1 * 16),
        f64::from(key.2 * 16),
    ) - camera
}

/// The camera's chunk column.
fn camera_column(camera: DVec3) -> (i32, i32) {
    (
        (camera.x.floor() as i32) >> 4,
        (camera.z.floor() as i32) >> 4,
    )
}

/// Whether the section at `key` is drawn: its column within the render
/// distance of the camera's (`SectionOcclusionGraph.isInViewDistance`), and
/// partly inside the frustum. `None` when it is not, else its squared centre
/// distance and origin. `Resident::visible` walks columns instead.
#[cfg(test)]
fn in_view(key: SectionKey, camera: DVec3, radius: i32, frustum: &Frustum) -> Option<(f64, Vec3)> {
    if !in_view_distance(camera_column(camera), radius, key.0, key.2) {
        return None;
    }
    in_frustum(key, camera, frustum)
}

/// `in_view` without the distance test.
fn in_frustum(key: SectionKey, camera: DVec3, frustum: &Frustum) -> Option<(f64, Vec3)> {
    let origin = origin(key, camera);
    let min = origin.as_vec3();
    if !frustum.intersects(min, min + Vec3::splat(16.0)) {
        return None;
    }
    Some(((origin + DVec3::splat(8.0)).length_squared(), min))
}

impl<T> Resident<T> {
    #[cfg(test)]
    fn enable_tiled_index(&mut self) {
        self.tiled_index = Some(tiled::TileIndex::from_columns(&self.columns));
    }
    #[cfg(test)]
    fn visible_tiled(
        &self,
        camera: DVec3,
        radius: i32,
        frustum: &Frustum,
        reachable: impl Fn(SectionKey) -> bool,
        beyond: impl Fn(i32, i32) -> bool,
        out: &mut Vec<Visible>,
    ) {
        self.tiled_index
            .as_ref()
            .expect("enable_tiled_index first")
            .visible(self, camera, radius, frustum, reachable, beyond, out);
    }
    /// Appends the sections `in_view` and `reachable` to `out`, nearest
    /// first (ties by key, so the order never depends on storage), and the
    /// sections in the frustum of columns past the render distance that
    /// `beyond` keeps (distant terrain still leaves them out; the occlusion
    /// graph never reaches them).
    ///
    /// Columns are tested first, with the same distance test (it depends
    /// only on x and z) and a box spanning their sections: every section box
    /// lies inside it and the frustum test rounds monotonically, so a column
    /// the frustum rejects holds no section it would accept. `radius` is the
    /// render distance in chunks.
    pub fn visible(
        &self,
        camera: DVec3,
        radius: i32,
        frustum: &Frustum,
        reachable: impl Fn(SectionKey) -> bool,
        beyond: impl Fn(i32, i32) -> bool,
        out: &mut Vec<Visible>,
    ) {
        let from = camera_column(camera);
        // When the near-plane normal points mostly vertically, projecting
        // tall columns leaves too many active planes for masking to pay off.
        // Keep the exact original per-section path in that camera regime.
        let mask_columns = frustum.masks_help_for_view();
        for column in &self.columns {
            let near = in_view_distance(from, radius, column.x, column.z);
            if !near && !beyond(column.x, column.z) {
                continue;
            }
            let min = origin((column.x, column.low, column.z), camera).as_vec3();
            let top = origin((column.x, column.high, column.z), camera).as_vec3();
            let max = top + Vec3::splat(16.0);
            let Some(clip_mask) = (if mask_columns {
                frustum.column_clip_mask(min, max)
            } else {
                frustum.intersects(min, max).then_some(0b11_1111)
            }) else {
                continue;
            };
            for &(y, slot) in &column.sections {
                let key = (column.x, y, column.z);
                if near && !reachable(key) {
                    continue;
                }
                // Only planes intersecting the column can reject a section;
                // leave all other rules and f64 distance arithmetic intact.
                let visibility = if clip_mask == 0 {
                    let rel = origin(key, camera);
                    Some(((rel + DVec3::splat(8.0)).length_squared(), rel.as_vec3()))
                } else if clip_mask == 0b11_1111 {
                    in_frustum(key, camera, frustum)
                } else {
                    let rel = origin(key, camera);
                    let pos = rel.as_vec3();
                    frustum
                        .intersects_masked(pos, pos + Vec3::splat(16.0), clip_mask)
                        .then(|| ((rel + DVec3::splat(8.0)).length_squared(), pos))
                };
                if let Some((d, o)) = visibility {
                    out.push(Visible {
                        distance: d.to_bits(),
                        key,
                        slot,
                        origin: o.to_array(),
                    });
                }
            }
        }
        out.sort_unstable_by_key(|v| (v.distance, v.key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A normal camera still uses the mask; very steep upward or downward
    /// views use the legacy clip loop regardless of yaw or field of view.
    /// This guard is an optimization decision, not a visibility decision.
    #[test]
    fn adaptive_column_mask_uses_view_direction_not_yaw_or_fov() {
        for yaw in [0.0, 45.0, 135.0, 270.0] {
            for fov in [50.0, 70.0, 90.0] {
                let projection = super::super::world_projection(fov, 16.0 / 9.0, 1100.0);
                for (pitch, expected) in [
                    (0.0, true),
                    (50.0, true),
                    (-50.0, true),
                    (60.0, true),
                    (-60.0, true),
                    (75.0, false),
                    (-75.0, false),
                    (89.0, false),
                    (-89.0, false),
                ] {
                    let view = crate::camera::view_matrix(yaw, pitch);
                    let frustum = Frustum {
                        planes: super::super::frustum(projection * view),
                    };
                    assert_eq!(
                        frustum.masks_help_for_view(),
                        expected,
                        "yaw={yaw} pitch={pitch} fov={fov}"
                    );
                }
            }
        }
    }

    #[test]
    fn column_walk_finds_exactly_the_sections_in_view() {
        let mut r = Resident::default();
        for x in -12i32..=12 {
            for z in -12..=12 {
                for y in 0..24 {
                    // Leave holes so columns have uneven bounds.
                    if (x * 7 + y * 3 + z * 5).rem_euclid(4) != 0 {
                        r.insert((x, y, z), ());
                    }
                }
            }
        }
        let reachable = |k: SectionKey| (k.0 + k.1 + k.2).rem_euclid(9) != 0;
        let cameras = [
            (DVec3::new(0.5, 70.0, 0.5), 0.0, 0.0),
            (DVec3::new(-37.25, 130.5, 21.75), 135.0, 35.0),
            (DVec3::new(100.0, -60.0, -3.0), 270.0, -80.0),
            (DVec3::new(8.0, 64.0, 8.0), 45.0, 89.0),
        ];
        for (camera, yaw, pitch) in cameras {
            for radius in [2, 6, 10] {
                let projection = super::super::world_projection(70.0, 16.0 / 9.0, 512.0);
                let view = crate::camera::view_matrix(yaw, pitch);
                let frustum = Frustum {
                    planes: super::super::frustum(projection * view),
                };
                let mut fast = Vec::new();
                // Columns past the distance kept for distant terrain.
                let beyond = |x: i32, z: i32| (x + z).rem_euclid(3) == 0;
                r.visible(camera, radius, &frustum, reachable, beyond, &mut fast);
                let mut slow: Vec<(SectionKey, f64, [f32; 3])> = r
                    .items
                    .iter()
                    .filter_map(|&(k, _)| {
                        let near = in_view_distance(camera_column(camera), radius, k.0, k.2);
                        if near {
                            in_view(k, camera, radius, &frustum).filter(|_| reachable(k))
                        } else if beyond(k.0, k.2) {
                            in_frustum(k, camera, &frustum)
                        } else {
                            None
                        }
                        .map(|(d, o)| (k, d, o.to_array()))
                    })
                    .collect();
                slow.sort_unstable_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
                let fast: Vec<_> = fast
                    .iter()
                    .map(|v| {
                        assert_eq!(r.items[v.slot as usize].0, v.key);
                        (v.key, f64::from_bits(v.distance), v.origin)
                    })
                    .collect();
                assert!(!slow.is_empty());
                assert_eq!(fast, slow, "{camera} {yaw} {pitch} {radius}");
            }
        }
    }

    /// Every key reaches its value through its slot and column, and every
    /// column's bounds and sections match the keys.
    fn check(r: &Resident<u32>, model: &HashMap<SectionKey, u32>) {
        assert_eq!(r.len(), model.len());
        let mut seen = 0;
        for column in &r.columns {
            assert!(!column.sections.is_empty());
            let ys: Vec<i32> = column.sections.iter().map(|s| s.0).collect();
            assert_eq!(column.low, *ys.iter().min().unwrap());
            assert_eq!(column.high, *ys.iter().max().unwrap());
            for &(y, slot) in &column.sections {
                let (key, value) = &r.items[slot as usize];
                assert_eq!(*key, (column.x, y, column.z));
                assert_eq!(model[key], *value);
                seen += 1;
            }
        }
        assert_eq!(seen, model.len());
        for key in model.keys() {
            assert!(r.contains(*key));
        }
    }

    #[test]
    fn slots_and_columns_follow_inserts_and_removals() {
        let mut r = Resident::default();
        let mut model = HashMap::new();
        // A deterministic mix of inserts, replacements and removals.
        let mut seed = 0x2545_f491_u32;
        for step in 0..4000u32 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let key = (
                (seed % 5) as i32 - 2,
                ((seed >> 4) % 6) as i32 - 1,
                ((seed >> 8) % 5) as i32 - 2,
            );
            if (seed >> 12).is_multiple_of(3) {
                assert_eq!(r.remove(key), model.remove(&key));
            } else {
                r.insert(key, step);
                model.insert(key, step);
            }
            check(&r, &model);
        }
        r.clear();
        assert_eq!(r.len(), 0);
        assert!(r.columns.is_empty());
    }

    /// Thousands of changing section/camera combinations against the
    /// reference implementation that always tests every section box. The
    /// reference computes identical distance bits, slot and origin values.
    #[test]
    fn fully_contained_columns_match_individual_section_tests_with_churn() {
        let mut r: Resident<u32> = Resident::default();
        for x in -12i32..=12 {
            for z in -12i32..=12 {
                for y in 0..24 {
                    if (x * 7 + y * 3 + z * 5).rem_euclid(5) != 0 {
                        r.insert((x, y, z), (x ^ y ^ z) as u32);
                    }
                }
            }
        }
        for i in 0i32..180 {
            // Mutate an existing section near the camera each step: the
            // dense slot table can swap its final occupant into the hole.
            let key = (i * 7 % 23 - 11, i * 3 % 24, i * 13 % 23 - 11);
            if i % 3 == 0 {
                r.remove(key);
            } else {
                r.insert(key, i as u32);
            }
            let camera = DVec3::new(
                f64::from(i * 17 % 181 - 90) + 0.25,
                f64::from(i * 23 % 280 - 70) + 0.75,
                f64::from(i * 31 % 181 - 90) + 0.5,
            );
            let yaw = (i * 71 % 360) as f32;
            let pitch = (i * 43 % 160 - 80) as f32;
            let projection = super::super::world_projection(70.0, 16.0 / 9.0, 1100.0);
            let frustum = Frustum {
                planes: super::super::frustum(projection * crate::camera::view_matrix(yaw, pitch)),
            };
            let reachable = |key: SectionKey| (key.0 + 3 * key.1 - key.2 + i).rem_euclid(11) != 0;
            let beyond = |x: i32, z: i32| (x * 3 + z + i).rem_euclid(13) == 0;
            for radius in [2, 8, 16] {
                let mut actual = Vec::new();
                r.visible(camera, radius, &frustum, reachable, beyond, &mut actual);
                let mut expected = Vec::new();
                for (slot, (key, _)) in r.items.iter().enumerate() {
                    let near = in_view_distance(camera_column(camera), radius, key.0, key.2);
                    if (near && !reachable(*key)) || (!near && !beyond(key.0, key.2)) {
                        continue;
                    }
                    if let Some((distance, rel)) = in_frustum(*key, camera, &frustum) {
                        expected.push(Visible {
                            distance: distance.to_bits(),
                            key: *key,
                            slot: slot as u32,
                            origin: rel.to_array(),
                        });
                    }
                }
                expected.sort_unstable_by_key(|v| (v.distance, v.key));
                assert_eq!(
                    actual, expected,
                    "case={i} radius={radius}, yaw={yaw} pitch={pitch}"
                );
            }
        }
    }

    /// Same-process A/B benchmark against the original column-frustum loop.
    /// Never use this as a claim of whole-game FPS improvement.
    #[test]
    #[ignore = "CPU timing: run separately with --profile fast and --nocapture"]
    fn column_inside_fast_path_speed() {
        use std::{hint::black_box, time::Instant};
        let mut r: Resident<u32> = Resident::default();
        for x in -33i32..=33 {
            for z in -33i32..=33 {
                for y in 0..24 {
                    if (x * 7 + y * 3 + z * 5).rem_euclid(4) != 0 {
                        r.insert((x, y, z), (x ^ y ^ z) as u32);
                    }
                }
            }
        }
        let projection = super::super::world_projection(70.0, 16.0 / 9.0, 1100.0);
        let camera = DVec3::new(8.5, 100.0, 8.5);
        for (name, yaw, pitch, radius) in [
            ("meadow_rd16", 20.0, 15.0, 16),
            ("meadow_rd32", 20.0, 15.0, 32),
            ("peak_rd32", 65.0, -40.0, 32),
            ("pitch_n89_rd32", 30.0, -89.0, 32),
            ("pitch_n80_rd32", 30.0, -80.0, 32),
            ("pitch_n70_rd32", 30.0, -70.0, 32),
            ("pitch_n60_rd32", 30.0, -60.0, 32),
            ("pitch_0_rd32", 30.0, 0.0, 32),
            ("pitch_50_rd32", 30.0, 50.0, 32),
            ("pitch_60_rd32", 30.0, 60.0, 32),
            ("pitch_70_rd32", 30.0, 70.0, 32),
            ("pitch_75_rd32", 30.0, 75.0, 32),
            ("pitch_80_rd32", 30.0, 80.0, 32),
            ("top_down_rd32", 30.0, 85.0, 32),
            ("pitch_89_rd32", 30.0, 89.0, 32),
        ] {
            let frustum = Frustum {
                planes: super::super::frustum(projection * crate::camera::view_matrix(yaw, pitch)),
            };
            let old = |out: &mut Vec<Visible>| {
                out.clear();
                let from = camera_column(camera);
                for c in &r.columns {
                    if !in_view_distance(from, radius, c.x, c.z) {
                        continue;
                    }
                    let min = origin((c.x, c.low, c.z), camera).as_vec3();
                    let top = origin((c.x, c.high, c.z), camera).as_vec3();
                    if !frustum.intersects(min, top + Vec3::splat(16.0)) {
                        continue;
                    }
                    for &(y, slot) in &c.sections {
                        let key = (c.x, y, c.z);
                        if let Some((dist, o)) = in_frustum(key, camera, &frustum) {
                            out.push(Visible {
                                distance: dist.to_bits(),
                                key,
                                slot,
                                origin: o.to_array(),
                            });
                        }
                    }
                }
                out.sort_unstable_by_key(|v| (v.distance, v.key));
            };
            let mut fast = Vec::new();
            let mut reference = Vec::new();
            for _ in 0..6 {
                old(&mut reference);
                fast.clear();
                r.visible(camera, radius, &frustum, |_| true, |_, _| false, &mut fast);
            }
            assert_eq!(fast, reference, "reference/fast mismatch for {name}");
            let mut base_samples = Vec::new();
            let mut fast_samples = Vec::new();
            for k in 0..24 {
                let first_fast = k % 2 == 0;
                for is_fast in [first_fast, !first_fast] {
                    let t = Instant::now();
                    if is_fast {
                        for _ in 0..3 {
                            fast.clear();
                            r.visible(camera, radius, &frustum, |_| true, |_, _| false, &mut fast);
                            black_box(fast.len());
                        }
                        fast_samples.push(t.elapsed().as_secs_f64() * 1e6 / 3.0);
                    } else {
                        for _ in 0..3 {
                            old(&mut reference);
                            black_box(reference.len());
                        }
                        base_samples.push(t.elapsed().as_secs_f64() * 1e6 / 3.0);
                    }
                }
            }
            base_samples.sort_by(f64::total_cmp);
            fast_samples.sort_by(f64::total_cmp);
            assert_eq!(fast, reference);
            let baseline_us = base_samples[12];
            let candidate_us = fast_samples[12];
            println!(
                "culling_ab,{name},radius={radius},resident={},visible={},near_y={:.4},baseline_us={baseline_us:.3},candidate_us={candidate_us:.3},speedup={:.3}",
                r.len(),
                fast.len(),
                frustum.planes[4].y,
                baseline_us / candidate_us,
            );
        }
    }

    /// Explain which columns and section counts benefit from a shared
    /// frustum-plane mask. Diagnostic-only; does not change rendering.
    #[test]
    #[ignore = "geometry diagnostics only"]
    fn column_clip_mask_distribution() {
        let mut r: Resident<()> = Resident::default();
        for x in -33i32..=33 {
            for z in -33i32..=33 {
                for y in 0..24 {
                    if (x * 7 + y * 3 + z * 5).rem_euclid(4) != 0 {
                        r.insert((x, y, z), ());
                    }
                }
            }
        }
        let camera = DVec3::new(8.5, 100.0, 8.5);
        for (name, yaw, pitch, radius) in [
            ("meadow_rd16", 20.0, 15.0, 16),
            ("meadow_rd32", 20.0, 15.0, 32),
            ("peak_rd32", 65.0, -40.0, 32),
            ("top_down_rd32", 30.0, 85.0, 32),
        ] {
            let proj = super::super::world_projection(70.0, 16.0 / 9.0, 1100.0);
            let frustum = Frustum {
                planes: super::super::frustum(proj * crate::camera::view_matrix(yaw, pitch)),
            };
            let mut inspected = 0usize;
            let mut accepted = 0usize;
            let mut accepted_sections = 0usize;
            let mut by_planes = [0usize; 7];
            let mut by_sections = [0usize; 7];
            for c in &r.columns {
                if !in_view_distance(camera_column(camera), radius, c.x, c.z) {
                    continue;
                }
                inspected += 1;
                let min = origin((c.x, c.low, c.z), camera).as_vec3();
                let top = origin((c.x, c.high, c.z), camera).as_vec3();
                if let Some(mask) = frustum.column_clip_mask(min, top + Vec3::splat(16.0)) {
                    accepted += 1;
                    accepted_sections += c.sections.len();
                    let p = mask.count_ones() as usize;
                    by_planes[p] += 1;
                    by_sections[p] += c.sections.len();
                }
            }
            println!(
                "frustum_mask_diag,{name},columns_inspected={inspected},columns_accepted={accepted},sections_accepted={accepted_sections},columns_by_active_planes={by_planes:?},sections_by_active_planes={by_sections:?}"
            );
        }
    }

    /// Research proxy for #987: group existing chunk columns into larger
    /// X/Z cells and reject the whole tile by its tight frustum AABB before
    /// visiting contained columns. Built only in the ignored benchmark: this
    /// is NOT a dynamic octree or a proposed production data structure.
    #[test]
    #[ignore = "CPU timing / hierarchy research, not a normal regression test"]
    fn tiled_column_hierarchy_speed() {
        use std::{collections::BTreeMap, hint::black_box, time::Instant};
        struct Tile {
            min: (i32, i32, i32),
            max: (i32, i32, i32),
            columns: Vec<usize>,
        }
        let mut r: Resident<()> = Resident::default();
        for x in -33i32..=33 {
            for z in -33i32..=33 {
                for y in 0..24 {
                    if (x * 7 + y * 3 + z * 5).rem_euclid(4) != 0 {
                        r.insert((x, y, z), ());
                    }
                }
            }
        }
        let camera = DVec3::new(8.5, 100.0, 8.5);
        let project = super::super::world_projection(70.0, 16.0 / 9.0, 1100.0);
        for width in [4, 8, 16] {
            let build_at = Instant::now();
            let mut cells = BTreeMap::<(i32, i32), Vec<usize>>::new();
            for (i, c) in r.columns.iter().enumerate() {
                cells
                    .entry((c.x.div_euclid(width), c.z.div_euclid(width)))
                    .or_default()
                    .push(i);
            }
            let tiles: Vec<Tile> = cells
                .into_values()
                .map(|columns| {
                    let mut bounds = Tile {
                        min: (i32::MAX, i32::MAX, i32::MAX),
                        max: (i32::MIN, i32::MIN, i32::MIN),
                        columns,
                    };
                    for &i in &bounds.columns {
                        let c = &r.columns[i];
                        bounds.min.0 = bounds.min.0.min(c.x);
                        bounds.min.1 = bounds.min.1.min(c.low);
                        bounds.min.2 = bounds.min.2.min(c.z);
                        bounds.max.0 = bounds.max.0.max(c.x);
                        bounds.max.1 = bounds.max.1.max(c.high);
                        bounds.max.2 = bounds.max.2.max(c.z);
                    }
                    bounds
                })
                .collect();
            let build_us = build_at.elapsed().as_secs_f64() * 1e6;
            for (name, yaw, pitch, radius) in [
                ("meadow_rd16", 20.0, 15.0, 16),
                ("meadow_rd32", 20.0, 15.0, 32),
                ("peak_rd32", 65.0, -40.0, 32),
                ("top_down_rd32", 30.0, 85.0, 32),
            ] {
                let frustum = Frustum {
                    planes: super::super::frustum(project * crate::camera::view_matrix(yaw, pitch)),
                };
                let from = camera_column(camera);
                let mask = frustum.masks_help_for_view();
                let mut tiled = Vec::<Visible>::new();
                let mut base = Vec::<Visible>::new();
                let tiled_walk = |out: &mut Vec<Visible>| {
                    out.clear();
                    for tile in &tiles {
                        let min = origin(tile.min, camera).as_vec3();
                        let max = origin(tile.max, camera).as_vec3() + Vec3::splat(16.0);
                        if !frustum.intersects(min, max) {
                            continue;
                        }
                        for &idx in &tile.columns {
                            let c = &r.columns[idx];
                            if !in_view_distance(from, radius, c.x, c.z) {
                                continue;
                            }
                            let min = origin((c.x, c.low, c.z), camera).as_vec3();
                            let max =
                                origin((c.x, c.high, c.z), camera).as_vec3() + Vec3::splat(16.0);
                            let Some(clip_mask) = (if mask {
                                frustum.column_clip_mask(min, max)
                            } else {
                                frustum.intersects(min, max).then_some(0b11_1111)
                            }) else {
                                continue;
                            };
                            for &(y, slot) in &c.sections {
                                let key = (c.x, y, c.z);
                                let visibility = if clip_mask == 0 {
                                    let rel = origin(key, camera);
                                    Some((
                                        (rel + DVec3::splat(8.0)).length_squared(),
                                        rel.as_vec3(),
                                    ))
                                } else if clip_mask == 0b11_1111 {
                                    in_frustum(key, camera, &frustum)
                                } else {
                                    let rel = origin(key, camera);
                                    let pos = rel.as_vec3();
                                    frustum
                                        .intersects_masked(pos, pos + Vec3::splat(16.0), clip_mask)
                                        .then(|| ((rel + DVec3::splat(8.0)).length_squared(), pos))
                                };
                                if let Some((dist, rel)) = visibility {
                                    out.push(Visible {
                                        distance: dist.to_bits(),
                                        key,
                                        slot,
                                        origin: rel.to_array(),
                                    });
                                }
                            }
                        }
                    }
                    out.sort_unstable_by_key(|v| (v.distance, v.key));
                };
                for _ in 0..5 {
                    base.clear();
                    r.visible(camera, radius, &frustum, |_| true, |_, _| false, &mut base);
                    tiled_walk(&mut tiled);
                }
                assert_eq!(tiled, base, "tile={width} scene={name}");
                let mut baseline = Vec::new();
                let mut candidate = Vec::new();
                for k in 0..24 {
                    for test_tile in [k % 2 == 0, k % 2 != 0] {
                        let before = Instant::now();
                        if test_tile {
                            for _ in 0..3 {
                                tiled_walk(&mut tiled);
                                black_box(tiled.len());
                            }
                            candidate.push(before.elapsed().as_secs_f64() * 1e6 / 3.0);
                        } else {
                            for _ in 0..3 {
                                base.clear();
                                r.visible(
                                    camera,
                                    radius,
                                    &frustum,
                                    |_| true,
                                    |_, _| false,
                                    &mut base,
                                );
                                black_box(base.len());
                            }
                            baseline.push(before.elapsed().as_secs_f64() * 1e6 / 3.0);
                        }
                    }
                }
                baseline.sort_by(f64::total_cmp);
                candidate.sort_by(f64::total_cmp);
                assert_eq!(tiled, base);
                let original_us = baseline[12];
                let tiled_us = candidate[12];
                println!(
                    "culling_tiles,{name},tile={width},groups={},build_us={build_us:.3},resident={},visible={},original_us={original_us:.3},tiled_us={tiled_us:.3},speedup={:.3}",
                    tiles.len(),
                    r.len(),
                    base.len(),
                    original_us / tiled_us,
                );
            }
        }
    }

    /// The old walk (every key of a hash map, sorted by distance, then two
    /// lookups per visible section) against `visible` on a render distance
    /// 32 world. Run with `cargo test --profile fast -p horizon-client --lib
    /// -- --ignored column_walk_speed --nocapture`.
    #[test]
    #[ignore = "timing, not a check"]
    fn column_walk_speed() {
        use std::time::{Duration, Instant};
        let mut r = Resident::default();
        let mut map = HashMap::new();
        for x in -33i32..=33 {
            for z in -33i32..=33 {
                for y in 0..24 {
                    if (x * 7 + y * 3 + z * 5).rem_euclid(4) != 0 {
                        r.insert((x, y, z), [x, y, z]);
                        map.insert((x, y, z), [x, y, z]);
                    }
                }
            }
        }
        let camera = DVec3::new(8.5, 100.0, 8.5);
        let projection = super::super::world_projection(70.0, 16.0 / 9.0, 1024.0);
        let frustum = Frustum {
            planes: super::super::frustum(projection * crate::camera::view_matrix(30.0, 35.0)),
        };
        let (mut old, mut new) = (Duration::MAX, Duration::MAX);
        let (mut list, mut keys) = (Vec::new(), Vec::new());
        let mut sum = 0i64;
        for _ in 0..200 {
            let t = Instant::now();
            keys.clear();
            keys.extend(map.keys().filter_map(|&k| {
                in_view(k, camera, 32, &frustum).map(|(d, o)| (k, d, o.to_array()))
            }));
            keys.sort_unstable_by(|a, b| a.1.total_cmp(&b.1));
            for (k, _, _) in &keys {
                sum += i64::from(map[k][1] + map[k][0]);
            }
            old = old.min(t.elapsed());
            let t = Instant::now();
            list.clear();
            r.visible(camera, 32, &frustum, |_| true, |_, _| false, &mut list);
            for v in &list {
                sum += i64::from(r.get(v.slot)[1] + r.get(v.slot)[0]);
            }
            new = new.min(t.elapsed());
        }
        assert_eq!(keys.len(), list.len());
        println!(
            "{} resident, {} visible: old walk {old:?}, column walk {new:?} (checksum {sum})",
            r.len(),
            list.len()
        );
    }
}
