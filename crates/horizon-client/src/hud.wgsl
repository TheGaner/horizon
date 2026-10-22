struct Hud { size: vec4<f32> }
@group(0) @binding(0) var<uniform> hud: Hud;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) sample_rect: vec4<f32>,
    @location(3) @interpolate(flat) sample_flags: vec4<f32>,
}
@vertex fn vs_main(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>, @location(3) depth: f32,
    @location(4) sample_rect: vec4<f32>, @location(5) sample_flags: vec4<f32>) -> Out {
    let pixels = pos * hud.size.z;
    var out: Out;
    out.clip = vec4(pixels.x / hud.size.x * 2.0 - 1.0, 1.0 - pixels.y / hud.size.y * 2.0, depth, 1.0);
    out.uv = uv;
    out.color = color;
    out.sample_rect = sample_rect;
    out.sample_flags = sample_flags;
    return out;
}
// The texture is a rectangle of the shared HUD atlas. Sampling its own
// pixels manually is necessary for blurred texels at the boundary: a normal
// filtering sampler would bleed the neighbouring packed texture.
fn standalone_texel(in: Out, index: vec2<i32>) -> vec4<f32> {
    let atlas_size = vec2<f32>(textureDimensions(atlas));
    let corner = vec2<i32>(round(in.sample_rect.xy * atlas_size));
    let extent = max(vec2<i32>(round((in.sample_rect.zw - in.sample_rect.xy) * atlas_size)), vec2<i32>(1));
    var at = index;
    if in.sample_flags.z > 0.5 {
        at = clamp(at, vec2<i32>(0), extent - vec2<i32>(1));
    } else {
        at = ((at % extent) + extent) % extent;
    }
    return textureLoad(atlas, corner + at, 0);
}
fn hud_sample(in: Out, uv: vec2<f32>) -> vec4<f32> {
    if in.sample_flags.x < 0.5 {
        return textureSampleLevel(atlas, atlas_sampler, uv, 0.0);
    }
    let atlas_size = vec2<f32>(textureDimensions(atlas));
    let extent = max(round((in.sample_rect.zw - in.sample_rect.xy) * atlas_size), vec2<f32>(1.0));
    let sprite_uv = (uv - in.sample_rect.xy) / (in.sample_rect.zw - in.sample_rect.xy);
    let pixel = sprite_uv * extent;
    if in.sample_flags.y > 0.5 {
        let shifted = pixel - vec2<f32>(0.5);
        let origin = vec2<i32>(floor(shifted));
        let f = fract(shifted);
        let top = mix(standalone_texel(in, origin), standalone_texel(in, origin + vec2<i32>(1, 0)), f.x);
        let bottom = mix(standalone_texel(in, origin + vec2<i32>(0, 1)),
                         standalone_texel(in, origin + vec2<i32>(1, 1)), f.x);
        return mix(top, bottom, f.y);
    }
    return standalone_texel(in, vec2<i32>(floor(pixel)));
}

@fragment fn fs_main(in: Out) -> @location(0) vec4<f32> {
    // u >= 2: flat vertex colour masked by the texture's alpha (enchantment glint).
    if in.uv.x >= 2.0 {
        let m = hud_sample(in, in.uv - vec2(2.0, 0.0)).a * in.color.a;
        if m < 0.004 { discard; }
        return vec4(in.color.rgb, m);
    }
    let c = hud_sample(in, in.uv) * in.color;
    if c.a < 0.004 { discard; }
    return c;
}
@fragment fn fs_crosshair(in: Out) -> @location(0) vec4<f32> {
    let c = hud_sample(in, in.uv) * in.color;
    if c.a == 0.0 { discard; }
    return c;
}
