//! Section visibility gather/sort attribution and exact-output comparisons.
//! Benchmarks are ignored by default and do not replace production sorting.
use super::*;
use std::{hint::black_box, time::Instant};

fn populate(sparse: bool) -> Resident<u32> {
    let mut sections = Resident::default();
    for x in -33i32..=33 {
        for z in -33i32..=33 {
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
                sections.insert((x, y + shift, z), (x ^ y ^ z) as u32);
            }
        }
    }
    sections
}

fn stage_quantiles(values: &mut [f64]) -> (f64, f64, f64) {
    values.sort_by(f64::total_cmp);
    let at = |f: f64| values[((values.len() - 1) as f64 * f).round() as usize];
    (at(0.50), at(0.95), at(0.99))
}

#[test]
fn unsorted_gather_then_sort_matches_full_visible_with_append() {
    for sparse in [false, true] {
        let r = populate(sparse);
        for (yaw, pitch, radius) in [
            (20., 15., 16),
            (20., 15., 32),
            (65., -40., 32),
            (30., 85., 32),
        ] {
            let camera = DVec3::new(8.5, 100., 8.5);
            let projection = super::super::world_projection(70., 16. / 9., 1100.);
            let frustum = Frustum {
                planes: super::super::frustum(projection * crate::camera::view_matrix(yaw, pitch)),
            };
            // `visible` currently sorts the entire output, not just what it
            // appends. Preserve that contract even with an initial record.
            let sentinel = Visible {
                distance: 14.0f64.to_bits(),
                key: (999, 0, -999),
                slot: u32::MAX,
                origin: [1., 2., 3.],
            };
            let mut full = vec![sentinel];
            let mut separated = vec![sentinel];
            r.visible(
                camera,
                radius,
                &frustum,
                |k| (k.0 + k.1 - k.2).rem_euclid(7) != 0,
                |x, z| (x + z).rem_euclid(5) == 0,
                &mut full,
            );
            r.gather_visible(
                camera,
                radius,
                &frustum,
                |k| (k.0 + k.1 - k.2).rem_euclid(7) != 0,
                |x, z| (x + z).rem_euclid(5) == 0,
                &mut separated,
            );
            separated.sort_unstable_by_key(|v| (v.distance, v.key));
            assert_eq!(full, separated, "sparse={sparse} yaw={yaw} pitch={pitch}");
        }
    }
}

#[test]
fn radix_sort_keeps_exact_distance_key_order_with_ties() {
    let mut seed = 0x46ab_9013u32;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for n in [0, 1, 2, 3, 10, 300, 10000] {
        let mut values = Vec::new();
        for number in 0..n {
            let i = random();
            let distance = (i % 32) as f64 * 0.25;
            // x is unique; numerous values still have the same distance.
            let key = (number - 5000, (i as i32 % 31) - 15, (i as i32 % 19) - 9);
            values.push(Visible {
                distance: distance.to_bits(),
                key,
                slot: i,
                origin: [i as f32, (i >> 8) as f32, 0.],
            });
        }
        let mut expected = values.clone();
        expected.sort_unstable_by_key(|v| (v.distance, v.key));
        let mut scratch = Vec::new();
        radix_distance_then_key(&mut values, &mut scratch);
        assert_eq!(values, expected, "n={n}");
        values.reverse();
        radix_distance_then_key(&mut values, &mut scratch);
        assert_eq!(values, expected, "reused scratch n={n}");
    }
}

