// shader-canvas
//
// Um "canvas" que roda um fragment shader em tela cheia, animado pelo tempo,
// mais um sprite (nave) desenhado por cima, controlado pelo teclado.
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
    event::{ElementState, Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowBuilder},
};

// Estado das teclas que nos interessam. Guardamos como booleans simples —
// nada de "eventos", é o estado ATUAL de cada tecla (pressionada ou não)
// no momento em que o frame é desenhado.
#[derive(Default)]
struct KeysState {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
}

impl KeysState {
    // Empacota os 4 booleans num único f32 via bitmask (1, 2, 4, 8).
    // Vai junto no uniform buffer; não é usado no shader ainda, mas fica
    // disponível pra experimentar (ex: cor da nave mudar por tecla).
    fn as_bitmask(&self) -> f32 {
        let mut mask: u32 = 0;
        if self.up {
            mask |= 1;
        }
        if self.down {
            mask |= 2;
        }
        if self.left {
            mask |= 4;
        }
        if self.right {
            mask |= 8;
        }
        mask as f32
    }
}

// Precisa bater exatamente com o layout esperado no shader.wgsl (bloco
// `Uniforms`).
//
// Uniform buffers em wgpu seguem as regras de alinhamento do WGSL, que são
// DIFERENTES das regras padrão do Rust. A forma mais segura de evitar
// ambiguidade: usar blocos de 16 bytes (equivalente a vec4) pra cada
// "grupo" de dados. 16 bytes é o alinhamento de vec4 tanto em Rust (nesse
// layout manual) quanto em WGSL.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    // x = time; y, z, w não usados (só preenchendo o bloco de 16 bytes)
    time_and_pad: [f32; 4],
    // x, y = resolution; z, w não usados
    resolution_and_pad: [f32; 4],
    // x, y = posição do mouse em pixels; z = bitmask das teclas WASD/setas;
    // w não usado
    mouse_and_keys: [f32; 4],
    // x, y = posição da nave (espaço "p", aspect-corrected); z, w =
    // meia-largura / meia-altura da nave, no mesmo espaço
    ship_pos_and_size: [f32; 4],
}

struct State {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: winit::dpi::PhysicalSize<u32>,
    render_pipeline: wgpu::RenderPipeline,
    sprite_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    ship_bind_group: wgpu::BindGroup,
    start_time: Instant,
    window: Arc<Window>,
    mouse_pos: (f32, f32),
    keys: KeysState,
    ship_pos: (f32, f32),
    ship_half_size: (f32, f32),
    last_frame_time: Instant,
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

        // Posição/tamanho iniciais da nave, no mesmo espaço "p" usado no
        // shader (centro da tela = (0,0); altura da tela = 1.0 de -0.5 a
        // 0.5; largura = aspect, aspect-corrected).
        let ship_pos = (0.0_f32, 0.0_f32);
        let ship_half_size = (0.06_f32, 0.06_f32);

