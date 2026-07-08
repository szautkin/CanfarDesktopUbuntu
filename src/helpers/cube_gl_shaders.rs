//! GLSL 330-core shaders for the cube volume ray-marcher.
//!
//! Ported from `Services/CubeViewer/CubeVolumeShaders.cs` (HLSL SM5.0). A
//! fullscreen-triangle pass reconstructs the world ray from the inverse
//! view-projection, marches an `R32F` `sampler3D` front-to-back with a jittered
//! start and early ray termination, applies the shared stretch + opacity
//! transfer function + colormap, and supports a MIP mode.
//!
//! GL-vs-D3D adaptations (the "black screen" pitfalls):
//! * Clip-space depth: OpenGL uses z∈[-1,1] (D3D uses [0,1]) — the ray's near
//!   plane is reconstructed at **-1.0**, not 0.0. This is the single most common
//!   port bug.
//! * Matrices are column-major and used as `M * v` (uploaded with
//!   `transpose = GL_FALSE`), matching `cube_math`'s column-major `Mat4`.
//! * The caller MUST `glDisable(GL_CULL_FACE)` and set premultiplied over-blend
//!   `glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA)`.

/// Vertex shader: emits a single fullscreen triangle from `gl_VertexID`,
/// no vertex buffer required, and passes the clip-space NDC to the fragment stage.
pub const VERTEX_SRC: &str = r#"#version 330 core
out vec2 v_ndc;
void main() {
    vec2 uv  = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    vec2 ndc = uv * 2.0 - 1.0;
    v_ndc = ndc;
    gl_Position = vec4(ndc, 0.0, 1.0);
}
"#;

/// Fragment shader: the volume ray-march. Uniforms mirror `CubeUniforms`.
pub const FRAGMENT_SRC: &str = r#"#version 330 core
in vec2 v_ndc;
out vec4 fragColor;

uniform mat4 invViewProj;   // inverse(proj * view), column-major, used as M * v
uniform mat4 inverseModel;  // inverse(model)
uniform vec2 window;        // normalized lo, hi
uniform float steps;
uniform float density;
uniform float jitter;
uniform int   stretch;      // 0 Linear, 1 Log, 2 Sqrt, 3 Squared, 4 Asinh
uniform int   mip;

uniform sampler3D dataTex;  // R32F, normalized [0,1], NaN = blank
uniform sampler2D cmapTex;  // 256x1 RGBA colormap
uniform sampler2D tfTex;    // 256x1 R opacity ramp

float asinh_(float x) { return log(x + sqrt(x * x + 1.0)); }

float applyStretch(float x, int mode) {
    x = clamp(x, 0.0, 1.0);
    if (mode == 1) return log(1.0 + 9.0 * x) / log(10.0);      // Log
    if (mode == 2) return sqrt(x);                             // Sqrt
    if (mode == 3) return x * x;                               // Squared
    if (mode == 4) return asinh_(10.0 * x) / asinh_(10.0);     // Asinh
    return x;                                                  // Linear
}

vec2 hitBox(vec3 orig, vec3 dir) {
    vec3 invDir = 1.0 / dir;
    vec3 t0 = (vec3(-0.5) - orig) * invDir;
    vec3 t1 = (vec3( 0.5) - orig) * invDir;
    vec3 tmin = min(t0, t1);
    vec3 tmax = max(t0, t1);
    return vec2(max(max(tmin.x, tmin.y), tmin.z),
               min(min(tmax.x, tmax.y), tmax.z));
}

float hashf(vec2 p) { return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453); }

void main() {
    // Reconstruct the world ray. OpenGL clip-space depth is [-1, 1], so the near
    // plane is at z = -1.0 (NOT 0.0 as in the D3D/HLSL source).
    vec4 nearH = invViewProj * vec4(v_ndc, -1.0, 1.0);
    vec4 farH  = invViewProj * vec4(v_ndc,  1.0, 1.0);
    vec3 nearW = nearH.xyz / nearH.w;
    vec3 farW  = farH.xyz / farH.w;
    vec3 ro = (inverseModel * vec4(nearW, 1.0)).xyz;
    vec3 rd = normalize((inverseModel * vec4(farW - nearW, 0.0)).xyz);

    vec2 bounds = hitBox(ro, rd);
    bounds.x = max(bounds.x, 0.0);
    if (bounds.x >= bounds.y) discard;

    float dt = 1.7320508 / steps;   // unit-cube diagonal / steps
    float t  = bounds.x + dt * hashf(gl_FragCoord.xy + jitter);

    vec3 acc = vec3(0.0);
    float alpha = 0.0;
    float mipVal = 0.0;

    for (int i = 0; i < 2048; i++) {
        if (t > bounds.y || alpha > 0.98) break;
        if (float(i) >= steps * 1.7320508) break;
        vec3 p = ro + rd * t + 0.5;
        float r = texture(dataTex, p).r;
        if (r > 0.0) {
            float v = (r - window.x) / max(window.y - window.x, 1.0e-6);
            float s = applyStretch(v, stretch);
            if (mip == 1) {
                mipVal = max(mipVal, s);
            } else {
                float a = clamp(texture(tfTex, vec2(s, 0.5)).r * density * dt * 60.0, 0.0, 1.0);
                vec3 c = texture(cmapTex, vec2(s, 0.5)).rgb;
                acc += (1.0 - alpha) * a * c;
                alpha += (1.0 - alpha) * a;
            }
        }
        t += dt;
    }

    if (mip == 1) {
        if (mipVal <= 0.003) discard;
        float a = smoothstep(0.0, 0.25, mipVal);
        fragColor = vec4(texture(cmapTex, vec2(mipVal, 0.5)).rgb * a, a); // premultiplied
        return;
    }
    if (alpha <= 0.003) discard;
    fragColor = vec4(acc, alpha); // premultiplied
}
"#;
