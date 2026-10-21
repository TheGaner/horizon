//! Reproducible GPU-free tests of the *real* FaceGroups::ranges → indirect
//! arguments path used by Renderer. A benchmark proxy, not an FPS test.
use super::*;
use crate::mesher::{FACE_GROUPS, FaceGroups, TerrainVertex};
use std::{hint::black_box, time::Instant};

#[derive(Clone, Copy)]
struct Item {
    faces: FaceGroups,
    start: u64,
    page: u32,
    origin: [f32; 3],
    first_phase: bool,
}

#[derive(Default, PartialEq, Eq, Debug)]
struct List {
    origins: Vec<[u32; 4]>,
    draws: Vec<[u32; 5]>,
    pages: Vec<(u32, Option<u32>)>,
    phase2: Vec<[u32; 5]>,
    phase2_pages: Vec<(u32, Option<u32>)>,
    phase1_count: usize,
}

impl List {
    fn clear(&mut self) {
        self.origins.clear();
        self.draws.clear();
        self.pages.clear();
        self.phase2.clear();
        self.phase2_pages.clear();
        self.phase1_count = 0;
    }
    fn page_runs(&self) -> usize {
        self.pages
            .iter()
            .enumerate()
            .filter(|(i, p)| *i == 0 || *p != &self.pages[*i - 1])
            .count()
    }
    fn indirect_bytes(&self) -> usize {
        self.draws.len() * std::mem::size_of::<[u32; 5]>()
    }
    fn instance_bytes(&self) -> usize {
        self.origins.len() * std::mem::size_of::<[u32; 4]>()
    }
}

fn quad(axis: usize, positive: bool, bent: bool) -> [TerrainVertex; 4] {
    let a = (axis + 1) % 3;
    let b = (axis + 2) % 3;
    let mut p = [7.0f32; 3];
    p[axis] = if positive { 12.0 } else { 4.0 };
    let mut p1 = p;
    p1[a] += 1.0;
    let mut p2 = p;
    p2[b] += 1.0;
    let mut p3 = p;
    p3[a] += 1.0;
    p3[b] += 1.0;
    if bent {
        p3[axis] += 0.375;
    }
    let points = if positive {
        [p, p1, p2, p3]
    } else {
        [p, p2, p1, p3]
    };
    points.map(|pos| TerrainVertex::new(pos, [0.0, 0.0], 0, 0xffffffff, 0xffff))
}

fn patterns() -> [FaceGroups; 5] {
    // Always create actual FaceGroups through the mesher's grouping routine.
    // These cover all seven groups, purely opaque, an unaligned model quad,
    // a lone side and sparse mixed geometry.
    let masks = [0b1111111u8, 0b1100111, 0b0001000, 0b1000000, 0b0111010];
    std::array::from_fn(|i| {
        let mut solid = Vec::new();
        for g in 0..FACE_GROUPS {
            if (masks[i] & (1 << g)) == 0 {
                continue;
            }
            // g3 is rotated/non-coplanar and must remain unaligned.
            let (axis, positive, bent) = match g {
                0 => (0, false, false),
                1 => (2, false, false),
                2 => (1, false, false),
                3 => (0, true, true),
                4 => (1, true, false),
                5 => (2, true, false),
                6 => (0, true, false),
                _ => unreachable!(),
            };
            solid.extend(quad(axis, positive, bent));
            if i == 0 && g == 3 {
                // Two genuine unaligned mesh quads exercise group merging.
                solid.extend(quad(axis, positive, bent));
            }
        }
        let faces = FaceGroups::group(&mut solid);
        assert_eq!(faces.quads.iter().sum::<u32>() as usize, solid.len() / 4);
        // Reject silently malformed synthetic quad orientation/grouping.
        if i == 0 {
            assert!(
                faces.quads.iter().all(|&q| q >= 1),
                "all 7 face groups must be populated: {:?}",
                faces.quads
            );
            assert_eq!(faces.quads[3], 2, "both unaligned quads must be retained");
        }
        if i == 2 {
            assert_eq!(faces.quads[3], 1);
        }
        faces
    })
}

fn sample(count: usize, seed: u32) -> Vec<Item> {
    let templates = patterns();
    (0..count)
        .map(|i| {
            let n = i as u32;
            let tile_x = (n.wrapping_mul(37).wrapping_add(seed) % 101) as f32 - 50.0;
            let tile_y = (n.wrapping_mul(11).wrapping_add(seed) % 24) as f32 - 8.0;
            let tile_z = (n.wrapping_mul(79).wrapping_add(seed) % 101) as f32 - 50.0;
            Item {
                faces: templates[((n.wrapping_mul(17).wrapping_add(seed)) % 5) as usize],
                // Fits the real indexed draw first_index u32 encoding.
                start: u64::from(n.wrapping_mul(1024)),
                page: (n / 37) % 12,
                origin: [tile_x * 16.0, tile_y * 16.0, tile_z * 16.0],
                first_phase: (n.wrapping_add(seed) % 7) != 0,
            }
        })
        .collect()
}

