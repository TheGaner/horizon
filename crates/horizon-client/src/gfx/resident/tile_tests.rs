//! Differential and streaming-churn benchmarks for optional incremental tiles.
use super::*;
use std::time::Instant;

fn rng(seed: &mut u32) -> u32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    *seed
}

fn compare_visible(reference: &Resident<u32>, indexed: &Resident<u32>, iteration: u32) {
    assert_eq!(reference.items, indexed.items);
    indexed
        .tiled_index
        .as_ref()
        .unwrap()
        .validate(&indexed.columns);
    for (yaw, pitch, radius) in [(15.0, 5.0, 8), (65.0, -60.0, 16), (195.0, 80.0, 24)] {
        let camera = DVec3::new(
            f64::from((iteration * 17) % 320) - 160.25,
            f64::from((iteration * 29) % 360) - 100.5,
            f64::from((iteration * 43) % 320) - 159.75,
        );
        let projection = super::super::world_projection(70.0, 16.0 / 9.0, 1024.0);
        let frustum = Frustum {
            planes: super::super::frustum(projection * crate::camera::view_matrix(yaw, pitch)),
        };
        let reachable = |key: SectionKey| (key.0 * 3 + key.1 - key.2).rem_euclid(9) != 0;
        let beyond = |x: i32, z: i32| (x + z).rem_euclid(5) == 0;
        let mut original = Vec::new();
        let mut tiled = Vec::new();
        reference.visible(camera, radius, &frustum, reachable, beyond, &mut original);
        indexed.visible_tiled(camera, radius, &frustum, reachable, beyond, &mut tiled);
        assert_eq!(
            original, tiled,
            "iteration={iteration} yaw={yaw} pitch={pitch} radius={radius}"
        );
        for v in tiled {
            assert_eq!(indexed.items[v.slot as usize].0, v.key);
        }
    }
}

#[test]
fn incremental_tiles_handle_extrema_negative_coords_swap_removes_and_clear() {
    let mut baseline = Resident::<u32>::default();
    let mut indexed = Resident::<u32>::default();
    let keys = [
        (-9, -5, -9),
        (-8, 0, -8),
        (-1, 1, -1),
        (0, 2, 0),
        (7, 3, 7),
        (8, 4, 8),
        (7, 100, 7),
        (-8, -12, -8),
        (-9, 80, -8),
        (-8, 20, -9),
        (8, 10, 7),
    ];
    for (i, &key) in keys.iter().enumerate() {
        baseline.insert(key, i as u32);
        indexed.insert(key, i as u32);
    }
    indexed.enable_tiled_index();
    compare_visible(&baseline, &indexed, 7);
    for (i, &key) in keys.iter().rev().enumerate() {
        assert_eq!(baseline.remove(key), indexed.remove(key));
        compare_visible(&baseline, &indexed, i as u32);
    }
    assert_eq!(indexed.tiled_index.as_ref().unwrap().tile_count(), 0);
    for key in keys {
        baseline.insert(key, 17);
        indexed.insert(key, 17);
    }
    compare_visible(&baseline, &indexed, 70);
    baseline.clear();
    indexed.clear();
    indexed
        .tiled_index
        .as_ref()
        .unwrap()
        .validate(&indexed.columns);
    assert_eq!(indexed.tiled_index.as_ref().unwrap().tile_count(), 0);
    indexed.insert((-65, -7, 64), 41);
    baseline.insert((-65, -7, 64), 41);
    compare_visible(&baseline, &indexed, 88);
}

#[test]
fn incremental_tiles_match_unindexed_store_under_random_streaming_churn() {
    let mut reference = Resident::default();
    let mut indexed = Resident::default();
    let mut seed = 0x2545_ba71u32;
    // Populate pre-existing sections before enabling the index; real clients
    // may activate a feature after a world is already resident.
    for i in 0..2100u32 {
        let r = rng(&mut seed);
        let key = (
            (r % 67) as i32 - 33,
            ((r >> 10) % 36) as i32 - 7,
            ((r >> 19) % 67) as i32 - 33,
        );
        reference.insert(key, i);
        indexed.insert(key, i);
    }
    indexed.enable_tiled_index();
    compare_visible(&reference, &indexed, 0);
    for iteration in 0..3200u32 {
        let r = rng(&mut seed);
        let key = if iteration % 3 == 0 && !reference.items.is_empty() {
            reference.items[(r as usize) % reference.items.len()].0
        } else {
            (
                (r % 89) as i32 - 44,
                ((r >> 9) % 44) as i32 - 9,
                ((r >> 18) % 89) as i32 - 44,
            )
        };
        if iteration % 3 == 0 {
            assert_eq!(reference.remove(key), indexed.remove(key));
        } else {
            reference.insert(key, iteration);
            indexed.insert(key, iteration);
        }
        assert_eq!(reference.items, indexed.items);
        // Validate *every* mutation, including replacement and empty tiles.
        indexed
            .tiled_index
            .as_ref()
            .unwrap()
            .validate(&indexed.columns);
        if iteration % 47 == 0 {
            compare_visible(&reference, &indexed, iteration + 1);
        }
        if iteration == 2100 {
            reference.clear();
            indexed.clear();
            compare_visible(&reference, &indexed, iteration + 1);
        }
    }
    compare_visible(&reference, &indexed, 3201);
}

