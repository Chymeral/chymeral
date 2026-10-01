# Chymeral

Chymeral es un runtime gráfico minimalista, alojado por una terminal
programable y escrito en Rust.

Su superficie inicial se comporta como una terminal extremadamente mínima y
funcional. Su propósito es ofrecer a programas externos un runtime gráfico
portable con las primitivas necesarias para construir interfaces propias, sin
imponer un toolkit de widgets ni ocultar que el programa también conserva su
interfaz convencional de terminal.

Chymeral debe pensarse como un pequeño runtime gráfico guiado por eventos cuyo
objeto principal predeterminado es una terminal, no como un emulador de
terminal al que posteriormente se le añaden funciones.

## Objetivo del producto

La experiencia inicial de Chymeral debe ser deliberadamente mínima:

* abrir una ventana;
* ejecutar el shell del usuario mediante PTY;
* interpretar ANSI/VT;
* mantener un grid propio;
* renderizar texto;
* aceptar teclado;
* soportar resize y comportamiento normal de una terminal.

Sobre esa base, Chymeral debe proporcionar un contrato propio que permita
construir interfaces gráficas y elementos interactivos.

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

Estas capacidades deben ser primitivas del runtime. Chymeral no debe
convertirse directamente en un toolkit de widgets.

No deben existir primitivas de alto nivel como:

```text
create_button()
create_checkbox()
create_file_browser()
```

La API debe ofrecer primitivas sobre las cuales terceros puedan desarrollar sus
propios frameworks de interfaz.

Conceptualmente:

```text
Chymeral Runtime
     │
     ├── Terminal tradicional
     │
     ├── API y protocolo del motor
     │
     ├── Programas Chymeral
     │
     └── Frameworks de interfaz de terceros
```

Una aplicación tradicional como bash, vim o htop debe poder ejecutarse
normalmente sin conocer nada de Chymeral.

Una aplicación diseñada específicamente para Chymeral podrá usar las
capacidades avanzadas del runtime.

## Filosofía de plataforma

Chymeral trata las interfaces gráficas como programas ejecutados sobre una
plataforma. No existe una categoría conceptual de "mod": un programa Chymeral
es software distribuible con Chymeral como dependencia de instalación y con
acceso opcional a sus capacidades de runtime.

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

Los widgets, layouts complejos, editores, navegadores de archivos, interfaces visuales y demás abstracciones deben poder ser creados como librerías de programas externos.

El objetivo es que el ecosistema pueda construir cosas encima de Chymeral sin
necesitar modificar el core.

El espíritu del sistema debe ser similar al de un motor de videojuegos altamente modeable:

```text
engine
  ↓
primitives
  ↓
API
  ↓
programas/frameworks
  ↓
experiences
```

pero aplicado a una terminal y a aplicaciones gráficas.

## Arquitectura fundamental

El grid de terminal NO debe ser el sistema gráfico de Chymeral.

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

Debe quedar detrás de una abstracción propia de Chymeral.

Parser ANSI/VT:

```text
vte
```

Se utilizará como parser, pero Chymeral mantendrá su propio estado y grid de terminal.

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

No se deben exponer referencias o punteros internos a programas externos.

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

La API pública de Chymeral NO debe exponer tipos de dependencias externas.

Nunca hacer que una interfaz pública dependa directamente de tipos como:

```rust
winit::event::WindowEvent
wgpu::Texture
vte::Parser
cosmic_text::Buffer
portable_pty::PtyPair
```

Estas dependencias son detalles internos del backend.

Chymeral debe definir sus propios tipos:

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

    Focus(FocusEvent),

    Terminal(TerminalEvent),

    Custom(CustomEvent),
}
```

Los eventos de winit deben ser traducidos a eventos de Chymeral antes de llegar a sistemas superiores.

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
programas
```

## Ciclo de vida del runtime

El runtime de un programa tiene tres rutas internas. Son contratos de
ejecución, no una exigencia de usar tres hilos del sistema.

```text
Init     init() → start()
Event    press(), release(), scroll(), …
Runtime  tick(dt), frame(dt)
```

`init()` se invoca una vez antes de iniciar el programa. Permite cargar
argumentos, configuración, comandos y recursos iniciales, pero todavía no debe
recibir órdenes ni eventos de usuario.

`start()` se invoca una única vez después de que `init()` haya terminado con
éxito y el programa esté preparado para recibir órdenes. Ningún evento ni
llamada de runtime se entrega antes de `start()`.