fn append_origins(items: &[Item], out: &mut List) {
    for s in items {
        out.origins
            .push(section_instance(s.origin).map(f32::to_bits));
    }
}

fn append_groups(items: &[Item], eye: Vec3, out: &mut List) {
    for (i, s) in items.iter().enumerate() {
        let (draws, pages) = if s.first_phase {
            (&mut out.draws, &mut out.pages)
        } else {
            (&mut out.phase2, &mut out.phase2_pages)
        };
        s.faces.ranges(eye - Vec3::from(s.origin), |first, quads| {
            draws.push([
                quads * 6,
                u32::from(s.first_phase),
                0,
                (s.start + u64::from(first) * 4) as u32,
                i as u32,
            ]);
            pages.push((s.page, None));
        });
    }
}

fn merge_phases(out: &mut List) {
    out.phase1_count = out.draws.len();
    out.draws.extend_from_slice(&out.phase2);
    out.pages.extend_from_slice(&out.phase2_pages);
}

fn build(items: &[Item], eye: Vec3, out: &mut List) {
    out.clear();
    append_origins(items, out);
    append_groups(items, eye, out);
    merge_phases(out);
}

/// A deliberately independent seven-group oracle, not FaceGroups::ranges.
/// It checks group skipping, adjacent-run joining and first-index offsets.
fn reference(items: &[Item], eye: Vec3) -> List {
    let mut out = List::default();
    append_origins(items, &mut out);
    for (i, s) in items.iter().enumerate() {
        let p = eye - Vec3::from(s.origin);
        let (draws, pages) = if s.first_phase {
            (&mut out.draws, &mut out.pages)
        } else {
            (&mut out.phase2, &mut out.phase2_pages)
        };
        let mut first = 0u32;
        let mut run = None::<(u32, u32)>;
        for g in 0..FACE_GROUPS {
            let quads = s.faces.quads[g];
            if quads > 0 {
                if s.faces.visible(g, p) {
                    let pair = run.unwrap_or((first, 0));
                    run = Some((pair.0, pair.1 + quads));
                } else if let Some((start, len)) = run.take() {
                    draws.push([
                        len * 6,
                        u32::from(s.first_phase),
                        0,
                        (s.start + u64::from(start) * 4) as u32,
                        i as u32,
                    ]);
                    pages.push((s.page, None));
                }
            }
            first += quads;
        }
        if let Some((start, len)) = run {
            draws.push([
                len * 6,
                u32::from(s.first_phase),
                0,
                (s.start + u64::from(start) * 4) as u32,
                i as u32,
            ]);
            pages.push((s.page, None));
        }
    }
    merge_phases(&mut out);
    out
}

fn p(samples: &mut [f64]) -> (f64, f64, f64) {
    samples.sort_by(f64::total_cmp);
    let x = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    (x(0.5), x(0.95), x(0.99))
}

#[test]
fn draw_list_cpu_exactly_matches_facegroup_oracle() {
    for count in [0, 1, 7, 128, 2965, 13680] {
        let items = sample(count, 37);
        for eye in [
            Vec3::ZERO,
            Vec3::splat(300.),
            Vec3::splat(-300.),
            Vec3::new(0.0, 9.0, 24.0),
        ] {
            let mut actual = List::default();
            build(&items, eye, &mut actual);
            assert_eq!(actual, reference(&items, eye), "count={count} eye={eye:?}");
            // The production path must tolerate scratch reuse as the camera moves.
            build(&items, eye + Vec3::new(16.0, -5.0, 2.0), &mut actual);
            assert_eq!(actual, reference(&items, eye + Vec3::new(16.0, -5.0, 2.0)));
            assert_eq!(actual.origins.len(), count);
            assert_eq!(actual.draws.len(), actual.pages.len());
        }
    }
}

#[test]
fn draw_list_cpu_all_seven_facing_groups_are_present() {
    let faces = patterns();
    assert!(faces[0].quads.iter().all(|&q| q >= 1));
    assert_eq!(faces[2].quads.iter().sum::<u32>(), 1);
    assert_eq!(faces[2].quads[3], 1);
}

