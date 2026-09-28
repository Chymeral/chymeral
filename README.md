# Moditty

Moditty es un motor de terminal extensible escrito en Rust.

Su objetivo inicial es comportarse como una terminal extremadamente mínima y funcional, pero su propósito real es servir como plataforma para desarrollar TUIs avanzadas, plugins y mods con capacidades de interacción que normalmente son difíciles o imposibles de implementar limpiamente sobre una terminal tradicional.

Moditty debe pensarse más como un pequeño motor gráfico/event-driven cuyo objeto principal por defecto es una terminal, y no simplemente como un emulador de terminal al que posteriormente se le añaden funciones.

## Objetivo del producto

La experiencia inicial de Moditty debe ser deliberadamente mínima:

* abrir una ventana;
* ejecutar el shell del usuario mediante PTY;
* interpretar ANSI/VT;
* mantener un grid propio;
* renderizar texto;
* aceptar teclado;
* soportar resize y comportamiento normal de una terminal.

Sobre esa base, Moditty debe proporcionar una API propia que permita construir interfaces TUI avanzadas y elementos interactivos.

Entre las capacidades futuras de esta API estarán:

* eventos de teclado;
* mouse con coordenadas precisas;
* hover;
* click;
* double click;
* drag;
* drag and drop;
* captura de puntero;
* focus;
* regiones interactivas;
* hit-testing;
* clipping;
* renderizado de imágenes;
* texturas;
* capas y z-order;
* eventos personalizados;
* timers;
* animaciones;
* overlap/collision events;
* clipboard;
* superficies o nodos renderizables.

Estas capacidades deben ser primitivas del motor. Moditty no debe convertirse directamente en un toolkit de widgets.

No deben existir primitivas de alto nivel como:

```text
create_button()
create_checkbox()
create_file_browser()
```

La API debe ofrecer primitivas sobre las cuales terceros puedan desarrollar sus propios frameworks TUI.

Conceptualmente:

```text
Moditty Engine
     │
     ├── Terminal tradicional
     │
     ├── STAPI / API del motor
     │
     ├── Mods
     │
     └── Frameworks TUI de terceros
```

Una aplicación tradicional como bash, vim o htop debe poder ejecutarse normalmente sin conocer nada de Moditty.

Una aplicación diseñada específicamente para Moditty podrá usar las capacidades avanzadas del motor.

## Filosofía de plataforma

Moditty debe tratar las TUIs avanzadas como mods o aplicaciones ejecutadas sobre una plataforma.

La terminal no debe imponer cómo debe verse o funcionar una interfaz.

El motor solo debe proporcionar primitivas como:

```text
Node
Text
Texture
Bounds
Transform
Clip
Focus
Pointer
Event
Timer
TerminalSurface
```

Los widgets, layouts complejos, editores, navegadores de archivos, interfaces visuales y demás abstracciones deben poder ser creados como librerías o mods externos.

El objetivo es que el ecosistema pueda construir cosas encima de Moditty sin necesitar modificar el core.

El espíritu del sistema debe ser similar al de un motor de videojuegos altamente modeable:

```text
engine
  ↓
primitives
  ↓
API
  ↓
mods/frameworks
  ↓
experiences
```

pero aplicado a una terminal y a aplicaciones TUI.

## Arquitectura fundamental

El grid de terminal NO debe ser el sistema gráfico de Moditty.

Debe existir como una superficie renderizable dentro del motor.

Conceptualmente:

```text
Scene
├── TerminalSurface
├── TextNode
├── ImageNode
├── TextureNode
├── CustomNode
└── Overlay
```

El terminal convencional será un componente especializado compuesto por:

```text
PTY
 ↓
ANSI/VT parser
 ↓
TerminalState
 ↓
TerminalGrid
 ↓
TerminalSurface
 ↓
Renderer
```

Esto permitirá que elementos gráficos e interactivos existan independientemente de las celdas de terminal.

No se deben representar imágenes, drag and drop u objetos gráficos únicamente mediante hacks ANSI si el motor puede representarlos directamente.

## Stack tecnológico inicial

Lenguaje principal:

```text
Rust
```

Ventana y eventos del sistema:

```text
winit
```

Responsabilidades:

* ventana;
* event loop;
* teclado;
* mouse;
* scroll;
* focus;
* drag and drop del sistema;
* eventos del sistema operativo.

Renderizado:

```text
wgpu
```

Responsabilidades:

* GPU;
* composición;
* primitivas 2D;
* texturas;
* renderizado de superficies.

Texto:

```text
cosmic-text
glyphon
```

Responsabilidades:

* font discovery;
* fallback;
* shaping;
* layout;
* rasterización/renderizado de glifos.

Terminal PTY:

```text
portable-pty
```

Debe quedar detrás de una abstracción propia de Moditty.

Parser ANSI/VT:

```text
vte
```

Se utilizará como parser, pero Moditty mantendrá su propio estado y grid de terminal.

Imágenes:

```text
image
```

Para decodificación de imágenes raster antes de convertirlas en texturas del renderer.

Handles internos:

```text
slotmap
```

o una solución equivalente basada en IDs generacionales.

No se deben exponer referencias o punteros internos a plugins.

Ejemplo conceptual:

```text
NodeId
TextureId
TerminalId
ResourceId
```

Las capacidades avanzadas de geometría pueden incorporar posteriormente:

```text
parry2d
```

pero NO debe añadirse inicialmente salvo necesidad real.

No utilizar un motor de física.

No introducir Tokio inicialmente salvo que aparezca una necesidad arquitectónica concreta.

## Regla arquitectónica crítica

La API pública de Moditty NO debe exponer tipos de dependencias externas.

Nunca hacer que una interfaz pública dependa directamente de tipos como:

```rust
winit::event::WindowEvent
wgpu::Texture
vte::Parser
cosmic_text::Buffer
portable_pty::PtyPair
```

Estas dependencias son detalles internos del backend.

Moditty debe definir sus propios tipos:

```rust
EngineEvent
PointerEvent
KeyEvent
DropEvent
NodeId
TextureId
Rect
Vec2
TerminalId
```

Ejemplo:

No:

```rust
fn handle_event(event: winit::event::WindowEvent);
```

Sí:

```rust
fn dispatch(event: EngineEvent);
```

No:

```rust
fn texture() -> &wgpu::Texture;
```

Sí:

```rust
fn texture() -> TextureId;
```

Esta separación debe preservarse incluso si inicialmente parece innecesaria.

## Sistema de eventos

Debe existir un modelo de eventos propio.

Ejemplo conceptual:

```rust
enum EngineEvent {
    Key(KeyEvent),

    Pointer(PointerEvent),
    Scene(SceneEvent),

    FileHover(FileEvent),
    FileDrop(FileEvent),

    Focus(FocusEvent),

    CollisionEnter(CollisionEvent),
    CollisionLeave(CollisionEvent),

    Terminal(TerminalEvent),

    Custom(CustomEvent),
}
```

Los eventos de winit deben ser traducidos a eventos de Moditty antes de llegar a sistemas superiores.

Pipeline esperado:

```text
OS / winit
    ↓
Input backend
    ↓
normalización
    ↓
EngineEvent
    ↓
hit testing / focus
    ↓
EngineEvent::Scene
    ↓
mods
```

## Ciclo de vida del runtime

El runtime de un Mod o programa tiene tres rutas internas. Son contratos de
ejecución, no una exigencia de usar tres hilos del sistema.

```text
Init     init() → start()
Event    press(), release(), scroll(), …
Runtime  tick(dt), frame(dt)
```

`init()` se invoca una vez antes de iniciar el Mod o programa. Permite cargar
argumentos, configuración, comandos y recursos iniciales, pero todavía no debe
recibir órdenes ni eventos de usuario.

`start()` se invoca una única vez después de que `init()` haya terminado con
éxito y el Mod o programa esté preparado para recibir órdenes. Ningún evento ni
llamada de runtime se entrega antes de `start()`.

La ruta `Event` entrega las entradas discretas mediante la API canónica del
runtime, por ejemplo `press()`, `release()` y `scroll()`. Estas entradas se
originan en `EngineEvent`, no en tipos de `winit` u otro backend.

La ruta `Runtime` mantiene la ejecución continua: `tick(dt)` actualiza la
lógica y `frame(dt)` prepara o renderiza un cuadro. Un frame puede no contener
ningún tick o contener varios; ambos deben operar únicamente después de
`start()`.

