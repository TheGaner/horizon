//! Full-screen camera effects drawn between the world and the HUD:
//! `ScreenEffectRenderer` (in-wall block texture, underwater overlay, fire)
//! and the full-screen `Hud` camera overlays laid out by
//! `hud_overlays::camera_overlays` (vignette, pumpkin blur, powder-snow
//! outline, nether portal swirl). Positions are clip-space quads; textures
//! come from the shared 2D atlas and may repeat.
use crate::{
    hud::Rect,
    hud_layout::{GuiDraw, IDENTITY, Pose, pose_apply},
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    /// `GUI_TEXTURED` / screen effects: alpha blending.
    Alpha,
    /// `RenderPipelines.VIGNETTE`: `dst * (1 - src)`.
    Vignette,
    /// Original 26.3 `GUI_NAUSEA_OVERLAY`: `BlendFunction.ADDITIVE`, ONE + ONE.
    Nausea,
}
/// One clip-space vertex of a screen quad.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ScreenVertex {
    pub pos: [f32; 2],
    /// Texture coordinates in sprite units (repeat outside 0..1).
    pub uv: [f32; 2],
    /// Atlas rectangle (u0, v0, u1, v1) of the sprite.
    pub rect: [f32; 4],
    pub color: [f32; 4],
    /// TextureMetadataSection blur/clamp (0 or 1) resolved per sprite.
    pub sampling: [f32; 2],
}
#[derive(Clone, Debug)]
pub struct ScreenQuad {
    pub vertices: [ScreenVertex; 4],
    pub blend: Blend,
    /// Source #1190: 0..3 metadata blur/clamp pipeline specialization.
    pub sampling: u32,
}

pub const SAMPLE_BLUR: u32 = 1;
pub const SAMPLE_CLAMP: u32 = 2;
fn sampling_bits(flags: [f32; 2]) -> u32 {
    u32::from(flags[0] > 0.5) * SAMPLE_BLUR | u32::from(flags[1] > 0.5) * SAMPLE_CLAMP
}

/// What the camera sees this frame.
pub struct ScreenInputs<'a> {
    pub sprites: &'a HashMap<String, Rect>,
    pub sampling: &'a HashMap<String, crate::hud::TextureSampling>,
    pub aspect: f32,
    pub partial: f32,
    /// Particle sprite of the view-blocking block around the eye.
    pub block_overlay: Option<String>,
    /// Eye in water: (lightmap brightness at the eye, yaw, pitch).
    pub water: Option<(f32, f32, f32)>,
    pub on_fire: bool,
    /// `Hud.extractCameraOverlays` draws: the full-screen blits become
    /// screen quads (the spyglass and its fills do not).
    pub gui_overlays: &'a [GuiDraw],
}