fn summarize(samples: &mut [f64]) -> (f64, f64, f64) {
    samples.sort_by(f64::total_cmp);
    let get = |q: f64| {
        let at = ((samples.len() - 1) as f64 * q).round() as usize;
        samples[at]
    };
    (get(0.5), get(0.95), get(0.99))
}

/// Timed hot update/visibility comparison. Does not affect production builds.
#[test]
#[ignore = "optimized CPU benchmark; use --profile fast --nocapture"]
fn incremental_tiles_update_and_visibility_speed() {
    use std::hint::black_box;
    let mut baseline = Resident::<u32>::default();
    let mut indexed = Resident::<u32>::default();
    for x in -33i32..=33 {
        for z in -33i32..=33 {
            for y in 0..24 {
                if (x * 7 + y * 3 + z * 5).rem_euclid(4) != 0 {
                    let key = (x, y, z);
                    baseline.insert(key, 0);
                    indexed.insert(key, 0);
                }
            }
        }
    }
    let start = Instant::now();
    indexed.enable_tiled_index();
    let build_us = start.elapsed().as_secs_f64() * 1e6;
    let tiles = indexed.tiled_index.as_ref().unwrap().tile_count();
    assert_eq!(baseline.items, indexed.items);
    let mut baseline_samples = Vec::new();
    let mut indexed_samples = Vec::new();
    let mut rng_state = 0x1357_2468u32;
    for i in 0..2400u32 {
        let r = rng(&mut rng_state);
        let key = if i % 3 == 0 {
            baseline.items[(r as usize) % baseline.items.len()].0
        } else {
            (
                (r % 73) as i32 - 36,
                ((r >> 9) % 28) as i32,
                ((r >> 17) % 73) as i32 - 36,
            )
        };
        let update = |store: &mut Resident<u32>| {
            if i % 3 == 0 {
                black_box(store.remove(key));
            } else {
                store.insert(key, i);
            }
        };
        let (old, new) = if i % 2 == 0 {
            let t = Instant::now();
            update(&mut baseline);
            let a = t.elapsed().as_secs_f64() * 1e6;
            let t = Instant::now();
            update(&mut indexed);
            let b = t.elapsed().as_secs_f64() * 1e6;
            (a, b)
        } else {
            let t = Instant::now();
            update(&mut indexed);
            let b = t.elapsed().as_secs_f64() * 1e6;
            let t = Instant::now();
            update(&mut baseline);
            let a = t.elapsed().as_secs_f64() * 1e6;
            (a, b)
        };
        baseline_samples.push(old);
        indexed_samples.push(new);
        if i % 200 == 0 {
            indexed
                .tiled_index
                .as_ref()
                .unwrap()
                .validate(&indexed.columns);
            compare_visible(&baseline, &indexed, i);
        }
    }
    assert_eq!(baseline.items, indexed.items);
    let a = summarize(&mut baseline_samples);
    let b = summarize(&mut indexed_samples);
    println!(
        "incremental_tiles_update,sections={},tiles={},build_us={build_us:.3},baseline_p50_us={:.3},indexed_p50_us={:.3},baseline_p95_us={:.3},indexed_p95_us={:.3},baseline_p99_us={:.3},indexed_p99_us={:.3}",
        indexed.len(),
        tiles,
        a.0,
        b.0,
        a.1,
        b.1,
        a.2,
        b.2
    );
    let camera = DVec3::new(8.5, 100.0, 8.5);
    for (name, yaw, pitch, radius) in [
        ("meadow_rd16", 20.0, 15.0, 16),
        ("meadow_rd32", 20.0, 15.0, 32),
        ("peak_rd32", 65.0, -40.0, 32),
        ("top_down_rd32", 30.0, 85.0, 32),
    ] {
        let projection = super::super::world_projection(70.0, 16.0 / 9.0, 1100.0);
        let frustum = Frustum {
            planes: super::super::frustum(projection * crate::camera::view_matrix(yaw, pitch)),
        };
        let mut a = Vec::new();
        let mut b = Vec::new();
        let mut old = Vec::new();
        let mut tile = Vec::new();
        for i in 0..40 {
            for choose_tile in [i % 2 == 0, i % 2 != 0] {
                let start = Instant::now();
                if choose_tile {
                    b.clear();
                    indexed.visible_tiled(camera, radius, &frustum, |_| true, |_, _| false, &mut b);
                    black_box(b.len());
                    tile.push(start.elapsed().as_secs_f64() * 1e6);
                } else {
                    a.clear();
                    baseline.visible(camera, radius, &frustum, |_| true, |_, _| false, &mut a);
                    black_box(a.len());
                    old.push(start.elapsed().as_secs_f64() * 1e6);
                }
            }
        }
        assert_eq!(a, b);
        let old = summarize(&mut old);
        let tile = summarize(&mut tile);
        println!(
            "incremental_tiles_visible,{name},sections={},tiles={},visible={},baseline_p50_us={:.3},indexed_p50_us={:.3},baseline_p95_us={:.3},indexed_p95_us={:.3},baseline_p99_us={:.3},indexed_p99_us={:.3}",
            indexed.len(),
            indexed.tiled_index.as_ref().unwrap().tile_count(),
            a.len(),
            old.0,
            tile.0,
            old.1,
            tile.1,
            old.2,
            tile.2
        );
    }
}