La ruta `Event` entrega las entradas discretas mediante la API canónica del
runtime, por ejemplo `press()`, `release()` y `scroll()`. Estas entradas se
originan en `EngineEvent`, no en tipos de `winit` u otro backend.

La ruta `Runtime` mantiene la ejecución continua: `tick(dt)` actualiza la
lógica y `frame(dt)` prepara o renderiza un cuadro. Un frame puede no contener
ningún tick o contener varios; ambos deben operar únicamente después de
`start()`.

M2 concreta este contrato en el core mediante un `Program` interno:

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
programa interno activo, por lo que el host registra el fallo y cierra
limpiamente la aplicación. Este comportamiento sólo describe M2 y no impone la
política de aislamiento de los futuros programas externos.

`RuntimeCommand` es un canal separado de `EngineEvent`: una orden controla al
programa, mientras que un evento describe una entrada o suceso. Internamente
ambos comparten una única cola FIFO, por lo que se despachan al programa activo
en el orden exacto en que llegan. Cada iteración del scheduler vacía esa cola y
ejecuta cero o más `tick(dt)` de intervalo fijo. Cuando ocurre un redibujado,
después de esos pasos se invoca un único `frame(dt)`. El scheduler limita la
recuperación a ocho ticks por iteración y descarta retraso sobrante para no
bloquear la interfaz tras una pausa larga.

Este `Program` interno no es todavía una API ni un ABI público. Establece las
semánticas que M10 en adelante expondrán a programas externos mediante un
contrato de proceso. Un fallo de un futuro programa externo deberá cerrar su
sesión y liberar sus recursos sin cerrar la terminal anfitriona.

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
layouts, widgets ni APIs públicas de programas. Esos sistemas consumen esta estructura
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
nodos, repetición, sprites ni una API pública para programas.

## Interacción y colisiones

Las necesidades iniciales son de UI, no de física.

M6 introduce hit testing geométrico y backend-agnóstico. `Point` y `Rect` usan
píxeles lógicos; los rectángulos son semiabiertos, por lo que incluyen su borde
izquierdo y superior, pero excluyen el derecho e inferior. `Scene::hit_test`
devuelve el nodo visible más alto bajo un punto. `Scene::hit_test_rect` devuelve
todos los nodos visibles que se superponen con un rectángulo, de frente a fondo.
Ambas consultas usan los bounds mundiales y el mismo orden determinista de
pintura del scene graph.

M6 ofrece las consultas geométricas base. M13.2 las eleva para programas CLP
como overlap rectangular opt-in por sesión, con consulta de estado y
transiciones de entrada/salida. No es un sistema de física: cuerpos, respuesta
de colisión y colisión barrida siguen siendo trabajo futuro. `hit_test_rect`
sigue siendo una consulta interna del core; el contrato público de overlap no
expone consultas globales de la escena.

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
emite `DragEnd`. Chymeral no mueve nodos ni cambia su contenido automáticamente:
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

## Modelo de programas

Los consumidores de las capacidades avanzadas de Chymeral son programas, no
mods ni plugins cargados por el host. Un programa Chymeral se distribuye e
instala como cualquier otro programa de terminal; su paquete o instalador
declara una versión mínima de Chymeral como dependencia.

Un programa puede ser un binario nativo para Windows o Linux, o un lanzador
empaquetado que inicia un runtime como Python, Lua, JavaScript, .NET o Java.
El host no conoce ni selecciona el lenguaje: sólo inicia y se comunica con el
ejecutable indicado por el paquete. Por ello Rust, C, C++, Zig, Go y cualquier
otro lenguaje capaz de implementar el contrato tienen el mismo estatus.

Al ejecutarlo desde el shell contenido en Chymeral, debe sentirse normal:

```text
PS> mi-programa.exe
$ mi-programa
```

Sus argumentos, `stdin`, `stdout`, `stderr` y código de salida conservan la
semántica de un programa convencional. El shell y las aplicaciones que no
conocen Chymeral siguen funcionando sin cambios.

Un programa que use Chymeral hereda información privada de la sesión actual y
abre un canal con el host mediante su SDK. Por ese canal solicita recursos de
escena y recibe el ciclo de vida, eventos y tiempo del runtime; sus flujos
estándar continúan unidos al PTY.

```text
shell dentro de Chymeral
  └── programa
       ├── stdin / stdout / stderr ──> PTY y terminal convencional
       └── SDK de Chymeral ──────────> canal privado ──> host
```