        // Uniform buffer: pequeno bloco de dados que o shader lê todo
        // frame. COPY_DST porque vamos atualizar via queue.write_buffer.
        let uniforms = Uniforms {
            time_and_pad: [0.0, 0.0, 0.0, 0.0],
            resolution_and_pad: [size.width as f32, size.height as f32, 0.0, 0.0],
            mouse_and_keys: [0.0, 0.0, 0.0, 0.0],
            ship_pos_and_size: [ship_pos.0, ship_pos.1, ship_half_size.0, ship_half_size.1],
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

        // --- Textura da nave ---
        //
        // include_bytes! embute o PNG dentro do próprio binário em tempo de
        // compilação (não precisa carregar arquivo em disco em runtime).
        let ship_image_bytes = include_bytes!("../assets/ship.png");
        let ship_image = image::load_from_memory(ship_image_bytes)
            .expect("Falha ao decodificar assets/ship.png")
            .to_rgba8();
        let ship_width = ship_image.width();
        let ship_height = ship_image.height();

        let ship_texture_size = wgpu::Extent3d {
            width: ship_width,
            height: ship_height,
            depth_or_array_layers: 1,
        };

        let ship_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Ship Texture"),
            size: ship_texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Envia os bytes decodificados da imagem pra dentro da textura na
        // GPU. bytes_per_row precisa ser width * 4 (RGBA, 1 byte por canal).
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &ship_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &ship_image,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * ship_width),
                rows_per_image: Some(ship_height),
            },
            ship_texture_size,
        );

        let ship_texture_view =
            ship_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Sampler com filtro "Nearest": é o que preserva a cara pixelada.
        // O padrão (Linear) borraria os pixels ao ampliar a textura pequena
        // — o oposto do que queremos num sprite 8-bit.
        let ship_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ship Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Texture Bind Group Layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let ship_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ship Bind Group"),
            layout: &texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&ship_texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&ship_sampler),
                },
            ],
        });

        // Carrega o shader (vertex + fragment de fundo, e vertex + fragment
        // do sprite, tudo no mesmo arquivo .wgsl).
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

        // Pipeline do fundo: triângulo fullscreen, sem transparência (cobre
        // a tela inteira, então blending não é necessário).
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

        // Pipeline do sprite: usa DOIS bind group layouts — group(0) pros
        // uniforms (mesmo bloco do fundo) e group(1) pra textura/sampler.
        // Com blending ALPHA (não REPLACE), porque o PNG da nave tem
        // pixels transparentes que precisam deixar o fundo aparecer atrás.
        let sprite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Sprite Pipeline Layout"),
                bind_group_layouts: &[&uniform_bind_group_layout, &texture_bind_group_layout],
                push_constant_ranges: &[],
            });

        let sprite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Sprite Pipeline"),
            layout: Some(&sprite_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_sprite",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_sprite",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
            sprite_pipeline,
            uniform_buffer,
            uniform_bind_group,
            ship_bind_group,
            start_time: Instant::now(),
            window,
            mouse_pos: (0.0, 0.0),
            keys: KeysState::default(),
            ship_pos,
            ship_half_size,
            last_frame_time: Instant::now(),
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
        // Delta time: quanto tempo passou desde o frame anterior. Usar dt
        // (em vez de um valor fixo) garante que o movimento tenha a mesma
        // velocidade percebida independente do framerate da máquina.
        let now = Instant::now();
        let dt = (now - self.last_frame_time).as_secs_f32();
        self.last_frame_time = now;

        let speed = 0.8_f32; // unidades do espaço "p" por segundo

        // Convenção: no clip space do wgpu, y positivo aponta pra CIMA na
        // tela. Então seta-pra-cima deve AUMENTAR ship_pos.y. Se ao testar
        // isso estiver invertido na sua tela, é só trocar os sinais aqui —
        // eu não consigo rodar o programa pra confirmar visualmente.
        if self.keys.up {
            self.ship_pos.1 += speed * dt;
        }
        if self.keys.down {
            self.ship_pos.1 -= speed * dt;
        }
        if self.keys.left {
            self.ship_pos.0 -= speed * dt;
        }
        if self.keys.right {
            self.ship_pos.0 += speed * dt;
        }

        // Trava a nave dentro da tela. O espaço "p" tem altura total 1.0
        // (-0.5 a 0.5) e largura total = aspect (-aspect/2 a aspect/2).
        let aspect = self.size.width as f32 / self.size.height as f32;
        let max_x = aspect * 0.5 - self.ship_half_size.0;
        let max_y = 0.5 - self.ship_half_size.1;
        self.ship_pos.0 = self.ship_pos.0.clamp(-max_x, max_x);
        self.ship_pos.1 = self.ship_pos.1.clamp(-max_y, max_y);

        let uniforms = Uniforms {
            time_and_pad: [self.start_time.elapsed().as_secs_f32(), 0.0, 0.0, 0.0],
            resolution_and_pad: [self.size.width as f32, self.size.height as f32, 0.0, 0.0],
            mouse_and_keys: [
                self.mouse_pos.0,
                self.mouse_pos.1,
                self.keys.as_bitmask(),
                0.0,
            ],
            ship_pos_and_size: [
                self.ship_pos.0,
                self.ship_pos.1,
                self.ship_half_size.0,
                self.ship_half_size.1,
            ],
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

            // 1) Fundo: triângulo fullscreen com o shader de plasma.
            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.draw(0..3, 0..1);

            // 2) Nave: desenhada por cima, na MESMA render pass (mesmo
            // "load: Clear" não é reaplicado — ele só vale pro começo da
            // pass; esse segundo draw soma-se ao que já está no framebuffer
            // via alpha blending).
            render_pass.set_pipeline(&self.sprite_pipeline);
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_bind_group(1, &self.ship_bind_group, &[]);
            render_pass.draw(0..6, 0..1);
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
                        WindowEvent::CursorMoved { position, .. } => {
                            state.mouse_pos = (position.x as f32, position.y as f32);
                        }
                        WindowEvent::KeyboardInput { event, .. } => {
                            let pressed = event.state == ElementState::Pressed;
                            match event.physical_key {
                                PhysicalKey::Code(KeyCode::KeyW | KeyCode::ArrowUp) => {
                                    state.keys.up = pressed;
                                }
                                PhysicalKey::Code(KeyCode::KeyS | KeyCode::ArrowDown) => {
                                    state.keys.down = pressed;
                                }
                                PhysicalKey::Code(KeyCode::KeyA | KeyCode::ArrowLeft) => {
                                    state.keys.left = pressed;
                                }
                                PhysicalKey::Code(KeyCode::KeyD | KeyCode::ArrowRight) => {
                                    state.keys.right = pressed;
                                }
                                _ => {}
                            }
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
