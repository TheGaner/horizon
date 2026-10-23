//! Same-input comparisons for dynamically maintained 4/8/16-column tiles.
use super::*;
use std::time::Instant;

fn random(seed: &mut u32) -> u32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    *seed
}

fn populate(store: &mut Resident<u32>, sparse: bool) {
    for x in -33i32..=33 {
        for z in -33i32..=33 {
            // Sparse/cave layouts exercise widely varying y bounds and empty columns.
            if sparse && (x * 7 + z * 11).rem_euclid(5) <= 1 {
                continue;
            }
            for y in 0..24 {
                if sparse {
                    if (x * 13 + y * 3 + z * 7).rem_euclid(8) > 1 {
                        continue;
                    }
                } else if (x * 7 + y * 3 + z * 5).rem_euclid(4) == 0 {
                    continue;
                }
                let shift = if sparse {
                    (x * 7 + z * 5).rem_euclid(18) - 9
                } else {
                    0
                };
                store.insert((x, y + shift, z), (x ^ y ^ z) as u32);
            }
        }
    }
}

fn check_exact(a: &Resident<u32>, b: &Resident<u32>, frame: u32) {
    assert_eq!(a.items, b.items);
    let tile = b.tiled_index.as_ref().expect("indexed store");
    tile.validate(&b.columns);
    for (yaw, pitch, radius) in [(15., 10., 8), (65., -40., 16), (35., 85., 32)] {
        let camera = DVec3::new(
            (frame % 163) as f64 - 79.25,
            (frame % 151) as f64 - 23.75,
            (frame % 113) as f64 - 56.5,
        );
        let proj = super::super::world_projection(70., 16. / 9., 900.);
        let frustum = Frustum {
            planes: super::super::frustum(proj * crate::camera::view_matrix(yaw, pitch)),
        };
        let reachable = |key: SectionKey| (key.0 + key.1 * 3 - key.2).rem_euclid(7) != 0;
        let beyond = |x: i32, z: i32| (x * 5 + z).rem_euclid(6) == 0;
        let mut reference = Vec::new();
        let mut indexed = Vec::new();
        a.visible(camera, radius, &frustum, reachable, beyond, &mut reference);
        b.visible_tiled(camera, radius, &frustum, reachable, beyond, &mut indexed);
        assert_eq!(
            reference,
            indexed,
            "width={} frame={} pitch={pitch}",
            tile.width(),
            frame
        );
    }
}

#[test]
fn all_tile_widths_preserve_bounds_indices_and_visible_during_churn() {
    for width in [4, 8, 16] {
        for sparse in [false, true] {
            let mut reference = Resident::<u32>::default();
            populate(&mut reference, sparse);
            let mut indexed = Resident::<u32>::default();
            populate(&mut indexed, sparse);
            indexed.enable_tiled_index_with_width(width);
            check_exact(&reference, &indexed, 0);
            let mut seed = 0xc5ac_7241u32;
            for n in 0..1800u32 {
                let r = random(&mut seed);
                let key = if n % 3 == 0 && !reference.items.is_empty() {
                    reference.items[(r as usize) % reference.items.len()].0
                } else {
                    (
                        (r % 81) as i32 - 40,
                        ((r >> 8) % 45) as i32 - 12,
                        ((r >> 17) % 81) as i32 - 40,
                    )
                };
                if n % 3 == 0 {
                    assert_eq!(reference.remove(key), indexed.remove(key));
                } else {
                    reference.insert(key, n);
                    indexed.insert(key, n);
                }
                assert_eq!(reference.items, indexed.items);
                indexed
                    .tiled_index
                    .as_ref()
                    .unwrap()
                    .validate(&indexed.columns);
                if n % 75 == 0 {
                    check_exact(&reference, &indexed, n + 1);
                }
                if n == 1100 {
                    reference.clear();
                    indexed.clear();
                    assert_eq!(indexed.tiled_index.as_ref().unwrap().width(), width);
                    check_exact(&reference, &indexed, n + 1);
                }
            }
            check_exact(&reference, &indexed, 1900);
        }
    }
}

fn percentiles(v: &mut [f64]) -> (f64, f64, f64) {
    v.sort_by(f64::total_cmp);
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    (at(0.5), at(0.95), at(0.99))
}