Si el programa se inicia fuera de una sesión Chymeral, el SDK no tendrá un
endpoint al que conectarse. La política final —modo CLI alternativo o error
explícito de requisito— corresponde al programa; Chymeral no requiere un
servicio global para que sus programas se ejecuten.

## Contrato entre programa y runtime

La frontera pública es un protocolo local, versionado y neutral al lenguaje.
Su nombre es **CLP** (*Chymeral Link Protocol*). No es una syscall:
es un contrato de mensajes entre procesos que evita exponer memoria, tipos de
Rust o dependencias gráficas del host.

M10 implementa dentro del core el primer contrato de CLP y M11 lo conecta a un
transporte local real. M12 elevó el protocolo a `1.1` con comandos de escena y
recursos RGBA8 aislados. M13 lo elevó a `1.2` y entrega entrada, tiempo,
redimensionamiento y cierre al programa conectado. M13.2 lo eleva a `1.3` con
consulta y transiciones de overlap geométrico opcionales.

CLP tendrá dos direcciones lógicas en una sesión bidireccional:

```text
Chymeral → programa: Init, Start, Event, Tick, Frame, Resize, errores
programa → Chymeral: comandos, lotes de escena, recursos, suscripciones
```

`Init`, `Start`, `Event`, `Tick` y `Frame` preservarán las semánticas ya
validadas por M2. Los comandos permanecen separados de los eventos, aunque el
protocolo determinará de forma explícita el orden, los límites de lote y las
respuestas a fallos. Todos los identificadores de nodos y recursos serán
handles opacos propiedad de la sesión del programa.

El host valida cada comando antes de aplicarlo, asocia los recursos a la
conexión que los creó y los libera al cerrar, fallar o desconectar el programa.
Un programa no obtiene referencias ni acceso directo a la memoria del host.

M11 usa un transporte local privado: socket de dominio Unix en Linux y named
pipe en Windows. Cada ventana crea un endpoint efímero y un token aleatorio de
256 bits; el shell y todo proceso que éste inicie reciben ambos mediante
`CHYMERAL_CLP_ENDPOINT` y `CHYMERAL_CLP_TOKEN`. No se escriben en configuración,
no persisten tras cerrar la ventana y no existe un servicio global al que un
proceso externo pueda conectarse. El token evita conexiones accidentales, pero
M11 no pretende ser un sandbox ni sustituye la seguridad del usuario del
sistema.

CLP sigue separado de ese transporte para permitir adaptadores futuros sin
alterar la API, pero `stdio`, red, memoria compartida, WASM y carga de código
nativo quedan fuera de este alcance inicial.

### Transporte CLP v1.3 (M11–M13.2)

El flujo de bytes está compuesto por frames con una longitud `u32` big-endian,
un discriminante de un byte y su payload. `J` identifica un objeto JSON UTF-8;
`B`, un fragmento binario. Cada frame, incluido el discriminante, tiene un
límite de 1 MiB. Los identificadores se serializan como strings para no perder
precisión en lenguajes cuyo tipo numérico no representa todos los enteros de
64 bits.

La primera frame debe ser `hello` y debe llegar dentro de cinco segundos. Lleva
la versión `{ "major": 1, "minor": 3 }`, el token heredado y la lista de
capacidades solicitadas (`scene`, `resources`, `input`, `timing`, `overlap`). Tras validar
el token y negociar capacidades, la sesión sigue exactamente este orden:

```text
programa → hello
Chymeral → init
programa → init_complete
Chymeral → start
programa → started
          running
```

El host acepta el mismo major y un minor no superior al suyo; `init` devuelve
la versión menor negociada. Una sesión `1.2` sigue funcionando contra un host
`1.3`, pero nunca recibe ni puede invocar la capacidad `overlap`; un minor
futuro o un major distinto se rechazan.

`init` contiene el id de sesión, la versión concedida, capacidades concedidas y
un contexto inicial. M11 entrega por ahora un contexto vacío: el lanzador y los
argumentos de programa entrarán en el alcance de paquetes posterior. Durante o
después del handshake, el programa puede enviar `fault` con su fase y mensaje,
o `close`; el host responde `closed` y elimina sólo esa sesión. Una desconexión,
mensaje malformado, orden inválido o timeout también elimina esa sesión sin
afectar a las demás.