#[test]
fn adaptive_sort_production_entry_matches_old_across_frame_reuse() {
    for sparse in [false, true] {
        let r = populate(sparse);
        let projection = super::super::world_projection(70., 16. / 9., 1100.);
        let mut scratch = Vec::new();
        let mut optimized = Vec::new();
        let mut old = Vec::new();
        for frame in 0..24u32 {
            let (yaw, pitch, rd) = match frame % 4 {
                0 => (20., 15., 16),
                1 => (20., 15., 32),
                2 => (65., -40., 32),
                _ => (30., 85., 32),
            };
            let camera = DVec3::new(
                8.5 + (frame as f64 - 12.) * 9.,
                100. - (frame as f64 * 3.),
                8.5,
            );
            let frustum = Frustum {
                planes: super::super::frustum(projection * crate::camera::view_matrix(yaw, pitch)),
            };
            optimized.clear();
            old.clear();
            if frame % 3 == 0 {
                // The old API sorts any already present entries alongside
                // appended sections. Preserve that unusual but real contract.
                let sentinel = Visible {
                    distance: 4.0f64.to_bits(),
                    key: (999, -1, -999),
                    slot: u32::MAX,
                    origin: [0.25, 0.5, 0.75],
                };
                optimized.push(sentinel);
                old.push(sentinel);
            }
            r.visible(
                camera,
                rd,
                &frustum,
                |key| (key.0 + key.1 + key.2).rem_euclid(7) != 0,
                |x, z| (x - z).rem_euclid(11) == 0,
                &mut old,
            );
            r.visible_with_sort_scratch(
                camera,
                rd,
                &frustum,
                |key| (key.0 + key.1 + key.2).rem_euclid(7) != 0,
                |x, z| (x - z).rem_euclid(11) == 0,
                &mut optimized,
                &mut scratch,
            );
            assert_eq!(optimized, old, "frame={frame} sparse={sparse}");
        }
    }
}

/// This tests the actual production-facing API, including its size switch,
/// reusable output capacity and scratch lifetime; not only its sorter.
#[test]
#[ignore = "optimized CPU microbench: --profile fast -- --ignored --nocapture"]
fn visibility_end_to_end_sort_strategy() {
    for sparse in [false, true] {
        let layout = if sparse { "sparse" } else { "dense" };
        let resident = populate(sparse);
        let camera = DVec3::new(8.5, 100., 8.5);
        let projection = super::super::world_projection(70., 16. / 9., 1100.);
        for (name, yaw, pitch, rd) in [
            ("meadow_rd16", 20., 15., 16),
            ("meadow_rd32", 20., 15., 32),
            ("peak_rd32", 65., -40., 32),
            ("top_down_rd32", 30., 85., 32),
        ] {
            let frustum = Frustum {
                planes: super::super::frustum(projection * crate::camera::view_matrix(yaw, pitch)),
            };
            let mut reference = Vec::new();
            let mut candidate = Vec::new();
            let mut scratch = Vec::new();
            for _ in 0..8 {
                reference.clear();
                candidate.clear();
                resident.visible(camera, rd, &frustum, |_| true, |_, _| false, &mut reference);
                resident.visible_with_sort_scratch(
                    camera,
                    rd,
                    &frustum,
                    |_| true,
                    |_, _| false,
                    &mut candidate,
                    &mut scratch,
                );
            }
            assert_eq!(reference, candidate);
            let mut old_samples = Vec::new();
            let mut new_samples = Vec::new();
            for i in 0..40 {
                for optimized_first in [i % 2 == 0, i % 2 != 0] {
                    let at = Instant::now();
                    if optimized_first {
                        candidate.clear();
                        resident.visible_with_sort_scratch(
                            camera,
                            rd,
                            &frustum,
                            |_| true,
                            |_, _| false,
                            &mut candidate,
                            &mut scratch,
                        );
                        new_samples.push(at.elapsed().as_secs_f64() * 1e6);
                        black_box(candidate.len());
                    } else {
                        reference.clear();
                        resident.visible(
                            camera,
                            rd,
                            &frustum,
                            |_| true,
                            |_, _| false,
                            &mut reference,
                        );
                        old_samples.push(at.elapsed().as_secs_f64() * 1e6);
                        black_box(reference.len());
                    }
                }
            }
            assert_eq!(reference, candidate);
            let old = stage_quantiles(&mut old_samples);
            let new = stage_quantiles(&mut new_samples);
            println!(
                "visibility_end_to_end,layout={layout},scene={name},resident={},visible={},original_p50_us={:.3},adaptive_p50_us={:.3},original_p95_us={:.3},adaptive_p95_us={:.3}",
                resident.len(),
                reference.len(),
                old.0,
                new.0,
                old.1,
                new.1
            );
        }
    }
}

