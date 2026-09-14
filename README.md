# shader-canvas

Primeiro exercício de wgpu: uma janela com um fragment shader animado,
sem malhas, sem texturas — só o pipeline gráfico básico.

## Como rodar

```powershell
cd shader-canvas
cargo run
```

A primeira compilação demora (baixa e compila wgpu + dependências).
Depois disso deve abrir uma janela com um efeito de "plasma" colorido
se movendo.

Se der erro de compilação por causa de versão de wgpu/winit, veja o
comentário no topo de `src/main.rs`.

## O que está acontecendo, em ordem

1. **Instance → Adapter → Device/Queue**: a cadeia de setup do wgpu.
   `Instance` é o ponto de entrada; `Adapter` representa a GPU física;
   `Device`/`Queue` é como você efetivamente cria recursos e manda
   trabalho pra GPU.
2. **Surface**: a superfície de desenho ligada à janela do sistema
   operacional.
3. **Uniform buffer**: um pequeno bloco de dados (`time`, `resolution`)
   que é reenviado pra GPU a cada frame (`queue.write_buffer`) e lido
   pelo shader.
4. **Shader module**: carrega `shader.wgsl`, que tem duas funções —
   `vs_main` (vertex shader) e `fs_main` (fragment shader).
5. **Render pipeline**: descreve o processo completo de desenho —
   qual shader, formato de saída, topologia de primitivas, etc.
6. **Truque do triângulo fullscreen**: em vez de mandar 2 triângulos
   (retângulo) via vertex buffer, geramos matematicamente 1 triângulo
   gigante dentro do próprio vertex shader, só a partir do índice do
   vértice (0, 1, 2). É um truque clássico pra economizar setup quando
   você só quer "pintar a tela inteira".
7. **Render loop**: a cada frame, atualiza o `time` no uniform buffer e
   desenha.

## Próximos passos (na ordem que eu sugiro)

- [ ] **Mexer no shader.wgsl**: troque as fórmulas de `fs_main` (senos,
      frequências, multiplicadores) e veja o que muda. É o jeito mais
      rápido de "sentir" GLSL/WGSL.
- [ ] **Mouse reativo**: adicionar `mouse: vec2<f32>` no `Uniforms`,
      capturar `WindowEvent::CursorMoved` no `main.rs` e usar a posição
      do mouse no shader (ex: distância do pixel até o mouse afeta a cor).
- [ ] **Instancing**: desenhar centenas de formas (círculos, quads) numa
      única draw call, cada uma com posição/cor diferente vindas de um
      buffer de instâncias.
- [ ] **Movimento/física simples**: dar velocidade e comportamento às
      instâncias (atração ao mouse, ruído, colisão simples).
- [ ] **Compor a experiência final**: decidir se vira um "brinquedo"
      interativo, uma tela de menu de jogo estilizada, ou o próprio jogo.

Quando terminar de explorar o passo 1, me chama que a gente monta o
próximo (mouse reativo ou instancing, o que você preferir).