Como límite de recursos, una ventana admite hasta 16 conexiones que aún no han
completado `started`; las excedentes se cierran inmediatamente sin crear una
sesión o se rechazan por capacidad antes de establecer el stream. Las sesiones
que ya están ejecutándose no comparten ese límite. M13 añade además una cola
host→programa acotada a 64 mensajes por sesión. `tick`, `frame`, movimientos
de puntero, `DragMove` y `Terminal::OutputAvailable` son continuos: si ya hay
una muestra pendiente de la misma clase, el host conserva sólo la última y la
coloca después de toda entrada discreta pendiente. Si debe omitir una muestra
continua, emite `delivery_gap` con sus categorías afectadas antes de reanudar
la entrega. Una entrada discreta o `resize` desplaza una muestra continua
pendiente cuando es posible; si la cola contiene únicamente entradas discretas,
Chymeral termina sólo esa sesión con `host_overloaded` en vez de perder una
tecla, clic o cierre silenciosamente.

Para observabilidad del host, `ClpServer::notification_metrics()` ofrece una
instantánea acumulada de muestras coalescidas, muestras continuas descartadas,
`delivery_gap` emitidos y sesiones terminadas por sobrecarga. Son métricas
internas del host, no mensajes CLP ni una API de SDK; permiten detectar presión
de transporte sin exponer el endpoint, token o contenido de entrada.

Al entrar en ejecución, el host activa la sesión en el hilo de la ventana. Si
tiene `input`, su primera notificación siempre es `resize`; los eventos y los
callbacks de tiempo se suprimen hasta entonces. Después de `started`, el
programa puede reemplazar sus suscripciones con:

```json
{ "type": "subscribe", "subscriptions": ["raw_input", "terminal", "timing"] }
```

El host confirma el conjunto efectivo con `subscribed`. `raw_input` y
`terminal` exigen la capacidad `input`; `timing`, la capacidad `timing`. Una
suscripción no concedida recibe `error { code: "capability_denied" }` y no
altera el conjunto anterior.

La entrega se separa deliberadamente por alcance:

```text
input sin suscripción → resize y foco de ventana
input + escena         → eventos de los nodos que posee la sesión
input + foco de nodo   → teclado; sin nodo de programa enfocado, va al terminal
input + raw_input      → puntero y scroll crudos del programa enfocado;
                         Custom se difunde a las sesiones suscritas
input + terminal       → estado del terminal
timing + timing        → tick y frame
```

El window manager decide qué ventana Chymeral recibe entrada. Dentro de esa
única ventana activa, un clic primario sobre un nodo de programa actualiza el
foco de escena y desvía el teclado a la sesión propietaria; un clic en espacio
vacío devuelve el teclado al terminal. Los eventos de escena no requieren una
suscripción adicional: son ya dirigidos por ownership.

`event` serializa todos los tipos canónicos ya existentes: `key`, puntero,
scroll, foco de ventana, terminal y eventos personalizados; conserva teclas,
modificadores y coordenadas lógicas sin filtrar tipos de `winit`. También
transporta el canal derivado de escena: `PointerEnter`, `PointerLeave`,
`PointerMove`, `PointerPress`, `PointerRelease`, `FocusGained`, `FocusLost`,
`DragStart`, `DragEnter`, `DragLeave`, `DragMove`, `Drop`, `DragEnd` y
`DragCancel`, `OverlapEnter` y `OverlapLeave`.

Los eventos de escena usan `node-N`, nunca `NodeId`. Un programa recibe sólo
los handles que posee. Si un drag conecta nodos de sesiones diferentes, cada
sesión recibe el evento aplicable con su propio handle y el extremo ajeno como
`null`; así puede reaccionar sin obtener una referencia a recursos de otro
programa. `resize` se entrega primero al activarse una sesión con `input` y
tras cada redimensionamiento efectivo; incluye dimensiones físicas,
dimensiones lógicas y `scale_factor`.

La capacidad `overlap` añade geometría para programas sin mouse —por ejemplo,
juegos controlados por teclado— sin introducir física. Un nodo declara
`overlap_enabled: true` al crearse o mediante `update_node`; el valor por
defecto es `false` y una sesión puede habilitar como máximo 1024 nodos. Después
de cada creación, `update_node` o `reparent` exitosos, Chymeral compara los
rectángulos lógicos mundiales de los nodos habilitados de esa sesión y entrega
`event { kind: "scene", event: { kind: "overlap_enter", first, second } }`
o su variante `overlap_leave` al cruzar una transición. No intervienen puntero,
foco, visibilidad, contenido, clipping ni `z_index`; un nodo invisible puede
ser un collider. Los rectángulos son semiabiertos, por lo que tocar sólo un
borde no es overlap. No hay comparación ni filtración de handles entre
sesiones.

