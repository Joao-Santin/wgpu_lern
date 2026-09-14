// shader-canvas
//
// Um "canvas" que roda um fragment shader em tela cheia, animado pelo tempo.
// É o exercício de entrada pra wgpu: sem malhas, sem texturas — só o
// pipeline gráfico básico + um shader que pinta cor em função de (uv, tempo).
//
// NOTA SOBRE VERSÕES: wgpu e winit mudam de API com frequência entre
// versões minor. Este código foi escrito para wgpu ~0.19 e winit ~0.29.
// Se o `cargo build` reclamar de algo, o mais provável é que o Cargo
// puxou uma versão mais nova — dá pra fixar a versão exata no Cargo.toml
// (ex: wgpu = "=0.19.4") ou ajustar conforme o erro do compilador indicar.

use std::sync::Arc;
use std::time::Instant;

use wgpu::util::DeviceExt;
use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::{Window, WindowBuilder},
};

// Precisa bater exatamente com o layout esperado no shader.wgsl (bloco
// `Uniforms`).
//
// Uniform buffers em wgpu seguem as regras de alinhamento do WGSL, que são
// DIFERENTES das regras padrão do Rust. Um `vec2<f32>` dentro de uma struct
// WGSL, por exemplo, precisa começar num offset múltiplo de 8 bytes — o
// WGSL insere um padding automático pra garantir isso. O Rust não faz esse
// tipo de padding sozinho, então se você declarar os campos "soltos" (um
// f32, depois um vec2) os dois lados acabam com tamanhos diferentes, como
// aconteceu aqui.
//
// A forma mais segura de evitar esse problema pra sempre: usar blocos de
// 16 bytes (equivalente a vec4) pra cada "grupo" de dados. 16 bytes é o
// alinhamento de vec4 tanto em Rust (nesse layout manual) quanto em WGSL,
// então não sobra nenhuma ambiguidade.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    // x = time; y, z, w não usados (só preenchendo o bloco de 16 bytes)
    time_and_pad: [f32; 4],
    // x, y = resolution; z, w não usados
    resolution_and_pad: [f32; 4],
}

struct State {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: winit::dpi::PhysicalSize<u32>,
    render_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    start_time: Instant,
    window: Arc<Window>,
}

impl State {
    async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();

        // Instance: ponto de entrada da wgpu, escolhe o backend (Vulkan,
        // Metal, DX12...) dependendo do sistema operacional.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        // Surface: a "janela" onde a gente efetivamente desenha.
        let surface = instance.create_surface(window.clone()).unwrap();

        // Adapter: representa uma GPU física disponível no sistema.
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Não encontrei nenhuma GPU compatível");

        // Device + Queue: o "device" cria recursos (buffers, pipelines);
        // a "queue" é onde a gente manda comandos de fato pra GPU executar.
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("Device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .expect("Falha ao pedir device/queue à GPU");

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: surface_caps.present_modes[0],
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Uniform buffer: pequeno bloco de dados (tempo, resolução) que o
        // shader lê todo frame. COPY_DST porque vamos atualizar o "time"
        // a cada frame via queue.write_buffer.
        let uniforms = Uniforms {
            time_and_pad: [0.0, 0.0, 0.0, 0.0],
            resolution_and_pad: [size.width as f32, size.height as f32, 0.0, 0.0],
        };
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Uniform Buffer"),
            contents: bytemuck::cast_slice(&[uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Uniform Bind Group Layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Uniform Bind Group"),
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Carrega o shader (vertex + fragment no mesmo arquivo .wgsl).
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Render Pipeline Layout"),
                bind_group_layouts: &[&uniform_bind_group_layout],
                push_constant_ranges: &[],
            });

        // A pipeline descreve TODO o processo de desenhar: qual shader,
        // como interpretar vértices, formato de saída, etc. Aqui não temos
        // vertex buffer nenhum — os 3 vértices do triângulo fullscreen são
        // gerados dentro do próprio vertex shader (veja shader.wgsl).
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        Self {
            surface,
            device,
            queue,
            config,
            size,
            render_pipeline,
            uniform_buffer,
            uniform_bind_group,
            start_time: Instant::now(),
            window,
        }
    }

    fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    fn update(&mut self) {
        let uniforms = Uniforms {
            time_and_pad: [self.start_time.elapsed().as_secs_f32(), 0.0, 0.0, 0.0],
            resolution_and_pad: [self.size.width as f32, self.size.height as f32, 0.0, 0.0],
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });

            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            // 3 vértices, gerados no shader — nenhum vertex buffer necessário.
            render_pass.draw(0..3, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("shader-canvas")
            .with_inner_size(winit::dpi::LogicalSize::new(800, 600))
            .build(&event_loop)
            .unwrap(),
    );

    let mut state = pollster::block_on(State::new(window.clone()));

    event_loop
        .run(move |event, elwt| {
            match event {
                Event::WindowEvent { event, window_id } if window_id == state.window.id() => {
                    match event {
                        WindowEvent::CloseRequested => elwt.exit(),
                        WindowEvent::Resized(physical_size) => {
                            state.resize(physical_size);
                        }
                        WindowEvent::RedrawRequested => {
                            state.update();
                            match state.render() {
                                Ok(_) => {}
                                Err(wgpu::SurfaceError::Lost) => state.resize(state.size),
                                Err(wgpu::SurfaceError::OutOfMemory) => elwt.exit(),
                                Err(e) => eprintln!("Erro de render: {:?}", e),
                            }
                        }
                        _ => {}
                    }
                }
                Event::AboutToWait => {
                    // Pede o próximo frame — mantém a animação rodando
                    // continuamente em vez de só redesenhar sob demanda.
                    state.window.request_redraw();
                }
                _ => {}
            }
        })
        .unwrap();
}