M2 concreta ese contrato con un `Program` interno:

```rust
trait Program {
    fn init(&mut self, context: &mut InitContext) -> RuntimeResult<()>;
    fn start(&mut self) -> RuntimeResult<()>;
    fn on_event(&mut self, event: &EngineEvent) -> RuntimeResult<()>;
    fn on_command(&mut self, command: &RuntimeCommand) -> RuntimeResult<()>;
    fn tick(&mut self, dt: Duration) -> RuntimeResult<()>;
    fn frame(&mut self, dt: Duration) -> RuntimeResult<()>;
}
```

`InitContext` contiene los argumentos y comandos iniciales. Tras un `init()` y
`start()` correctos, los comandos iniciales se encolan para `on_command()`.
Un error en cualquiera de esas dos llamadas impide que el programa entre en
ejecución.

Un error de `on_event()`, `on_command()`, `tick()` o `frame()` es terminal para
el runtime activo: pasa a `RuntimeState::Faulted(RuntimeFault)`, conserva la
fase y el error, descarta los mensajes pendientes y cancela sus ticks. Desde
ese momento no acepta ni entrega más eventos o comandos. M2 sólo tiene un
programa activo, por lo que el host registra el fallo y cierra limpiamente la
aplicación; el aislamiento de extensiones llegará junto al sistema de plugins.

`RuntimeCommand` es un canal separado de `EngineEvent`: una orden controla al
programa, mientras que un evento describe una entrada o suceso. Internamente
ambos comparten una única cola FIFO, por lo que se despachan al programa activo
en el orden exacto en que llegan. Cada iteración del scheduler vacía esa cola y
ejecuta cero o más `tick(dt)` de intervalo fijo. Cuando ocurre un redibujado,
después de esos pasos se invoca un único `frame(dt)`. El scheduler limita la
recuperación a ocho ticks por iteración y descarta retraso sobrante para no
bloquear la interfaz tras una pausa larga.

Los tipos canónicos actuales incluyen teclado, puntero, scroll, foco, terminal
y eventos personalizados. Los adaptadores de plataforma viven en el backend;
el runtime y los programas no reciben tipos de `winit`.

## Scene graph

No usar ECS inicialmente.

M3 introduce un scene graph interno, sin editor visual. Su núcleo es una
estructura backend-agnóstica que organiza pertenencia, visibilidad y orden de
superposición para los recursos y la interacción que llegarán en milestones
posteriores. M5 añade su primer consumidor visual: nodos de imagen rasterizada.

Cada `Node` tiene un `NodeId` estable, un padre opcional, hijos ordenados,
una traslación local, bounds locales, visibilidad, `clip_children` y
`z_index`. Los nodos raíz pertenecen directamente a la escena y seleccionan
una `SceneLayer`; las posiciones usan píxeles lógicos y la traslación de un
hijo es relativa a su padre.

```rust
struct Node {
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    layer: SceneLayer,
    transform: Transform,
    bounds: Rect,
    visible: bool,
    clip_children: bool,
    z_index: i32,
    content: NodeContent,
}
```

Los objetos se referencian mediante handles generacionales, no referencias
Rust directas hacia las estructuras internas:

```text
NodeId { index, generation }
```

Al eliminar un nodo, también se eliminan sus descendientes. Si el slot interno
se reutiliza, cambia su generación y el `NodeId` anterior queda inválido.
Reparentar un nodo lo añade al final de los hijos del nuevo padre; no se
permiten ciclos. Un nodo invisible oculta su subárbol del orden de pintura.

El scene graph usa contextos de apilamiento jerárquicos. Cada padre se pinta
antes que su subárbol; los hijos se ordenan por `z_index` ascendente y, ante
empate, por su orden de inserción. Un subárbol completo se pinta antes de pasar
al siguiente hermano. La misma regla se aplica entre raíces de la misma capa.
M9 agrega un orden fijo entre `Underlay` y `Overlay`; por tanto, un
`z_index` alto organiza hermanos dentro de su capa, pero no puede atravesar
ese límite global.

El núcleo de M3 no impone renderer, texto de escena, hit testing, colisiones,
layouts, widgets ni APIs de plugins. Esos sistemas consumen esta estructura
sin alterar sus invariantes.