`query_overlaps` recibe cualquier `node-N` propio y responde
`result { kind: "overlaps", overlapping, nodes }`. `nodes` contiene, en orden
determinista, los nodos propios habilitados que intersectan el rectángulo
mundial actual del nodo consultado; el nodo consultado no necesita estar
habilitado. Un programa puede consultar inmediatamente después de mover un
nodo y restaurar su última posición válida si hay contactos. Los eventos de
overlap son discretos, no requieren suscripción y sólo se entregan a una
sesión que negoció `overlap`; nunca se coalescen silenciosamente. Eliminar un
nodo o cerrar su sesión cancela sus pares sin emitir `overlap_leave`, pues el
programa ya conoce la eliminación o el cierre. Crear o actualizar
`overlap_enabled`, y ejecutar `query_overlaps`, exigen las capacidades `scene`
y `overlap`; sin ellas el comando devuelve `capability_denied`.

`tick` y `frame` respetan el orden del scheduler de M2: un frame puede seguir
a cero o varios ticks. Ambos llevan `dt_ns` como string decimal para conservar
precisión en todos los lenguajes. El host conserva el orden de las
notificaciones discretas de cada sesión y no deja que una muestra continua
coalescida las adelante. Si una sesión lenta pierde un flujo continuo, antes
de reanudarlo recibe `delivery_gap` con las categorías afectadas (`raw_input`,
`scene`, `terminal`, `timing`); debe reconstruir el estado que dependa de ese
flujo. Los eventos discretos y `resize` nunca se descartan de forma
silenciosa: si no caben, termina sólo esa sesión con `host_overloaded`.

Al apagar el host, cada programa en ejecución recibe primero
`closing { reason: host_shutdown }`. Puede responder `close` durante una
ventana de gracia de 250 ms; entonces, o al expirar la ventana, Chymeral envía
el `closed` terminal con la misma razón y limpia los recursos de esa sesión.
Los cierres iniciados por el cliente, por fallo o por violación de protocolo
siguen entregando directamente su `closed` terminal. M13 no inicia programas,
no interpreta sus argumentos ni expone un SDK: M14 encapsulará este contrato
crudo y probado.

### Comandos de escena M12

Una vez que el programa ha enviado `started`, puede emitir una operación JSON
ordenada. Cada comando lleva un `request_id` no vacío elegido por el cliente y
recibe exactamente una respuesta `result` o `command_error` con el mismo id.
No hay lotes todavía: el siguiente comando se interpreta sólo después de que
el anterior haya respondido.

```text
programa → { type: "command", request_id, command: { op: ... } }
Chymeral → { type: "result", request_id, result: ... }
Chymeral → { type: "command_error", request_id, code, message }
```

Las operaciones de escena son `create_root`, `create_child`, `update_node`,
`set_content`, `reparent`, `remove_node` y `query_overlaps`. Una creación recibe
propiedades lógicas `{ x, y, width, height, visible, clip_children, z_index,
overlap_enabled }`; las
actualizaciones son parciales. Los hijos expresan coordenadas locales como el
scene graph existente. Un root puede declarar `layer: "underlay"` u
`"overlay"`; si se omite, usa `overlay`.

El contenido de un nodo es `empty`, `solid` con `color: [r, g, b, a]`, o
`image` con un handle de textura y opacidad independiente. Los rectángulos
sólidos e imágenes comparten el mismo espacio lógico, clipping, orden de
pintura y composición de M3–M9. `underlay` queda detrás del texto de terminal;
`overlay`, delante.

`upload_texture` declara `width` y `height`; debe seguirle inmediatamente una
secuencia de frames `B` que contenga exactamente `width * height * 4` bytes
RGBA8. La respuesta entrega `texture-N`. No existe Base64, rutas del host ni
lectura de archivos por parte de Chymeral. `destroy_texture` sólo tiene éxito
cuando ningún nodo la referencia: el programa debe primero cambiar o eliminar
esos nodos.

Los handles `node-N` y `texture-N` son opacos y exclusivos de la sesión que
los creó. Cada operación valida propiedad, capacidades, geometría y ciclo de
vida antes de tocar el scene graph. Al cerrar, fallar o desconectar una sesión,
el host elimina sus nodos y luego sus texturas, sin afectar a las demás. Una
textura individual puede medir hasta 64 MiB RGBA8 y una sesión puede retener
hasta 128 MiB; además, ancho y alto no pueden superar el límite de la GPU
seleccionada por la ventana. Sobrepasar esos límites devuelve `command_error`
recuperable.