fn compare_key(a: &Visible, b: &Visible) -> std::cmp::Ordering {
    a.distance.cmp(&b.distance).then_with(|| a.key.cmp(&b.key))
}

#[test]
fn equivalent_sorters_keep_distance_and_key_order() {
    for sparse in [false, true] {
        let r = populate(sparse);
        let cam = DVec3::new(8.5, 100., 8.5);
        let proj = super::super::world_projection(70., 16. / 9., 1100.);
        let f = Frustum {
            planes: super::super::frustum(proj * crate::camera::view_matrix(65., -40.)),
        };
        let mut records = Vec::new();
        r.gather_visible(cam, 32, &f, |_| true, |_, _| false, &mut records);
        let mut base = records.clone();
        base.sort_unstable_by_key(|v| (v.distance, v.key));
        let mut cmp = records.clone();
        cmp.sort_unstable_by(compare_key);
        let mut stable = records.clone();
        stable.sort_by_cached_key(|v| (v.distance, v.key));
        assert_eq!(base, cmp);
        assert_eq!(base, stable);
        assert!(base.windows(2).all(|a| compare_key(&a[0], &a[1]).is_le()));
    }
}

/// Read production CPU work shares before choosing a new sort algorithm.
/// Compares identical data and keeps all sorting variants test-only.
#[test]
#[ignore = "optimized CPU microbench: --profile fast -- --ignored --nocapture"]
fn visibility_stage_costs() {
    for sparse in [false, true] {
        let layout = if sparse { "sparse" } else { "dense" };
        let r = populate(sparse);
        let cam = DVec3::new(8.5, 100., 8.5);
        for (scene, yaw, pitch, rd) in [
            ("meadow_rd16", 20., 15., 16),
            ("meadow_rd32", 20., 15., 32),
            ("peak_rd32", 65., -40., 32),
            ("top_down_rd32", 30., 85., 32),
        ] {
            let proj = super::super::world_projection(70., 16. / 9., 1100.);
            let f = Frustum {
                planes: super::super::frustum(proj * crate::camera::view_matrix(yaw, pitch)),
            };
            let mut unsorted = Vec::new();
            let mut full = Vec::new();
            for _ in 0..6 {
                unsorted.clear();
                r.gather_visible(cam, rd, &f, |_| true, |_, _| false, &mut unsorted);
                full.clear();
                r.visible(cam, rd, &f, |_| true, |_, _| false, &mut full);
            }
            let mut expected = unsorted.clone();
            expected.sort_unstable_by_key(|v| (v.distance, v.key));
            assert_eq!(expected, full);
            let mut gather = Vec::new();
            let mut sorting = Vec::new();
            let mut total = Vec::new();
            let mut baseline = Vec::new();
            for i in 0..36 {
                // Rotate the starting operation to counter thermal/cache drift.
                if i % 2 == 0 {
                    let t = Instant::now();
                    unsorted.clear();
                    r.gather_visible(cam, rd, &f, |_| true, |_, _| false, &mut unsorted);
                    gather.push(t.elapsed().as_secs_f64() * 1e6);
                    let t = Instant::now();
                    unsorted.sort_unstable_by_key(|v| (v.distance, v.key));
                    sorting.push(t.elapsed().as_secs_f64() * 1e6);
                    let t = Instant::now();
                    full.clear();
                    r.visible(cam, rd, &f, |_| true, |_, _| false, &mut full);
                    total.push(t.elapsed().as_secs_f64() * 1e6);
                } else {
                    let t = Instant::now();
                    full.clear();
                    r.visible(cam, rd, &f, |_| true, |_, _| false, &mut full);
                    total.push(t.elapsed().as_secs_f64() * 1e6);
                    let t = Instant::now();
                    unsorted.clear();
                    r.gather_visible(cam, rd, &f, |_| true, |_, _| false, &mut unsorted);
                    gather.push(t.elapsed().as_secs_f64() * 1e6);
                    let t = Instant::now();
                    unsorted.sort_unstable_by_key(|v| (v.distance, v.key));
                    sorting.push(t.elapsed().as_secs_f64() * 1e6);
                }
                assert_eq!(unsorted, full);
                black_box(full.len());
            }
            let a = stage_quantiles(&mut gather);
            let b = stage_quantiles(&mut sorting);
            let c = stage_quantiles(&mut total);
            println!(
                "visibility_stage,layout={layout},scene={scene},resident={},visible={},gather_p50_us={:.3},gather_p95_us={:.3},sort_p50_us={:.3},sort_p95_us={:.3},total_p50_us={:.3},total_p95_us={:.3}",
                r.len(),
                full.len(),
                a.0,
                a.1,
                b.0,
                b.1,
                c.0,
                c.1
            );
            // Sorting algorithms receive the *same unsorted sequence* and a
            // reusable vector allocated outside timing; never sort an already
            // sorted `full`, which would invalidate comparisons.
            baseline.clear();
            r.gather_visible(cam, rd, &f, |_| true, |_, _| false, &mut baseline);
            let mut by_key = baseline.clone();
            let mut cmp = baseline.clone();
            let mut cached = baseline.clone();
            let mut radix = baseline.clone();
            let mut radix_scratch = Vec::with_capacity(baseline.len());
            let mut times_key = Vec::new();
            let mut times_cmp = Vec::new();
            let mut times_cached = Vec::new();
            let mut times_radix = Vec::new();
            for round in 0..36 {
                for order in 0..4 {
                    match (round + order) % 4 {
                        0 => {
                            by_key.clone_from(&baseline);
                            let t = Instant::now();
                            by_key.sort_unstable_by_key(|v| (v.distance, v.key));
                            times_key.push(t.elapsed().as_secs_f64() * 1e6);
                        }
                        1 => {
                            cmp.clone_from(&baseline);
                            let t = Instant::now();
                            cmp.sort_unstable_by(compare_key);
                            times_cmp.push(t.elapsed().as_secs_f64() * 1e6);
                        }
                        2 => {
                            cached.clone_from(&baseline);
                            let t = Instant::now();
                            cached.sort_by_cached_key(|v| (v.distance, v.key));
                            times_cached.push(t.elapsed().as_secs_f64() * 1e6);
                        }
                        _ => {
                            radix.clone_from(&baseline);
                            let t = Instant::now();
                            radix_distance_then_key(&mut radix, &mut radix_scratch);
                            times_radix.push(t.elapsed().as_secs_f64() * 1e6);
                        }
                    }
                }
            }
            assert_eq!(by_key, cmp);
            assert_eq!(by_key, cached);
            assert_eq!(by_key, radix);
            assert_eq!(by_key, full);
            let key = stage_quantiles(&mut times_key);
            let cmp_t = stage_quantiles(&mut times_cmp);
            let cached_t = stage_quantiles(&mut times_cached);
            let radix_t = stage_quantiles(&mut times_radix);
            println!(
                "visibility_sorters,layout={layout},scene={scene},visible={},key_p50_us={:.3},compare_p50_us={:.3},cached_key_p50_us={:.3},radix_p50_us={:.3},key_p95_us={:.3},compare_p95_us={:.3},cached_key_p95_us={:.3},radix_p95_us={:.3}",
                baseline.len(),
                key.0,
                cmp_t.0,
                cached_t.0,
                radix_t.0,
                key.1,
                cmp_t.1,
                cached_t.1,
                radix_t.1
            );
        }
    }
}
