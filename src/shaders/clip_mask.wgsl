// Viewport / XCLIP stencil-mask shader.
//
// Draws a clip-boundary polygon (as a triangle fan) into the stencil buffer so
// a paper content viewport can clip its render to an arbitrary shape. It writes
// no colour and no depth — only stencil (the pipeline uses `Invert`, so an
// even-odd fill marks the polygon interior regardless of convexity).
//
// The boundary arrives already projected into the content viewport's normalized
// device coordinates (paper-space shape mapped through the same visible-sub-rect
// crop as the content), so the vertex stage is a pure pass-through — the mask
// is fixed to the viewport frame and never moves with the model camera.

struct VIn {
    @location(0) ndc: vec2<f32>,
}

@vertex fn vs_main(in: VIn) -> @builtin(position) vec4<f32> {
    return vec4<f32>(in.ndc, 0.0, 1.0);
}

// Colour is write-masked off in the pipeline; this only satisfies the
// fragment-output requirement.
@fragment fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}