El handler CLP nunca muta `wgpu`, `Scene` ni `TextureStore` directamente.
Encola los comandos en una cola acotada para el hilo de ventana, espera la
respuesta y éste reconcilia interacción y solicita el redraw. Las notificaciones
de limpieza viajan por un canal separado para que cerrar muchas sesiones nunca
bloquee el apagado. Esa frontera mantiene el protocolo independiente del backend
gráfico y preserva el acceso de un solo hilo a la UI.

## SDKs de cliente e independencia de lenguaje

Un SDK es una biblioteca de cliente enlazada, incluida o implementada por el
programa. Traduce las convenciones de un lenguaje a CLP; no es un plugin que
Chymeral cargue ni un requisito de instalación dentro del host.

```text
Python / Lua / JavaScript / C# / Go / Rust / C / C++ / Zig
                         ↓
                SDK o implementación propia de CLP
                         ↓
                     Chymeral
```

Un programa nativo puede enlazar un SDK estático y distribuir un único binario.
Un runtime interpretado puede incluir su launcher y dependencias en su paquete.
Los SDKs oficiales son conveniencia y conformance, no una lista cerrada de
lenguajes permitidos: cualquier implementación correcta de CLP es válida.

El nombre **MLExtension** queda descartado como pieza arquitectónica central.
Podrá reutilizarse más adelante para un adaptador opcional de un lenguaje, pero
no habrá un directorio de extensiones por lenguaje ni DLL/SO cargadas por el
host para ejecutar programas ordinarios.

## Paquetes y manifiestos de programa

Los instaladores de programas resuelven la dependencia de Chymeral y registran
su comando en el entorno del usuario como harían con cualquier aplicación. Un
manifiesto de programa, cuando sea necesario, describirá sólo metadatos de
ejecución y compatibilidad, nunca el lenguaje como parte del contrato.

Los campos iniciales previstos son identificador, versión mínima de CLP,
ejecutable o launcher por plataforma y capacidades solicitadas. La forma del
paquete, el gestor de instalación y un registro central de programas no forman
parte del core inicial.

## Esquema de protocolo e IDL futura

CLP comenzará con mensajes tipados para el ciclo de vida, eventos y comandos
que realmente existan. No se añadirá una llamada genérica de valores dinámicos
ni una IDL antes de que el conjunto inicial de comandos se estabilice.

Cuando haya más de un SDK, una IDL neutral podrá convertirse en la fuente de
verdad de CLP y generar bindings, documentación y pruebas de conformidad. Esa
IDL debe describir handles, mensajes, errores, versiones y capacidades; no debe
filtrar tipos de `winit`, `wgpu`, Rust ni ningún backend de Chymeral.

## Estructura conceptual del proyecto

La estructura futura puede aproximarse a:

```text
chymeral/
├── Cargo.toml
├── crates/
│   ├── chymeral-core/
│   ├── chymeral-events/
│   ├── chymeral-scene/
│   ├── chymeral-render/
│   ├── chymeral-render-wgpu/
│   ├── chymeral-text/
│   ├── chymeral-terminal/
│   ├── chymeral-pty/
│   ├── chymeral-protocol/
│   └── chymeral-session/
│
├── protocol/
└── examples/
```

No crear crates vacíos solo para satisfacer esta estructura.

Crear únicamente los crates necesarios para cada milestone y separar componentes cuando exista una frontera arquitectónica real.

## Alcance original de M1

M1 no implementa programas externos, CLP, SDKs ni una API pública.

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

La implementación actual abre una ventana, crea una sesión de terminal con el shell definido en `$SHELL` (o `/bin/sh`) en Linux y `COMSPEC` (o `cmd.exe`) en Windows, usa `TERM=xterm-256color`, procesa su salida en un estado y grid propios y la renderiza con `wgpu` y `glyphon`. `CHYMERAL_SHELL` permite indicar un ejecutable alternativo en ambas plataformas.

Incluye entrada de texto UTF-8, atajos de control, teclas de navegación habituales, resize de ventana y PTY, título OSC 0/2, pantalla alternativa y scrollback interno acotado. Cuando el shell termina, Chymeral cierra la ventana.

El intérprete implementa los controles de línea, tabulación y retroceso, cursor CSI, regiones de scroll, inserción, borrado, pantalla alternativa y colores SGR de 8/16/256 colores, RGB y negrita.