## Recursos gráficos

M4 introduce un registro interno de texturas inmutables. Una textura almacena
píxeles rasterizados RGBA8 sRGB con alfa no premultiplicado; no representa una
posición ni un elemento visible de la escena. Las imágenes en formatos como
PNG, JPEG o GIF deben decodificarse primero a estos píxeles.

Cada recurso recibe un `TextureId { index, generation }` generacional. El
registro permite insertar, consultar y eliminar texturas; eliminar una textura
invalida su handle anterior incluso si se reutiliza el slot interno. El
registro no conoce el scene graph y no expone tipos de `wgpu`.

M4 valida dimensiones no nulas y la correspondencia exacta entre dimensiones y
bytes RGBA8, pero no define cuotas globales de memoria, actualizaciones de
píxeles, render targets, animación ni carga de archivos.

## Imagen de escena

M5 incorpora `NodeContent::Image(ImageNode)`: un nodo contiene cero o una
imagen y las composiciones se forman con hijos. `ImageNode` referencia un
`TextureId` y define una opacidad propia entre `0.0` y `1.0`. El renderer
recorre el orden de pintura del scene graph, convierte los bounds lógicos a la
superficie de la ventana y compone las imágenes en la capa de su raíz. M9
define si esa capa queda antes o después del texto de la terminal. Por tanto,
escena y terminal comparten ventana y coordenadas lógicas, pero el grid de
terminal mantiene su representación interna independiente.

Una textura eliminada se omite al dibujar aunque un nodo aún conserve su
handle. M5 no añade texto de escena, carga general de recursos, animación de
nodos, repetición, sprites ni una API pública para mods.

## Interacción y colisiones

Las necesidades iniciales son de UI, no de física.

M6 introduce hit testing geométrico y backend-agnóstico. `Point` y `Rect` usan
píxeles lógicos; los rectángulos son semiabiertos, por lo que incluyen su borde
izquierdo y superior, pero excluyen el derecho e inferior. `Scene::hit_test`
devuelve el nodo visible más alto bajo un punto. `Scene::hit_test_rect` devuelve
todos los nodos visibles que se superponen con un rectángulo, de frente a fondo.
Ambas consultas usan los bounds mundiales y el mismo orden determinista de
pintura del scene graph.

Un nodo no requiere contenido ni una marca de interactividad para participar:
M6 sólo resuelve geometría. Por defecto, un hijo puede detectarse fuera de los
bounds de su padre. M9 permite limitar explícitamente esa herencia visual.

M7 mantiene `hovered_node` y `focused_node` y convierte las posiciones de
puntero y scroll en píxeles lógicos antes de crear eventos. Conserva los
`EngineEvent::Pointer`, `Scroll` y `Focus` crudos; después del puntero crudo
emite el canal separado `EngineEvent::Scene` con `PointerEnter`, `PointerLeave`,
`PointerMove`, `PointerPress`, `PointerRelease`, `FocusGained` y `FocusLost`.
La salida del cursor de la ventana también emite `PointerLeave`.
Un clic primario actualiza el foco, un clic primario en espacio vacío lo limpia
y los otros botones no lo alteran. La pérdida de foco de la ventana no cambia
el foco de escena.

La liberación se dirige al nodo bajo el cursor en ese momento. M7 no añade
click/doble click, touch, foco por teclado, navegación por Tab ni entrega de
teclado al nodo enfocado; esos comportamientos se incorporan en milestones
posteriores. Si un nodo hovered o focused se elimina, se oculta o deja de estar
en el orden de pintura, la interacción se reconcilia y emite la salida
correspondiente.

M8 añade drag and drop al canal derivado `EngineEvent::Scene`, sin alterar los
eventos de puntero crudos. Un `PointerPress` primario sobre un nodo visible crea
un candidato; el drag comienza al recorrer una distancia euclídea de al menos
4 píxeles lógicos. Desde ese momento se emiten `DragStart`, `DragEnter`,
`DragLeave` y `DragMove`. Este último contiene un destino opcional y se emite
en cada movimiento del puntero mientras el drag esté activo.

