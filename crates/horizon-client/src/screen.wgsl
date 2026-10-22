// Full-screen camera effects (screen_effects.rs): clip-space quads sampling
// the shared 2D atlas with repeat inside each sprite rectangle.
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) @interpolate(flat) sampling: vec2<f32>,
}
@vertex fn vs_screen(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>,
    @location(2) rect: vec4<f32>, @location(3) color: vec4<f32>,
    @location(4) sampling: vec2<f32>) -> Out {
    var out: Out;
    out.clip = vec4(pos, 0.0, 1.0);
    out.uv = uv;
    out.rect = rect;
    out.color = color;
    out.sampling = sampling;
    return out;
}
// A standalone texture is packed into a larger atlas. Hardware repeat/linear
// filters would repeat the WHOLE atlas or blend neighbouring unrelated sprites.
// Resolve each texel inside this sprite's own bounds instead, including the
// four independent texels of a bilinear sample across its wrap boundary.
fn texel(in: Out, index: vec2<i32>) -> vec4<f32> {
    let dimensions = vec2<f32>(textureDimensions(atlas));
    let origin = vec2<i32>(round(in.rect.xy * dimensions));
    let extent = max(vec2<i32>(round((in.rect.zw - in.rect.xy) * dimensions)), vec2<i32>(1));
    var at = index;
    if in.sampling.y > 0.5 {
        at = clamp(at, vec2<i32>(0), extent - vec2<i32>(1));
    } else {
        at = ((at % extent) + extent) % extent;
    }
    return textureLoad(atlas, origin + at, 0);
}
fn sample(in: Out) -> vec4<f32> {
    let dimensions = vec2<f32>(textureDimensions(atlas));
    let extent = max(round((in.rect.zw - in.rect.xy) * dimensions), vec2<f32>(1.0));
    let pixel = in.uv * extent;
    if in.sampling.x > 0.5 {
        let shifted = pixel - vec2<f32>(0.5);
        let origin = vec2<i32>(floor(shifted));
        let f = fract(shifted);
        let top = mix(texel(in, origin), texel(in, origin + vec2<i32>(1, 0)), f.x);
        let bottom = mix(texel(in, origin + vec2<i32>(0, 1)),
                         texel(in, origin + vec2<i32>(1, 1)), f.x);
        return mix(top, bottom, f.y) * in.color;
    }
    return texel(in, vec2<i32>(floor(pixel))) * in.color;
}
@fragment fn fs_screen(in: Out) -> @location(0) vec4<f32> {
    let c = sample(in);
    if c.a <= 0.0 { discard; }
    return c;
}
// RenderPipelines.VIGNETTE blends ZERO, ONE_MINUS_SRC_COLOR: output the
// texture colour; the blend state darkens the frame by it.
@fragment fn fs_vignette(in: Out) -> @location(0) vec4<f32> {
    return vec4(sample(in).rgb, 1.0);
}