/// Explicitly exercise column deletion/re-creation, resident swap-remove and
/// tile-extrema changes rather than only random single-section updates.
#[test]
#[ignore = "optimized CPU streaming benchmark; use --profile fast --nocapture"]
fn incremental_tiles_full_column_streaming_speed() {
    use std::hint::black_box;
    let mut baseline = Resident::<u32>::default();
    let mut indexed = Resident::<u32>::default();
    for x in -24i32..=24 {
        for z in -24i32..=24 {
            for y in 0..24 {
                if (x * 3 + y + z * 7).rem_euclid(4) != 0 {
                    baseline.insert((x, y, z), 17);
                    indexed.insert((x, y, z), 17);
                }
            }
        }
    }
    let started = Instant::now();
    indexed.enable_tiled_index();
    let build_us = started.elapsed().as_secs_f64() * 1e6;
    let mut plain = Vec::new();
    let mut tiled = Vec::new();
    let mut seed = 0xa9bd_137cu32;
    for iteration in 0..1000u32 {
        let r = rng(&mut seed);
        let x = ((r % 49) as i32) - 24;
        let z = (((r >> 11) % 49) as i32) - 24;
        // Remove every possible prior Y before reinserting with different
        // extrema, including sections above and below the initial range.
        let operation = |store: &mut Resident<u32>| {
            for y in -3..27 {
                black_box(store.remove((x, y, z)));
            }
            for y in -3..27 {
                if (x * 3 + y + z * 7 + iteration as i32).rem_euclid(4) != 0 {
                    store.insert((x, y, z), iteration);
                }
            }
        };
        let (plain_us, tile_us) = if iteration % 2 == 0 {
            let at = Instant::now();
            operation(&mut baseline);
            let p = at.elapsed().as_secs_f64() * 1e6;
            let at = Instant::now();
            operation(&mut indexed);
            (p, at.elapsed().as_secs_f64() * 1e6)
        } else {
            let at = Instant::now();
            operation(&mut indexed);
            let t = at.elapsed().as_secs_f64() * 1e6;
            let at = Instant::now();
            operation(&mut baseline);
            (at.elapsed().as_secs_f64() * 1e6, t)
        };
        plain.push(plain_us);
        tiled.push(tile_us);
        if iteration % 50 == 0 {
            assert_eq!(baseline.items, indexed.items);
            compare_visible(&baseline, &indexed, iteration);
        }
    }
    let old = summarize(&mut plain);
    let new = summarize(&mut tiled);
    assert_eq!(baseline.items, indexed.items);
    indexed
        .tiled_index
        .as_ref()
        .unwrap()
        .validate(&indexed.columns);
    println!(
        "incremental_tiles_full_columns,cycles=1000,sections={},tiles={},build_us={build_us:.3},baseline_p50_us={:.3},indexed_p50_us={:.3},baseline_p95_us={:.3},indexed_p95_us={:.3},baseline_p99_us={:.3},indexed_p99_us={:.3}",
        indexed.len(),
        indexed.tiled_index.as_ref().unwrap().tile_count(),
        old.0,
        new.0,
        old.1,
        new.1,
        old.2,
        new.2
    );
}