El destino se obtiene con hit testing de los bounds visibles. El origen y todos
sus descendientes se excluyen para impedir soltarse sobre sí mismo, pero un
ancestro del origen sigue siendo un destino válido. En la liberación primaria,
un destino produce `Drop` seguido de `DragEnd`; fuera de un destino sólo se
emite `DragEnd`. Moditty no mueve nodos ni cambia su contenido automáticamente:
el consumidor de los eventos decide cualquier efecto visual o lógico.

La captura de M8 es lógica, no un grab del cursor del sistema. Por ello, salir
de la ventana conserva el drag activo, emite las salidas de hover y de destino,
y permite continuarlo cuando el cursor vuelve. Perder el foco de la ventana
cancela un drag activo mediante `DragCancel`. Si su origen se elimina u oculta,
la reconciliación también lo cancela. M8 sigue limitado al botón primario y no
añade gestos táctiles ni comportamiento de click.

M9 añade clipping rectangular heredado y capas globales de escena. Cada nodo
incluye `clip_children`, desactivado por defecto. Cuando está activo, los
bounds mundiales del nodo recortan solamente a sus descendientes; no cambian
el área propia del nodo. Los clips de ancestros se intersectan y una región
vacía deja al descendiente fuera de render y de interacción. Las consultas
`hit_test` y `hit_test_rect` usan esos mismos bounds visibles, por lo que no
existen áreas clicables que no puedan dibujarse.

M9 define dos `SceneLayer`: `Underlay` se dibuja sobre el fondo y antes del
texto de terminal; `Overlay`, predeterminado, se dibuja después del texto.
Una raíz selecciona su capa y todos sus descendientes la heredan. Las capas
ordenan árboles completos: ningún `z_index` puede llevar un nodo de
`Underlay` sobre uno de `Overlay`. Dentro de cada capa se conserva el orden
determinista de M3. El hit testing respeta el mismo orden, de modo que
`Overlay` tiene prioridad y `Underlay` recibe interacción cuando no existe
un destino superior de escena.

El renderer aplica los clips mediante scissor rects físicos derivados de los
bounds lógicos, incluida la conversión DPI. M9 no incorpora máscaras,
rectángulos de clip arbitrarios, esquinas redondeadas, filtros, render targets,
capas modales ni protocolos de mouse para la terminal.

La siguiente infraestructura permite:

```text
point vs rect
rect vs rect
```

Esto debe permitir:

```text
hover
hit testing
click
drag
drop
overlap
collision enter
collision leave
```

La API debe diferenciar conceptualmente hit-testing de colisiones, aunque internamente puedan compartir infraestructura.

## Sistema de plugins

El sistema de plugins debe ser independiente del lenguaje.

Rust NO debe ser un requisito para crear mods.

La arquitectura final debe permitir plugins escritos en cualquier lenguaje siempre que exista un adaptador de lenguaje capaz de comunicarse con la API de Moditty.

Ejemplos potenciales:

```text
Python
Lua
JavaScript
C
C++
Rust
Zig
C#
Java
Go
otros
```

El motor debe exponer una API canónica independiente del lenguaje.

Nombre provisional:

```text
STAPI
```

El nombre puede cambiar posteriormente.

Conceptualmente:

```text
Plugin
   ↓
Language Bridge
   ↓
STAPI
   ↓
Moditty Engine
```

## Plugins fuera del proceso

El mecanismo universal preferido debe considerar plugins ejecutados como procesos separados.

Ejemplo:

```text
Moditty
   │
   ├── Python plugin
   ├── Lua plugin
   ├── Node.js plugin
   └── executable arbitrario
```

Todos se comunicarían mediante un protocolo común.

Transportes potenciales:

```text
stdio
Unix socket
named pipe
shared memory
```

No fijar aún el transporte definitivo.

El protocolo debe estar desacoplado del transporte.

En el futuro pueden coexistir:

```text
process transport
WASM transport
native transport
```

sin cambiar la API lógica.

## Valores canónicos

Debe existir una representación neutral de valores para cruzar la frontera entre lenguajes.

Ejemplo conceptual:

```rust
enum STValue {
    Null,
    Bool(bool),

    I64(i64),
    U64(u64),
    F64(f64),

    String(String),
    Bytes(Vec<u8>),

    Handle(HandleId),

    Array(Vec<STValue>),
    Object(Vec<(String, STValue)>),
}
```

La API podría conceptualizarse como:

```rust
call(
    function: ApiFunction,
    arguments: &[STValue],
) -> Result<STValue, ApiError>
```

Esto es conceptual. No implementar todavía sin diseñar adecuadamente el protocolo.

## Extensiones de lenguaje

Cada lenguaje podrá tener una extensión encargada de traducir entre dicho lenguaje y STAPI.

Nombre provisional del manifiesto:

```text
STExtention.toml
```

El nombre contiene deliberadamente "Extention" por ahora y puede revisarse posteriormente.

Ejemplo:

```text
extensions/
└── python/
    ├── STExtention.toml
    ├── bridge.py
    └── ...
```

El manifiesto debe describir metadata y configuración.

La lógica compleja debe residir en el bridge, no en TOML.

Ejemplo conceptual:

```toml
[extension]
id = "org.moditty.python"
language = "python"
api = "1"

[execution]
command = "python3"
arguments = ["{bridge}", "{entry}"]

[bridge]
entry = "bridge.py"
transport = "stdio"

[files]
extensions = [".py"]
default_entry = "main.py"
```

Una sola extensión de lenguaje debe poder permitir la ejecución de muchos plugins escritos en ese lenguaje.

## IDL futura

Debe considerarse una Interface Definition Language neutral para definir STAPI.

Nombre provisional:

```text
STIDL
```

Ejemplo conceptual:

```text
resource Node
resource Texture

fn create_node() -> Node

fn set_position(
    node: Node,
    x: f32,
    y: f32
)

fn load_texture(
    path: string
) -> Texture

event PointerMove {
    x: f32
    y: f32
}
```

STIDL debe convertirse eventualmente en la fuente de verdad de la API.

A partir de ella podrán generarse bindings para diferentes lenguajes:

```text
STIDL
  │
  ├── Rust bindings
  ├── Python bindings
  ├── C bindings
  ├── Lua bindings
  └── otros
```

No implementar STIDL durante el primer milestone.

## Estructura conceptual del proyecto

La estructura futura puede aproximarse a:

```text
moditty/
├── Cargo.toml
├── crates/
│   ├── moditty-core/
│   ├── moditty-events/
│   ├── moditty-scene/
│   ├── moditty-render/
│   ├── moditty-render-wgpu/
│   ├── moditty-text/
│   ├── moditty-terminal/
│   ├── moditty-pty/
│   ├── moditty-api/
│   ├── moditty-plugin/
│   └── moditty-app/
│
├── api/
│
└── extensions/
```

No crear crates vacíos solo para satisfacer esta estructura.

Crear únicamente los crates necesarios para cada milestone y separar componentes cuando exista una frontera arquitectónica real.

## Primer milestone

No implementar plugins todavía.

No implementar STAPI todavía.

No implementar STIDL todavía.

No implementar un toolkit de UI.

No implementar colisiones avanzadas.

No implementar layouts.

El primer milestone debe producir una terminal mínima funcional.

Pipeline:

```text
winit
   ↓
window

wgpu
   ↓
render surface

portable-pty
   ↓
user shell

vte
   ↓
terminal parser

own terminal state
   ↓
own terminal grid

cosmic-text / glyphon
   ↓
visible text
```

El resultado mínimo esperado:

```text
$ echo hello
hello
$
```

con teclado funcional y resize.

### Estado de M1

M1 está orientado a Linux y Windows. En Windows requiere Windows 10 1809 o posterior, o Windows 11, para disponer de ConPTY. macOS queda fuera del alcance mientras no exista una infraestructura de prueba mantenida. Ejecutar:

```text
cargo run
```

La implementación actual abre una ventana, crea una sesión de terminal con el shell definido en `$SHELL` (o `/bin/sh`) en Linux y `COMSPEC` (o `cmd.exe`) en Windows, usa `TERM=xterm-256color`, procesa su salida en un estado y grid propios y la renderiza con `wgpu` y `glyphon`. `MODITTY_SHELL` permite indicar un ejecutable alternativo en ambas plataformas.

Incluye entrada de texto UTF-8, atajos de control, teclas de navegación habituales, resize de ventana y PTY, título OSC 0/2, pantalla alternativa y scrollback interno acotado. Cuando el shell termina, Moditty cierra la ventana.