/// Effect state advanced every tick.
#[derive(Clone, Copy, Debug)]
pub struct ScreenEffects {
    /// `Hud.vignetteBrightness`.
    pub vignette_brightness: f32,
    pub portal: f32,
    pub o_portal: f32,
    /// Nausea MobEffectInstance.BlendState: previous and current tick values.
    pub nausea_factor: f32,
    pub nausea_previous: f32,
}
impl Default for ScreenEffects {
    fn default() -> Self {
        Self {
            vignette_brightness: 1.0,
            portal: 0.0,
            o_portal: 0.0,
            nausea_factor: 0.0,
            nausea_previous: 0.0,
        }
    }
}
/// `Lightmap.getBrightness`.
pub fn lightmap_brightness(ambient_light: f32, level: u8) -> f32 {
    let v = f32::from(level) / 15.0;
    let curved = v / (4.0 - 3.0 * v);
    curved + (1.0 - curved) * ambient_light
}
/// Clip-space scale of the hand projection (fov 70) at view depth 0.5.
fn hand_scale(aspect: f32) -> [f32; 2] {
    let half = 0.5 * (35.0f32).to_radians().tan();
    [1.0 / (half * aspect), 1.0 / half]
}
impl ScreenEffects {
    /// `Hud.updateVignetteBrightness` and `LocalPlayer.handlePortalTransitionEffect`.
    pub fn tick(&mut self, eye_brightness: f32, inside_portal: bool, nausea_duration: Option<i32>) {
        let target = (1.0 - eye_brightness).clamp(0.0, 1.0);
        self.vignette_brightness += (target - self.vignette_brightness) * 0.01;
        self.o_portal = self.portal;
        let step = if inside_portal {
            0.0125
        } else if self.portal > 0.0 {
            -0.05
        } else {
            0.0
        };
        self.portal = (self.portal + step).clamp(0.0, 1.0);
        self.nausea_previous = self.nausea_factor;
        if let Some(duration) = nausea_duration {
            // Java 26.3 MobEffects.NAUSEA.setBlendDuration(150, 20, 60).
            // BlendState.tick targets 0 in the final 60 ticks, then advances
            // by 1 / blend-out time; infinite effects stay fully blended.
            let active = duration == -1 || duration > 60;
            let target = if active { 1.0 } else { 0.0 };
            let delta = 1.0 / if active { 150.0 } else { 20.0 };
            self.nausea_factor += (target - self.nausea_factor).clamp(-delta, delta);
        } else {
            // LivingEntity.getEffectBlendFactor returns zero with no effect.
            self.nausea_factor = 0.0;
            self.nausea_previous = 0.0;
        }
    }
    pub fn nausea(&self, partial: f32) -> f32 {
        (self.nausea_previous + (self.nausea_factor - self.nausea_previous) * partial)
            .clamp(0.0, 1.0)
    }
    pub fn build(&self, input: &ScreenInputs) -> Vec<ScreenQuad> {
        let mut out = Vec::new();
        let sprite = |name: &str| input.sprites.get(name).map(|r| [r.u0, r.v0, r.u1, r.v1]);
        let sampling = |name: &str| input.sampling.get(name).copied().unwrap_or_default();
        let flags = |name: &str| {
            let flags = sampling(name);
            [f32::from(flags.blur), f32::from(flags.clamp)]
        };
        let scale = hand_scale(input.aspect);
        // Screen-effect quads live at view depth 0.5 in the hand projection.
        let hand_quad = |corners: [[f32; 2]; 4],
                         uv: [[f32; 2]; 4],
                         rect: [f32; 4],
                         color: [f32; 4],
                         sampling: [f32; 2]| ScreenQuad {
            vertices: std::array::from_fn(|i| ScreenVertex {
                pos: [corners[i][0] * scale[0], corners[i][1] * scale[1]],
                uv: uv[i],
                rect,
                color,
                sampling,
            }),
            blend: Blend::Alpha,
            sampling: sampling_bits(sampling),
        };
        let full = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
        // ScreenEffectRenderer.submitBlockSprite: u1/v1 at (-1, -1), colour 0xFF191919.
        if let Some(name) = &input.block_overlay
            && let Some(rect) = sprite(name)
        {
            let c = 0x19 as f32 / 255.0;
            out.push(hand_quad(
                full,
                [[1.0, 1.0], [0.0, 1.0], [0.0, 0.0], [1.0, 0.0]],
                rect,
                [c, c, c, 1.0],
                [0.0; 2],
            ));
        }
        if let Some((brightness, yaw, pitch)) = input.water
            && let Some(rect) = sprite("misc/underwater")
        {
            let (u, v) = (-yaw / 64.0, pitch / 64.0);
            let uv = [[u + 4.0, v + 4.0], [u, v + 4.0], [u, v], [u + 4.0, v]];
            out.push(hand_quad(
                full,
                uv,
                rect,
                [brightness, brightness, brightness, 0.1],
                flags("misc/underwater"),
            ));
        }
        if input.on_fire
            && let Some(rect) = sprite("block/fire_1")
        {
            for (dx, angle) in [(0.24f32, -10.0f32), (-0.24, 10.0)] {
                let (s, c) = angle.to_radians().sin_cos();
                // rotateY about the quad then translate; z = -0.5 feeds into x.
                let corner = |x: f32, y: f32| [x * c + -0.5 * s + dx, y - 0.3];
                let corners = [
                    corner(-0.5, -0.5),
                    corner(0.5, -0.5),
                    corner(0.5, 0.5),
                    corner(-0.5, 0.5),
                ];
                let alpha = 0xE5 as f32 / 255.0;
                out.push(hand_quad(
                    corners,
                    [[1.0, 1.0], [0.0, 1.0], [0.0, 0.0], [1.0, 0.0]],
                    rect,
                    [1.0, 1.0, 1.0, alpha],
                    [0.0; 2],
                ));
            }
        }
        // Hud camera overlays: GUI-space full-screen blits (v grows downwards).
        // GuiGraphicsExtractor applies a Matrix3x2fStack to nausea's
        // centred fullscreen quad. Convert its GUI-space transformed corners
        // to clip space, keeping the texture UVs independent of the pose.
        let gui = |rect: [f32; 4],
                   color: [f32; 4],
                   blend: Blend,
                   sampling: [f32; 2],
                   dimensions: [f32; 2],
                   pose: Pose| ScreenQuad {
            vertices: std::array::from_fn(|i| {
                let [w, h] = dimensions;
                let [x, y] = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]][i];
                let transformed_x = pose[0] * x + pose[2] * y + pose[4];
                let transformed_y = pose[1] * x + pose[3] * y + pose[5];
                ScreenVertex {
                    pos: [transformed_x * 2.0 / w - 1.0, 1.0 - transformed_y * 2.0 / h],
                    uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]][i],
                    rect,
                    color,
                    sampling,
                }
            }),
            blend,
            sampling: sampling_bits(sampling),
        };
        let rgba = |c: u32| {
            let [a, r, g, b] = c.to_be_bytes();
            [r, g, b, a].map(|v| f32::from(v) / 255.0)
        };
        let mut poses = vec![IDENTITY];
        for d in input.gui_overlays {
            match d {
                GuiDraw::Push(ops) => {
                    let mut pose = *poses.last().unwrap_or(&IDENTITY);
                    for op in ops {
                        pose_apply(&mut pose, *op);
                    }
                    poses.push(pose);
                }
                GuiDraw::Pop => {
                    if poses.len() > 1 {
                        poses.pop();
                    }
                }
                GuiDraw::Blit {
                    texture,
                    blend,
                    x: 0,
                    y: 0,
                    w,
                    h,
                    color,
                    ..
                } if texture != "misc/spyglass_scope" => {
                    let blend = match blend {
                        crate::hud_layout::Blend::Vignette => Blend::Vignette,
                        crate::hud_layout::Blend::Nausea => Blend::Nausea,
                        _ => Blend::Alpha,
                    };
                    // Latest Source: a zero-colour vignette is a no-op.
                    if blend == Blend::Vignette && color & 0x00ff_ffff == 0 {
                        continue;
                    }
                    if let Some(rect) = sprite(texture)
                        && *w > 0
                        && *h > 0
                    {
                        out.push(gui(
                            rect,
                            rgba(*color),
                            blend,
                            flags(texture),
                            [*w as f32, *h as f32],
                            *poses.last().unwrap_or(&IDENTITY),
                        ));
                    }
                }
                GuiDraw::AtlasSprite {
                    sprite: name,
                    color,
                    w,
                    h,
                    ..
                } => {
                    if let Some(rect) = sprite(name)
                        && *w > 0
                        && *h > 0
                    {
                        out.push(gui(
                            rect,
                            rgba(*color),
                            Blend::Alpha,
                            flags(name),
                            [*w as f32, *h as f32],
                            *poses.last().unwrap_or(&IDENTITY),
                        ));
                    }
                }
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lightmap_brightness_curve() {
        assert_eq!(lightmap_brightness(0.0, 15), 1.0);
        assert_eq!(lightmap_brightness(0.0, 0), 0.0);
        assert!((lightmap_brightness(0.1, 0) - 0.1).abs() < 1e-6);
    }
    #[test]
    fn portal_intensity_ramps() {
        let mut e = ScreenEffects::default();
        for _ in 0..80 {
            e.tick(1.0, true, None);
        }
        assert!((e.portal - 1.0).abs() < 1e-5);
        e.tick(1.0, false, None);
        assert!((e.portal - 0.95).abs() < 1e-5);
        // Vignette fades towards 1 - brightness at 1% per tick.
        assert!(e.vignette_brightness < 1.0);
    }
    #[test]
    fn shader_understands_sprite_local_bilinear_and_wrap_sampling() {
        let shader = naga::front::wgsl::parse_str(include_str!("screen.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&shader)
        .unwrap();
    }

    #[test]
    fn blur_and_clamp_are_independent_for_each_overlay() {
        let sprites = HashMap::from([
            (
                "misc/vignette".to_owned(),
                Rect {
                    u0: 0.1,
                    v0: 0.1,
                    u1: 0.2,
                    v1: 0.2,
                    w: 16,
                    h: 16,
                },
            ),
            (
                "misc/underwater".to_owned(),
                Rect {
                    u0: 0.3,
                    v0: 0.3,
                    u1: 0.4,
                    v1: 0.4,
                    w: 16,
                    h: 16,
                },
            ),
        ]);
        let sampling = HashMap::from([
            (
                "misc/vignette".to_owned(),
                crate::hud::TextureSampling {
                    blur: true,
                    clamp: false,
                },
            ),
            (
                "misc/underwater".to_owned(),
                crate::hud::TextureSampling {
                    blur: false,
                    clamp: true,
                },
            ),
        ]);
        let input = ScreenInputs {
            sprites: &sprites,
            sampling: &sampling,
            aspect: 1.6,
            partial: 0.0,
            block_overlay: None,
            water: Some((1.0, 0.0, 0.0)),
            on_fire: false,
            gui_overlays: &[crate::hud_overlays::vignette(320, 240, 1.0, 0.0)],
        };
        let quads = ScreenEffects::default().build(&input);
        assert_eq!(quads.len(), 2);
        assert_eq!(quads[0].vertices[0].sampling, [0.0, 1.0]);
        assert_eq!(quads[1].vertices[0].sampling, [1.0, 0.0]);
    }

    #[test]
    fn vignette_sampling_is_read_from_the_texture_not_the_pipeline() {
        let sprites = HashMap::from([(
            "misc/vignette".to_owned(),
            Rect {
                u0: 0.125,
                v0: 0.25,
                u1: 0.375,
                v1: 0.75,
                w: 16,
                h: 16,
            },
        )]);
        let sampling = HashMap::from([(
            "misc/vignette".to_owned(),
            crate::hud::TextureSampling {
                blur: true,
                clamp: true,
            },
        )]);
        let input = ScreenInputs {
            sprites: &sprites,
            sampling: &sampling,
            aspect: 1.6,
            partial: 0.0,
            block_overlay: None,
            water: None,
            on_fire: false,
            gui_overlays: &[crate::hud_overlays::vignette(320, 240, 1.0, 0.0)],
        };
        let quad = ScreenEffects::default().build(&input).remove(0);
        assert_eq!(quad.vertices[0].sampling, [1.0, 1.0]);
        assert_eq!(quad.blend, Blend::Vignette);
    }

    #[test]
    fn nausea_blend_matches_java_26_3_150_20_60_tick_schedule() {
        let mut e = ScreenEffects::default();
        e.tick(1.0, false, Some(300));
        assert!((e.nausea(0.0) - 0.0).abs() < 1e-6);
        assert!((e.nausea(0.5) - 0.5 / 150.0).abs() < 1e-6);
        assert!((e.nausea(1.0) - 1.0 / 150.0).abs() < 1e-6);
        for _ in 1..150 {
            e.tick(1.0, false, Some(300));
        }
        assert!((e.nausea(1.0) - 1.0).abs() < 1e-5);
        e.tick(1.0, false, Some(60));
        assert!((e.nausea(0.0) - 1.0).abs() < 1e-5);
        assert!((e.nausea(1.0) - 0.95).abs() < 1e-5);
        for _ in 1..20 {
            e.tick(1.0, false, Some(60));
        }
        assert!(e.nausea(1.0).abs() < 1e-5);
        e.tick(1.0, false, Some(-1));
        assert!((e.nausea(1.0) - 1.0 / 150.0).abs() < 1e-6);
        e.tick(1.0, false, None);
        assert_eq!(e.nausea(1.0), 0.0);
    }

    #[test]
    fn nausea_reads_pack_sampling_uses_additive_blend_and_scales_about_screen_center() {
        let sprites = HashMap::from([(
            "misc/nausea".to_owned(),
            Rect {
                u0: 0.125,
                v0: 0.25,
                u1: 0.375,
                v1: 0.5,
                w: 128,
                h: 128,
            },
        )]);
        let sampling = HashMap::from([(
            "misc/nausea".to_owned(),
            crate::hud::TextureSampling {
                blur: true,
                clamp: false,
            },
        )]);
        let overlays = crate::hud_overlays::confusion_overlay(320, 240, 0.5);
        let input = ScreenInputs {
            sprites: &sprites,
            sampling: &sampling,
            aspect: 4.0 / 3.0,
            partial: 0.0,
            block_overlay: None,
            water: None,
            on_fire: false,
            gui_overlays: &overlays,
        };
        let quads = ScreenEffects::default().build(&input);
        assert_eq!(quads.len(), 1, "Horizon must not drop the nausea overlay");
        let q = &quads[0];
        assert_eq!(q.blend, Blend::Nausea);
        assert_eq!(q.vertices[0].sampling, [1.0, 0.0]);
        assert_eq!(q.vertices[0].rect, [0.125, 0.25, 0.375, 0.5]);
        for (vertex, pos) in
            q.vertices
                .iter()
                .zip([[-1.5, 1.5], [1.5, 1.5], [1.5, -1.5], [-1.5, -1.5]])
        {
            assert!((vertex.pos[0] - pos[0]).abs() < 1e-5);
            assert!((vertex.pos[1] - pos[1]).abs() < 1e-5);
        }
    }

    #[test]
    fn nausea_pose_is_restored_before_subsequent_vignette() {
        let sprites = HashMap::from([
            (
                "misc/nausea".to_owned(),
                Rect {
                    u0: 0.0,
                    v0: 0.0,
                    u1: 0.5,
                    v1: 0.5,
                    w: 16,
                    h: 16,
                },
            ),
            (
                "misc/vignette".to_owned(),
                Rect {
                    u0: 0.5,
                    v0: 0.5,
                    u1: 1.0,
                    v1: 1.0,
                    w: 16,
                    h: 16,
                },
            ),
        ]);
        let mut overlays = crate::hud_overlays::confusion_overlay(320, 240, 0.75);
        overlays.push(crate::hud_overlays::vignette(320, 240, 1.0, 0.0));
        let input = ScreenInputs {
            sprites: &sprites,
            sampling: &HashMap::new(),
            aspect: 4.0 / 3.0,
            partial: 0.0,
            block_overlay: None,
            water: None,
            on_fire: false,
            gui_overlays: &overlays,
        };
        let quads = ScreenEffects::default().build(&input);
        assert_eq!(quads.len(), 2);
        assert_eq!(quads[1].blend, Blend::Vignette);
        assert_eq!(quads[1].vertices[0].pos, [-1.0, 1.0]);
        assert_eq!(quads[1].vertices[2].pos, [1.0, -1.0]);
    }

    #[test]
    fn overlays_need_their_sprites() {
        let mut sprites = HashMap::new();
        sprites.insert(
            "misc/vignette".to_owned(),
            Rect {
                u0: 0.0,
                v0: 0.0,
                u1: 1.0,
                v1: 1.0,
                w: 256,
                h: 256,
            },
        );
        let e = ScreenEffects::default();
        let input = ScreenInputs {
            sprites: &sprites,
            sampling: &HashMap::new(),
            aspect: 1.6,
            partial: 0.0,
            block_overlay: None,
            water: Some((1.0, 0.0, 0.0)),
            on_fire: false,
            gui_overlays: &[crate::hud_overlays::vignette(320, 240, 1.0, 0.0)],
        };
        let quads = e.build(&input);
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].blend, Blend::Vignette);
    }
}