/// Uses an identical deterministic update sequence for all widths, each with
/// its own no-index control. The combined scenario includes sparse caves,
/// exact output equality, and full column unload/reload cycles.
#[test]
#[ignore = "optimized comparison: cargo test --profile fast --lib tile_width_sweep -- --ignored --nocapture"]
fn tile_width_sweep() {
    use std::hint::black_box;
    for sparse in [false, true] {
        let scene = if sparse { "sparse" } else { "dense" };
        for width in [4, 8, 16] {
            let mut plain = Resident::<u32>::default();
            let mut indexed = Resident::<u32>::default();
            populate(&mut plain, sparse);
            populate(&mut indexed, sparse);
            let start = Instant::now();
            indexed.enable_tiled_index_with_width(width);
            let build_us = start.elapsed().as_secs_f64() * 1e6;
            let tiles = indexed.tiled_index.as_ref().unwrap().tile_count();
            let memory_lower_bound_bytes = indexed
                .tiled_index
                .as_ref()
                .unwrap()
                .known_allocated_bytes();
            assert_eq!(plain.items, indexed.items);
            let mut old_ops = Vec::new();
            let mut new_ops = Vec::new();
            let mut seed = 0x4bad_f130u32;
            for i in 0..1400u32 {
                let r = random(&mut seed);
                let key = if i % 3 == 0 && !plain.items.is_empty() {
                    plain.items[(r as usize) % plain.items.len()].0
                } else {
                    (
                        (r % 75) as i32 - 37,
                        ((r >> 9) % 38) as i32 - 5,
                        ((r >> 18) % 75) as i32 - 37,
                    )
                };
                let operate = |store: &mut Resident<u32>| {
                    if i % 3 == 0 {
                        black_box(store.remove(key));
                    } else {
                        store.insert(key, i);
                    }
                };
                if i % 2 == 0 {
                    let t = Instant::now();
                    operate(&mut plain);
                    old_ops.push(t.elapsed().as_secs_f64() * 1e6);
                    let t = Instant::now();
                    operate(&mut indexed);
                    new_ops.push(t.elapsed().as_secs_f64() * 1e6);
                } else {
                    let t = Instant::now();
                    operate(&mut indexed);
                    new_ops.push(t.elapsed().as_secs_f64() * 1e6);
                    let t = Instant::now();
                    operate(&mut plain);
                    old_ops.push(t.elapsed().as_secs_f64() * 1e6);
                }
            }
            check_exact(&plain, &indexed, 1401);
            let old_updates = percentiles(&mut old_ops);
            let new_updates = percentiles(&mut new_ops);
            println!(
                "tile_width_update,layout={scene},width={width},sections={},tiles={tiles},build_us={build_us:.3},known_bytes={memory_lower_bound_bytes},plain_p50_us={:.3},tile_p50_us={:.3},plain_p95_us={:.3},tile_p95_us={:.3},plain_p99_us={:.3},tile_p99_us={:.3}",
                indexed.len(),
                old_updates.0,
                new_updates.0,
                old_updates.1,
                new_updates.1,
                old_updates.2,
                new_updates.2
            );
            let camera = DVec3::new(8.5, 100., 8.5);
            for (name, yaw, pitch, radius) in [
                ("meadow_rd16", 20., 15., 16),
                ("meadow_rd32", 20., 15., 32),
                ("peak_rd32", 65., -40., 32),
                ("top_down_rd32", 30., 85., 32),
            ] {
                let projection = super::super::world_projection(70., 16. / 9., 900.);
                let frustum = Frustum {
                    planes: super::super::frustum(
                        projection * crate::camera::view_matrix(yaw, pitch),
                    ),
                };
                let mut a = Vec::new();
                let mut b = Vec::new();
                let mut old = Vec::new();
                let mut candidate = Vec::new();
                for j in 0..36 {
                    for tile_first in [j % 2 == 0, j % 2 != 0] {
                        let t = Instant::now();
                        if tile_first {
                            b.clear();
                            indexed.visible_tiled(
                                camera,
                                radius,
                                &frustum,
                                |_| true,
                                |_, _| false,
                                &mut b,
                            );
                            black_box(b.len());
                            candidate.push(t.elapsed().as_secs_f64() * 1e6);
                        } else {
                            a.clear();
                            plain.visible(camera, radius, &frustum, |_| true, |_, _| false, &mut a);
                            black_box(a.len());
                            old.push(t.elapsed().as_secs_f64() * 1e6);
                        }
                    }
                }
                assert_eq!(a, b);
                let old = percentiles(&mut old);
                let new = percentiles(&mut candidate);
                println!(
                    "tile_width_visible,layout={scene},width={width},scene={name},sections={},visible={},plain_p50_us={:.3},tile_p50_us={:.3},plain_p95_us={:.3},tile_p95_us={:.3},plain_p99_us={:.3},tile_p99_us={:.3}",
                    plain.len(),
                    a.len(),
                    old.0,
                    new.0,
                    old.1,
                    new.1,
                    old.2,
                    new.2
                );
            }
            // Full column updates: remove all levels, reinsert new extrema.
            let mut old = Vec::new();
            let mut candidate = Vec::new();
            let mut seed = 0x31ab_906du32;
            for i in 0..360u32 {
                let r = random(&mut seed);
                let (x, z) = ((r % 67) as i32 - 33, ((r >> 12) % 67) as i32 - 33);
                let update = |store: &mut Resident<u32>| {
                    for y in -15..50 {
                        black_box(store.remove((x, y, z)));
                    }
                    for y in -8..35 {
                        if (y + x * 3 + z * 7 + i as i32).rem_euclid(4) != 0 {
                            store.insert((x, y, z), i);
                        }
                    }
                };
                if i % 2 == 0 {
                    let t = Instant::now();
                    update(&mut plain);
                    old.push(t.elapsed().as_secs_f64() * 1e6);
                    let t = Instant::now();
                    update(&mut indexed);
                    candidate.push(t.elapsed().as_secs_f64() * 1e6);
                } else {
                    let t = Instant::now();
                    update(&mut indexed);
                    candidate.push(t.elapsed().as_secs_f64() * 1e6);
                    let t = Instant::now();
                    update(&mut plain);
                    old.push(t.elapsed().as_secs_f64() * 1e6);
                }
            }
            check_exact(&plain, &indexed, 1800);
            let old = percentiles(&mut old);
            let new = percentiles(&mut candidate);
            println!(
                "tile_width_columns,layout={scene},width={width},cycles=360,plain_p50_us={:.3},tile_p50_us={:.3},plain_p95_us={:.3},tile_p95_us={:.3},plain_p99_us={:.3},tile_p99_us={:.3}",
                old.0, new.0, old.1, new.1, old.2, new.2
            );
        }
    }
}