Todavía no es una emulación VT completa: selección, navegación visual del scrollback, colores de fondo, decoraciones, protocolos de mouse y respuestas de dispositivo quedan fuera de M1. La única excepción es `CSI 6 n`, respondida como DSR de posición de cursor para completar el protocolo de inicio de ConPTY en Windows.

### Configuración gráfica base

Chymeral carga opcionalmente `chymeral.toml` desde `CHYMERAL_CONFIG`. Si esa variable no existe, busca `chymeral/chymeral.toml` bajo `XDG_CONFIG_HOME` (o `~/.config`) en Linux y `%APPDATA%` en Windows. La plantilla del repositorio está en `chymeral.toml.example`. Las claves desconocidas se rechazan para evitar configuraciones con errores tipográficos.

La fuente puede venir del sistema o de cualquier archivo que el motor de fuentes pueda cargar. En una colección, `face_index` selecciona exactamente la cara indicada. Chymeral no rechaza fuentes por no ser monoespaciadas; el grid conserva celdas lógicas fijas, aunque los avances visuales de una fuente proporcional pueden no coincidir con sus celdas. `size`, `line_height` y `cell_width` usan píxeles lógicos y se escalan con el DPI del monitor. Las fuentes corruptas o incompatibles producen un error al iniciar.

El color predeterminado de texto se define con `terminal.foreground.color` en formato `#RRGGBB`. Los programas que emiten colores ANSI mantienen sus propios colores; `SGR 0` y `SGR 39` restauran el color configurado. El texto y cursor son opacos: `opacity` no se aplica a ellos.

El fondo admite un color hexadecimal `#RRGGBB` o una imagen PNG, JPEG o GIF. Los GIF se animan según sus frames; `opacity` se aplica solo al fondo, multiplicándose con el alfa propio de la imagen. Las opciones `cover`, `contain` y `stretch` controlan su ajuste. Para evitar agotar recursos, cada imagen queda limitada a 8192×8192 píxeles y 64 MiB decodificados; un GIF admite hasta 300 frames y 128 MiB en total.

Cuando el fondo tiene alfa, Chymeral solicita una ventana transparente al compositor. Si el compositor no expone una superficie compatible, la aplicación continúa con una superficie opaca.

`appearance.startup_splash` habilita una pantalla de inicio estética opcional.
Al activarla, `image` es obligatoria, se resuelve relativa al archivo
`chymeral.toml` y debe referirse a una imagen PNG, JPG o JPEG estática. La imagen
se conserva a su tamaño natural en píxeles lógicos, se centra sobre la terminal
y desaparece después de `duration_ms`; `opacity` es independiente del fondo y
del texto. Los GIF y cualquier otro formato animado se rechazan expresamente
en esta pantalla hasta que exista un recurso de animación. La plantilla incluye
la sección completa y el comportamiento predeterminado es no mostrar splash.

### Diagnóstico del runtime

Las vistas internas de diagnóstico permiten comprobar visualmente la integración de M2 sin alterar la sesión del shell. Se activan desde `[input.shortcuts]` en `chymeral.toml`; los valores predeterminados son `Ctrl+Shift+E` para eventos, `Ctrl+Shift+T` para ticks y `Ctrl+Shift+F` para frames. Cada atajo debe incluir al menos un modificador y una única letra ASCII.

La vista es modal y permanece abierta hasta pulsar `Escape`. Limpia sólo la representación visual de Chymeral: el PTY continúa procesando la salida del shell, pero los caracteres escritos mientras el diagnóstico está activo no se envían a éste. Al volver, se muestra la terminal en su estado actual.

`events` muestra los sucesos canónicos de teclado, puntero, scroll, foco,
terminal y escena, incluidos los de drag. El texto imprimible y los eventos
personalizados se redactan. `ticks` y `frames` muestran contadores, `dt`,
ticks asociados al frame y ticks descartados por recuperación. Cada vista
conserva un buffer circular de las últimas 128 entradas relevantes.

### Herramientas de prueba

`testing_tools.scene` permite preparar una escena manual mínima para verificar
el comportamiento nativo del motor antes de que exista la API de programas.
Está desactivada por defecto y no crea un programa ni una interfaz pública.
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