/// Separately measure real FaceGroups traversal, origin construction, phase2
/// vector merge and full draw records; no GPU command encoding or upload.
#[test]
#[ignore = "optimized CPU test; cargo test --profile fast --lib draw_list_cpu_stage_costs -- --ignored --nocapture"]
fn draw_list_cpu_stage_costs() {
    for (name, count) in [
        ("small_rd8", 570),
        ("meadow_rd16", 2965),
        ("peak_rd32", 10707),
        ("meadow_rd32", 13680),
    ] {
        let items = sample(count, 37);
        for (view, eye) in [
            ("overview", Vec3::ZERO),
            ("off_axis", Vec3::new(220., -200., 310.)),
        ] {
            let mut out = List::default();
            let expected = reference(&items, eye);
            for _ in 0..5 {
                build(&items, eye, &mut out);
            }
            assert_eq!(out, expected);
            let mut origin_t = Vec::new();
            let mut face_t = Vec::new();
            let mut merge_t = Vec::new();
            let mut full_t = Vec::new();
            for i in 0..40 {
                for full_first in [i % 2 == 0, i % 2 != 0] {
                    if full_first {
                        let now = Instant::now();
                        build(&items, eye, &mut out);
                        full_t.push(now.elapsed().as_secs_f64() * 1e6);
                        black_box(out.draws.len());
                    } else {
                        out.clear();
                        let now = Instant::now();
                        append_origins(&items, &mut out);
                        origin_t.push(now.elapsed().as_secs_f64() * 1e6);
                        let now = Instant::now();
                        append_groups(&items, eye, &mut out);
                        face_t.push(now.elapsed().as_secs_f64() * 1e6);
                        let now = Instant::now();
                        merge_phases(&mut out);
                        merge_t.push(now.elapsed().as_secs_f64() * 1e6);
                        black_box(out.page_runs());
                    }
                }
            }
            assert_eq!(out, expected);
            let origins = p(&mut origin_t);
            let faces = p(&mut face_t);
            let merge = p(&mut merge_t);
            let full = p(&mut full_t);
            println!(
                "draw_cpu,scene={name},view={view},sections={count},draws={},phase2_draws={},page_runs={},instance_bytes={},indirect_bytes={},origins_p50_us={:.3},faces_p50_us={:.3},merge_p50_us={:.3},total_p50_us={:.3},faces_p95_us={:.3},total_p95_us={:.3}",
                out.draws.len(),
                out.phase2.len(),
                out.page_runs(),
                out.instance_bytes(),
                out.indirect_bytes(),
                origins.0,
                faces.0,
                merge.0,
                full.0,
                faces.1,
                full.1
            );
        }
    }
}

/// LOD draws must stay in phase 1 regardless of terrain occlusion state and
/// use instance IDs after the ordinary section list, as in `Renderer`.
fn append_lods(items: &[Item], eye: Vec3, instance_offset: usize, out: &mut List) {
    for (i, s) in items.iter().enumerate() {
        s.faces.ranges(eye - Vec3::from(s.origin), |first, quads| {
            out.draws.push([
                quads * 6,
                1,
                0,
                (s.start + u64::from(first) * 4) as u32,
                (instance_offset + i) as u32,
            ]);
            out.pages.push((s.page, None));
        });
    }
}

/// Independent reference for the distant-LOD fixed-first-phase draw path.
fn lod_reference(items: &[Item], eye: Vec3, instance_offset: usize) -> List {
    let mut out = List::default();
    append_origins(items, &mut out);
    for (i, s) in items.iter().enumerate() {
        let mut first = 0u32;
        let mut run: Option<(u32, u32)> = None;
        for group in 0..FACE_GROUPS {
            let count = s.faces.quads[group];
            if count > 0 {
                if s.faces.visible(group, eye - Vec3::from(s.origin)) {
                    let previous = run.unwrap_or((first, 0));
                    run = Some((previous.0, previous.1 + count));
                } else if let Some((start, len)) = run.take() {
                    out.draws.push([
                        len * 6,
                        1,
                        0,
                        (s.start + u64::from(start) * 4) as u32,
                        (instance_offset + i) as u32,
                    ]);
                    out.pages.push((s.page, None));
                }
            }
            first += count;
        }
        if let Some((start, len)) = run {
            out.draws.push([
                len * 6,
                1,
                0,
                (s.start + u64::from(start) * 4) as u32,
                (instance_offset + i) as u32,
            ]);
            out.pages.push((s.page, None));
        }
    }
    out.phase1_count = out.draws.len();
    out
}

#[test]
fn draw_list_distant_lod_first_phase_and_instance_offsets_match_reference() {
    for (total, offset) in [(0, 0), (1, 19), (40, 2965), (200, 13680)] {
        let items = sample(total, 67);
        for eye in [Vec3::ZERO, Vec3::new(-300.0, 600.0, 9.0)] {
            let mut actual = List::default();
            append_origins(&items, &mut actual);
            append_lods(&items, eye, offset, &mut actual);
            actual.phase1_count = actual.draws.len();
            let reference = lod_reference(&items, eye, offset);
            assert_eq!(actual, reference);
            assert_eq!(actual.draws.len(), actual.pages.len());
            assert!(actual.draws.iter().all(|d| d[1] == 1));
            assert!(actual.draws.iter().all(|d| (d[4] as usize) >= offset));
        }
    }
}
