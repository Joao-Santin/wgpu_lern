// Precisa bater com a struct Uniforms em main.rs. Usamos vec4 pra cada
// "grupo" de dados propositalmente: vec4 tem alinhamento de 16 bytes tanto
// em WGSL quanto no layout manual em Rust, então não existe padding
// implícito ambíguo entre os dois lados (foi exatamente esse padding
// implícito, com f32/vec2 soltos, que causou o erro de tamanho de buffer).
struct Uniforms {
    time_and_pad: vec4<f32>,
    resolution_and_pad: vec4<f32>,
    // xy = mouse em pixels; z = bitmask das teclas (bit0=up, bit1=down,
    // bit2=left, bit3=right)
    mouse_and_keys: vec4<f32>,
    // xy = posição da nave (espaço "p", aspect-corrected); zw = tamanho
    // (meia-largura, meia-altura) da nave, no mesmo espaço
    ship_pos_and_size: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Truque do "triângulo fullscreen": com apenas 3 vértices (índices 0,1,2),
// geramos um triângulo BEM MAIOR que a tela (vai de -1 até 3 em cada eixo,
// enquanto a tela visível vai só de -1 até 1). A GPU recorta (clip)
// automaticamente a parte que sobra fora da área visível — e o que sobra
// é exatamente um retângulo cobrindo a tela inteira, sem a costura
// diagonal que apareceria se o triângulo fosse do tamanho exato da tela.
// Índice 0 -> (-1,-1)   Índice 1 -> (3,-1)   Índice 2 -> (-1,3)
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;

    // x, y valem 0 ou 2 (não 0 ou 1) — é essa faixa maior que, depois de
    // "* 2.0 - 1.0", produz o overshoot necessário (-1 ou 3).
    let x = f32((vertex_index << 1u) & 2u);
    let y = f32(vertex_index & 2u);

    let pos = vec2<f32>(x, y) * 2.0 - 1.0;
    out.clip_position = vec4<f32>(pos, 0.0, 1.0);
    out.uv = vec2<f32>(pos.x * 0.5 + 0.5, 1.0 - (pos.y * 0.5 + 0.5));

    return out;
}

fn random(st: vec2<f32>) -> f32 {
    return fract(
        sin(dot(st, vec2<f32>(12.9898, 78.233))) * 43758.5453
    );
}

// Aqui mora a parte "artística": pura matemática sobre (uv, tempo) virando
// cor. Troque essas fórmulas à vontade — é o playground mais rápido pra
// sentir o que cada função faz visualmente.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let t = uniforms.time_and_pad.x;
    let resolution = uniforms.resolution_and_pad.xy;
    let mouse_px = uniforms.mouse_and_keys.xy;

    // =========================
    // PIXEL PERFECT
    // =========================

    let PIXEL_SIZE = 8.0;

    let frag_px = uv * resolution;

    let pixel_px =
        floor(frag_px / PIXEL_SIZE)
        * PIXEL_SIZE
        + PIXEL_SIZE * 0.5;

    let pixel_uv = pixel_px / resolution;

    // =========================
    // COORDENADAS
    // =========================

    let aspect = resolution.x / resolution.y;

    var p = (pixel_uv - 0.5) * vec2<f32>(aspect, 1.0);


    // =========================
    // FUNDO: PLASMA
    // =========================

    var color = vec3<f32>(0.0);

    color.r = 0.5 + 0.5 * cos(p.x * 8.0 + t);
    color.g = 0.5 + 0.5 * cos(p.y * 8.0 + t * 1.3);
    color.b = 0.5 + 0.5 * cos((p.x + p.y) * 8.0 + t * 0.7);


    // =========================
    // MOUSE / RIPPLE
    // =========================

    let mouse_uv = mouse_px / resolution;

    let mouse_p =
        (mouse_uv - 0.5) * vec2<f32>(aspect, 1.0);

    let dist_to_mouse = length(p - mouse_p);

    let ripple =
        sin(dist_to_mouse * 20.0 - t * 4.0)
        * 0.5
        + 0.5;

    let ripple_strength =
        ripple * exp(-dist_to_mouse * 3.0);

    color += vec3<f32>(ripple_strength);


    // =========================
    // CÍRCULO
    // =========================

    let circle_radius = 0.3;

    let dist_circle = length(p);

    let circle = 1.0 - step(
        circle_radius,
        dist_circle
    );

    // Começa a transparência em 65% do raio
    let edge = smoothstep(
        circle_radius * 0.20,
        circle_radius,
        dist_circle
    );

    let circle_alpha = mix(
        1.0,
        0.20,
        edge
    );

    let circle_color =
        vec3<f32>(1.0, 0.25, 0.0);

    color = mix(
        color,
        circle_color,
        circle * circle_alpha
    );

    // =========================
    // VINHETA
    // =========================

    let dist = length(p);

    let vignette =
        1.0 - smoothstep(0.3, 0.9, dist);

    color *= vignette;

    // =========================
    // GRANULAÇÃO
    // =========================
    let grain_scale = 180.0;

    let noise = random(
        floor(uv * grain_scale) +
        vec2<f32>(t * 8.0, t * 6.0)
    );

    let grain_strength = 0.02;
    let grain = (noise - 0.5) * 2.0;

    color += grain * grain_strength;


    return vec4<f32>(color, 1.0);
}

// =========================
// SPRITE DA NAVE
// =========================
//
// Pipeline separado do fundo: em vez de um triângulo fullscreen, aqui
// desenhamos um quad (2 triângulos, 6 vértices) posicionado e escalado
// pela nave, e pintamos com a textura da nave em vez de matemática pura.

struct SpriteVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_sprite(@builtin(vertex_index) vertex_index: u32) -> SpriteVertexOutput {
    var out: SpriteVertexOutput;

    // Cantos do quad em espaço local (-0.5..0.5), dois triângulos.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-0.5, -0.5),
        vec2<f32>(0.5, -0.5),
        vec2<f32>(-0.5, 0.5),
        vec2<f32>(-0.5, 0.5),
        vec2<f32>(0.5, -0.5),
        vec2<f32>(0.5, 0.5),
    );
    // UV correspondente a cada canto (0,0 = canto superior esquerdo da
    // textura, seguindo a convenção mais comum de imagem).
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
    );

    let local = corners[vertex_index];
    let ship_pos = uniforms.ship_pos_and_size.xy;
    let ship_size = uniforms.ship_pos_and_size.zw;
    let aspect = uniforms.resolution_and_pad.x / uniforms.resolution_and_pad.y;

    // "p" é o mesmo espaço aspect-corrected usado no fundo (centro em 0,0;
    // x escalado por aspect). Convertendo de volta pra clip space:
    // clip.x = 2 * p.x / aspect ; clip.y = 2 * p.y
    let p = ship_pos + local * ship_size;
    let clip_xy = vec2<f32>(p.x / aspect, p.y) * 2.0;

    out.clip_position = vec4<f32>(clip_xy, 0.0, 1.0);
    out.uv = uvs[vertex_index];

    return out;
}

@group(1) @binding(0)
var ship_texture: texture_2d<f32>;
@group(1) @binding(1)
var ship_sampler: sampler;

@fragment
fn fs_sprite(in: SpriteVertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(ship_texture, ship_sampler, in.uv);
    // Descarta pixels totalmente transparentes — evita desperdiçar
    // trabalho de blending onde não tem nada pra desenhar (e evita
    // artefatos de profundidade caso a gente adicione depth buffer depois).
    if texel.a < 0.01 {
        discard;
    }
    return texel;
}