El intérprete implementa los controles de línea, tabulación y retroceso, cursor CSI, regiones de scroll, inserción, borrado, pantalla alternativa y colores SGR de 8/16/256 colores, RGB y negrita.

Todavía no es una emulación VT completa: selección, navegación visual del scrollback, colores de fondo, decoraciones, protocolos de mouse y respuestas de dispositivo quedan fuera de M1. La única excepción es `CSI 6 n`, respondida como DSR de posición de cursor para completar el protocolo de inicio de ConPTY en Windows.

### Configuración gráfica base

Moditty carga opcionalmente `moditty.toml` desde `MODITTY_CONFIG`. Si esa variable no existe, busca `moditty/moditty.toml` bajo `XDG_CONFIG_HOME` (o `~/.config`) en Linux y `%APPDATA%` en Windows. La plantilla del repositorio está en `moditty.toml.example`. Las claves desconocidas se rechazan para evitar configuraciones con errores tipográficos.

La fuente puede venir del sistema o de cualquier archivo que el motor de fuentes pueda cargar. En una colección, `face_index` selecciona exactamente la cara indicada. Moditty no rechaza fuentes por no ser monoespaciadas; el grid conserva celdas lógicas fijas, aunque los avances visuales de una fuente proporcional pueden no coincidir con sus celdas. `size`, `line_height` y `cell_width` usan píxeles lógicos y se escalan con el DPI del monitor. Las fuentes corruptas o incompatibles producen un error al iniciar.

El color predeterminado de texto se define con `terminal.foreground.color` en formato `#RRGGBB`. Los programas que emiten colores ANSI mantienen sus propios colores; `SGR 0` y `SGR 39` restauran el color configurado. El texto y cursor son opacos: `opacity` no se aplica a ellos.

El fondo admite un color hexadecimal `#RRGGBB` o una imagen PNG, JPEG o GIF. Los GIF se animan según sus frames; `opacity` se aplica solo al fondo, multiplicándose con el alfa propio de la imagen. Las opciones `cover`, `contain` y `stretch` controlan su ajuste. Para evitar agotar recursos, cada imagen queda limitada a 8192×8192 píxeles y 64 MiB decodificados; un GIF admite hasta 300 frames y 128 MiB en total.

Cuando el fondo tiene alfa, Moditty solicita una ventana transparente al compositor. Si el compositor no expone una superficie compatible, la aplicación continúa con una superficie opaca.

`appearance.startup_splash` habilita una pantalla de inicio estética opcional.
Al activarla, `image` es obligatoria, se resuelve relativa al archivo
`moditty.toml` y debe referirse a una imagen PNG, JPG o JPEG estática. La imagen
se conserva a su tamaño natural en píxeles lógicos, se centra sobre la terminal
y desaparece después de `duration_ms`; `opacity` es independiente del fondo y
del texto. Los GIF y cualquier otro formato animado se rechazan expresamente
en esta pantalla hasta que exista un recurso de animación. La plantilla incluye
la sección completa y el comportamiento predeterminado es no mostrar splash.

### Diagnóstico del runtime

Las vistas internas de diagnóstico permiten comprobar visualmente la integración de M2 sin alterar la sesión del shell. Se activan desde `[input.shortcuts]` en `moditty.toml`; los valores predeterminados son `Ctrl+Shift+E` para eventos, `Ctrl+Shift+T` para ticks y `Ctrl+Shift+F` para frames. Cada atajo debe incluir al menos un modificador y una única letra ASCII.

La vista es modal y permanece abierta hasta pulsar `Escape`. Limpia sólo la representación visual de Moditty: el PTY continúa procesando la salida del shell, pero los caracteres escritos mientras el diagnóstico está activo no se envían a éste. Al volver, se muestra la terminal en su estado actual.

`events` muestra los sucesos canónicos de teclado, puntero, scroll, foco,
terminal y escena, incluidos los de drag. El texto imprimible y los eventos
personalizados se redactan. `ticks` y `frames` muestran contadores, `dt`,
ticks asociados al frame y ticks descartados por recuperación. Cada vista
conserva un buffer circular de las últimas 128 entradas relevantes.

### Herramientas de prueba