Al iniciar, Chymeral escribe la relación entre cada `id` y su `NodeId` en
stderr; `Ctrl+Shift+E` muestra después los `SceneEvent` asociados a esos
handles. Así se pueden probar hover, foco, orden de superposición, clipping y
drag and drop sin recursos externos. Los nodos de prueba siguen las mismas
reglas de escena que los de producción; `Underlay` se dibuja detrás del texto
y `Overlay` delante, incluso durante diagnóstico. Una splash activa queda
sobre ellos hasta expirar.

## Orden aproximado de desarrollo

M1–M13.2 están completados. `testing_tools.scene` y el `Program` de M2 son
infraestructura interna de prueba; no son un mecanismo de ejecución de
aplicaciones ni una API pública.

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
M10 contrato de programa y tipos internos de CLP ✓
M11 sesión local CLP: transporte, handshake, Init/Start y limpieza ✓
M12 comandos de escena y recursos con ownership por programa ✓
M13 entrega de Event/Tick/Frame, Resize y ciclo de cierre de programa ✓
M13.2 overlap geométrico opcional por sesión, consulta y transiciones ✓
M14 SDK de referencia, pruebas de conformidad y primer programa de prueba
M15 manifiesto mínimo de programa, capacidades y compatibilidad de paquete
M16 IDL sólo si más de un SDK justifica generación de bindings
```

El lenguaje del primer SDK o programa de prueba no condicionará CLP. Python,
Lua, JavaScript, C#, Go, Rust, C, C++ y Zig son candidatos equivalentes;
también será válida una implementación independiente del protocolo.

### Estado de M10, M11, M12 y M13

M10 define y prueba dentro del core el contrato que un programa externo usará:
versión de CLP, identidad y capacidades de sesión, mensajes de handshake,
transiciones `Init`/`Start`, mensajes de fallo y handles aislados por sesión.
Las sesiones son concurrentes; un fallo o cierre libera sólo los handles de su
programa. Después de entregar `Closed`, el host puede descartar el registro
terminado sin afectar las sesiones activas. El contrato mantiene la semántica
canónica de M2 sin filtrar tipos del backend.

M11 abre el endpoint privado antes del shell e inyecta sus dos variables de
entorno en éste; por herencia, también llegan a los programas que ejecute. Sus
tests recorren el framing, token inválido, negociación, el handshake completo,
el cierre, `HostShutdown`, el límite de handshakes y la limpieza al desconectar.
Una prueba de integración inicia además un proceso hijo independiente que sólo
recibe las variables heredadas e implementa el framing CLP crudo; recorre
activación, `resize`, suscripciones, eventos, `tick`, `frame` y cierre. M11 no
inicia programas por sí mismo, no expone comandos de escena a aplicaciones y
no crea SDKs. M12 añade comandos seriales de escena, rectángulos sólidos,
imágenes de texturas RGBA8 transferidas en fragmentos binarios, validación de
capacidad y propiedad, límites de memoria y limpieza de recursos al terminar
una sesión. Sus pruebas cubren ownership, limpieza, intento de destrucción de
una textura en uso y el recorrido real de un upload binario hacia el hilo de
host. M13
añade una cola host→programa aislada, entrada canónica serializada, callbacks
de tick/frame, resize inicial y posterior, suscripciones explícitas para
flujos optativos, enrutamiento de teclado por foco de nodo, recuperación de
flujos continuos perdidos y el cierre anunciado durante el apagado. Los
paquetes y SDKs corresponden a M14–M15. El endurecimiento final de M13 prueba
la coalescencia y recuperación desde un proceso heredado independiente tanto
en sockets Unix como en named pipes de Windows; también expone métricas de
presión del host sin convertirlas en parte del protocolo público. M13.2 añade
overlap de rectángulos lógicos entre nodos propios que el programa habilita
explícitamente; no depende del mouse, no revela otros programas y se consulta
con el mismo contrato serial de comandos.

## Principios que deben preservarse

1. Mantener el core pequeño.

2. Evitar introducir abstracciones antes de que exista una necesidad concreta.

3. No acoplar la API pública a crates externos.

4. No convertir Chymeral en un toolkit de widgets.

5. El terminal convencional debe seguir funcionando sin ningún programa cliente.

6. Los programas Chymeral deben poder alterar profundamente la experiencia sin modificar el core.

7. El protocolo y los SDKs deben ser independientes de Rust.

8. Usar handles en vez de referencias internas.

9. Separar protocolo de programas, transporte y SDKs de lenguaje.

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

Chymeral debe comenzar siendo una terminal extraordinariamente simple y terminar siendo una plataforma extraordinariamente extensible.

La complejidad debe vivir progresivamente encima del núcleo, no dentro de él.
