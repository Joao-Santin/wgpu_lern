// Precisa bater com a struct Uniforms em main.rs. Usamos vec4 pra cada
// "grupo" de dados propositalmente: vec4 tem alinhamento de 16 bytes tanto
// em WGSL quanto no layout manual em Rust, então não existe padding
// implícito ambíguo entre os dois lados (foi exatamente esse padding
// implícito, com f32/vec2 soltos, que causou o erro de tamanho de buffer).
struct Uniforms {
    time_and_pad: vec4<f32>,
    resolution_and_pad: vec4<f32>,
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

// Aqui mora a parte "artística": pura matemática sobre (uv, tempo) virando
// cor. Troque essas fórmulas à vontade — é o playground mais rápido pra
// sentir o que cada função faz visualmente.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let t = uniforms.time_and_pad.x;
    let resolution = uniforms.resolution_and_pad.xy;

    // Corrige a proporção da tela pra formas não ficarem esticadas.
    let aspect = resolution.x / resolution.y;
    var p = (uv - 0.5) * vec2<f32>(aspect, 1.0);

    // Efeito "plasma": soma de ondas senoidais em frequências e fases
    // diferentes por canal de cor.
    var color = vec3<f32>(0.0);
    color.r = 0.5 + 0.5 * sin(p.x * 8.0 + t);
    color.g = 0.5 + 0.5 * sin(p.y * 8.0 + t * 1.3);
    color.b = 0.5 + 0.5 * sin((p.x + p.y) * 8.0 + t * 0.7);

    // Um vinheta sutil, só pra dar profundidade.
    let dist = length(p);
    let vignette = 1.0 - smoothstep(0.3, 0.9, dist);
    color *= vignette;

    return vec4<f32>(color, 1.0);
}