`testing_tools.scene` permite preparar una escena manual mínima para verificar
el comportamiento nativo del motor antes de que exista la API de mods. Está
desactivada por defecto y no crea un programa, un plugin ni una interfaz pública.
Al habilitarla, cada entrada de `[[testing_tools.scene.nodes]]` crea un rectángulo
de color. Sus coordenadas, dimensiones y `z_index` usan píxeles lógicos;
`opacity` afecta sólo ese rectángulo. Una entrada con `parent` usa
coordenadas locales respecto de ese padre; el orden de declaración no importa.

~~~toml
[testing_tools.scene]
enabled = true

[[testing_tools.scene.nodes]]
id = "underlay"
x = 24.0
y = 48.0
width = 280.0
height = 150.0
color = "#0EA5E9"
opacity = 0.55
layer = "underlay"

[[testing_tools.scene.nodes]]
id = "panel"
x = 48.0
y = 72.0
width = 160.0
height = 96.0
color = "#F97316"
opacity = 0.85
z_index = 0
layer = "overlay"
clip_children = true

[[testing_tools.scene.nodes]]
id = "clipped-child"
parent = "panel"
x = 120.0
y = 24.0
width = 96.0
height = 48.0
color = "#A855F7"
opacity = 0.9
~~~

Los identificadores deben ser únicos y los padres deben existir; ciclos,
referencias faltantes y una `layer` asignada a un hijo se rechazan. Una raíz
puede elegir `layer = "underlay"` o `"overlay"`; los hijos siempre heredan
la de su raíz. `clip_children = true` permite confirmar que la parte del hijo
fuera del padre desaparece y no recibe hover ni drag.

Al iniciar, Moditty escribe la relación entre cada `id` y su `NodeId` en
stderr; `Ctrl+Shift+E` muestra después los `SceneEvent` asociados a esos
handles. Así se pueden probar hover, foco, orden de superposición, clipping y
drag and drop sin recursos externos. Los nodos de prueba siguen las mismas
reglas de escena que los de producción; `Underlay` se dibuja detrás del texto
y `Overlay` delante, incluso durante diagnóstico. Una splash activa queda
sobre ellos hasta expirar.

## Orden aproximado de desarrollo

Después del terminal mínimo:

```text
M1  terminal mínima funcional
M2  EngineEvent propio + ciclo de vida del runtime
M3  NodeId + scene graph
M4  recursos gráficos y TextureId
M5  ImageNode
M6  hit testing
M7  mouse/focus
M8  drag and drop + herramientas de prueba
M9  clipping/layers + jerarquía de pruebas
M10 API interna estable
M11 STAPI
M12 protocolo de plugins
M13 STIDL
M14 primera extensión de lenguaje
```

La primera extensión de lenguaje probablemente será Python, pero esto no debe condicionar el diseño del protocolo.

## Principios que deben preservarse

1. Mantener el core pequeño.

2. Evitar introducir abstracciones antes de que exista una necesidad concreta.

3. No acoplar la API pública a crates externos.

4. No convertir Moditty en un toolkit de widgets.

5. El terminal convencional debe seguir funcionando sin ninguna extensión.

6. Los mods deben poder alterar profundamente la experiencia sin modificar el core.

7. El sistema de plugins debe ser independiente de Rust.

8. Usar handles en vez de referencias internas.

9. Separar protocolo de plugins, transporte y bindings de lenguaje.

10. Priorizar una arquitectura que permita reemplazar backends sin romper la API.

11. No sacrificar simplicidad del primer MVP intentando construir desde el inicio toda la plataforma futura.

12. Antes de añadir una dependencia grande, justificar qué problema concreto resuelve.

## Mentalidad de implementación

Cuando trabajes sobre este repositorio:

* preserva estas fronteras arquitectónicas;
* evita tomar decisiones irreversibles sin necesidad;
* implementa verticalmente;
* mantén cada milestone compilable;
* añade tests donde tengan sentido;
* ejecuta `cargo check`, `cargo test` y las herramientas de lint apropiadas después de cambios relevantes;
* no refactorices partes no relacionadas salvo necesidad;
* si una decisión arquitectónica contradice esta especificación, señalarla antes de implementarla.

Moditty debe comenzar siendo una terminal extraordinariamente simple y terminar siendo una plataforma extraordinariamente extensible.

La complejidad debe vivir progresivamente encima del núcleo, no dentro de él.
