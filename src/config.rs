use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use moditty::scene::SceneLayer;
use serde::Deserialize;

use crate::diagnostics::{DiagnosticsShortcuts, Shortcut};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Rgb {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Appearance {
    pub font: FontAppearance,
    pub foreground: Foreground,
    pub background: Background,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            font: FontAppearance {
                source: FontSource::System("monospace".to_owned()),
                size: 16.0,
                line_height: 20.0,
                cell_width: CellWidth::Auto,
            },
            foreground: Foreground {
                color: Rgb::new(229, 231, 235),
            },
            background: Background::Color {
                color: Rgb::new(9, 11, 15),
                opacity: 1.0,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FontAppearance {
    pub source: FontSource,
    pub size: f32,
    pub line_height: f32,
    pub cell_width: CellWidth,
}

impl FontAppearance {
    pub fn resolved_cell_width(&self) -> f32 {
        match self.cell_width {
            CellWidth::Auto => (self.size * 0.6).max(1.0),
            CellWidth::Fixed(width) => width,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontSource {
    System(String),
    File { path: PathBuf, face_index: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CellWidth {
    Auto,
    Fixed(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Foreground {
    pub color: Rgb,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Background {
    Color {
        color: Rgb,
        opacity: f32,
    },
    Image {
        path: PathBuf,
        opacity: f32,
        fit: ImageFit,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct StartupSplash {
    pub path: PathBuf,
    pub duration: Duration,
    pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TestingScene {
    pub nodes: Vec<TestingNode>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TestingNode {
    pub id: String,
    pub parent: Option<String>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub color: Rgb,
    pub opacity: f32,
    pub clip_children: bool,
    pub z_index: i32,
    pub layer: Option<SceneLayer>,
}

impl Background {
    pub fn opacity(&self) -> f32 {
        match self {
            Self::Color { opacity, .. } | Self::Image { opacity, .. } => *opacity,
        }
    }

    pub fn requires_window_transparency(&self) -> bool {
        matches!(self, Self::Image { .. }) || self.opacity() < 1.0
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageFit {
    #[default]
    Cover,
    Contain,
    Stretch,
}

pub struct LoadedConfig {
    pub appearance: Appearance,
    pub startup_splash: Option<StartupSplash>,
    pub diagnostics: DiagnosticsShortcuts,
    pub testing_scene: Option<TestingScene>,
    pub path: Option<PathBuf>,
}

pub fn load() -> Result<LoadedConfig> {
    let Some(path) = config_path() else {
        return Ok(LoadedConfig {
            appearance: Appearance::default(),
            startup_splash: None,
            diagnostics: DiagnosticsShortcuts::default(),
            testing_scene: None,
            path: None,
        });
    };

    if !path.exists() {
        return Ok(LoadedConfig {
            appearance: Appearance::default(),
            startup_splash: None,
            diagnostics: DiagnosticsShortcuts::default(),
            testing_scene: None,
            path: None,
        });
    }

    let contents = fs::read_to_string(&path)
        .with_context(|| format!("could not read configuration file {}", path.display()))?;
    let base_directory = path.parent().unwrap_or_else(|| Path::new("."));
    let parsed = parse_config(&contents, base_directory)
        .with_context(|| format!("invalid configuration in {}", path.display()))?;

    Ok(LoadedConfig {
        appearance: parsed.appearance,
        startup_splash: parsed.startup_splash,
        diagnostics: parsed.diagnostics,
        testing_scene: parsed.testing_scene,
        path: Some(path),
    })
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("MODITTY_CONFIG") {
        return Some(PathBuf::from(path));
    }

    platform_config_path()
}

#[cfg(target_os = "windows")]
fn platform_config_path() -> Option<PathBuf> {
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| home_directory().map(|home| home.join("AppData").join("Roaming")))
        .map(|directory| directory.join("moditty").join("moditty.toml"))
}

#[cfg(not(target_os = "windows"))]
fn platform_config_path() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_directory().map(|home| home.join(".config")))
        .map(|directory| directory.join("moditty").join("moditty.toml"))
}

fn home_directory() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
fn parse(contents: &str, base_directory: &Path) -> Result<Appearance> {
    Ok(parse_config(contents, base_directory)?.appearance)
}

struct ParsedConfig {
    appearance: Appearance,
    startup_splash: Option<StartupSplash>,
    diagnostics: DiagnosticsShortcuts,
    testing_scene: Option<TestingScene>,
}

fn parse_config(contents: &str, base_directory: &Path) -> Result<ParsedConfig> {
    let raw: RawConfig = toml::from_str(contents).context("could not parse TOML")?;
    let RawConfig {
        terminal,
        appearance,
        input,
        testing_tools,
    } = raw;
    Ok(ParsedConfig {
        appearance: terminal.into_appearance(base_directory)?,
        startup_splash: appearance.into_startup_splash(base_directory)?,
        diagnostics: input.shortcuts.into_shortcuts()?,
        testing_scene: testing_tools.into_scene()?,
    })
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    terminal: RawTerminal,
    #[serde(default)]
    appearance: RawAppearance,
    #[serde(default)]
    input: RawInput,
    #[serde(default)]
    testing_tools: RawTestingTools,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTestingTools {
    #[serde(default)]
    scene: RawTestingScene,
}

impl RawTestingTools {
    fn into_scene(self) -> Result<Option<TestingScene>> {
        self.scene.into_scene()
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTestingScene {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    nodes: Vec<RawTestingNode>,
}

impl RawTestingScene {
    fn into_scene(self) -> Result<Option<TestingScene>> {
        if !self.enabled {
            return Ok(None);
        }
        if self.nodes.is_empty() {
            bail!("testing_tools.scene.nodes must contain at least one node when enabled = true");
        }

        let mut ids = HashSet::new();
        let nodes = self
            .nodes
            .into_iter()
            .map(|node| node.into_node(&mut ids))
            .collect::<Result<Vec<_>>>()?;
        validate_testing_hierarchy(&nodes)?;
        Ok(Some(TestingScene { nodes }))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTestingNode {
    id: String,
    parent: Option<String>,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    color: String,
    #[serde(default = "default_opacity")]
    opacity: f32,
    #[serde(default)]
    clip_children: bool,
    #[serde(default)]
    z_index: i32,
    layer: Option<RawSceneLayer>,
}

impl RawTestingNode {
    fn into_node(self, ids: &mut HashSet<String>) -> Result<TestingNode> {
        let id = self.id.trim().to_owned();
        if id.is_empty() {
            bail!("testing_tools.scene.nodes[].id must not be empty");
        }
        if !ids.insert(id.clone()) {
            bail!("testing_tools.scene.nodes[].id {id:?} is duplicated");
        }
        let parent = self.parent.map(|parent| parent.trim().to_owned());
        if parent.as_ref().is_some_and(String::is_empty) {
            bail!("testing_tools.scene.nodes[].parent must not be empty");
        }
        if parent.is_some() && self.layer.is_some() {
            bail!("testing_tools.scene.nodes[].layer is only valid on root nodes");
        }
        validate_finite("testing_tools.scene.nodes[].x", self.x)?;
        validate_finite("testing_tools.scene.nodes[].y", self.y)?;
        validate_positive_f64("testing_tools.scene.nodes[].width", self.width)?;
        validate_positive_f64("testing_tools.scene.nodes[].height", self.height)?;
        if !(self.x + self.width).is_finite() || !(self.y + self.height).is_finite() {
            bail!("testing_tools.scene.nodes[] bounds must remain finite");
        }
        if !self.opacity.is_finite() || !(0.0..=1.0).contains(&self.opacity) {
            bail!("testing_tools.scene.nodes[].opacity must be between 0.0 and 1.0");
        }
        Ok(TestingNode {
            id,
            parent,
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            color: parse_hex_color("testing_tools.scene.nodes[].color", &self.color)?,
            opacity: self.opacity,
            clip_children: self.clip_children,
            z_index: self.z_index,
            layer: self.layer.map(RawSceneLayer::into_layer),
        })
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawSceneLayer {
    Underlay,
    Overlay,
}

impl RawSceneLayer {
    const fn into_layer(self) -> SceneLayer {
        match self {
            Self::Underlay => SceneLayer::Underlay,
            Self::Overlay => SceneLayer::Overlay,
        }
    }
}

fn validate_testing_hierarchy(nodes: &[TestingNode]) -> Result<()> {
    let parents = nodes
        .iter()
        .map(|node| (node.id.as_str(), node.parent.as_deref()))
        .collect::<HashMap<_, _>>();
    for node in nodes {
        let mut visited = HashSet::new();
        let mut current = Some(node.id.as_str());
        while let Some(id) = current {
            if !visited.insert(id) {
                bail!("testing_tools.scene.nodes contain a parent cycle at {id:?}");
            }
            current = parents
                .get(id)
                .with_context(|| {
                    format!("testing_tools.scene.nodes[].parent references unknown node {id:?}")
                })?
                .to_owned();
        }
    }
    Ok(())
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAppearance {
    #[serde(default)]
    startup_splash: RawStartupSplash,
}

impl RawAppearance {
    fn into_startup_splash(self, base_directory: &Path) -> Result<Option<StartupSplash>> {
        self.startup_splash.into_splash(base_directory)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStartupSplash {
    #[serde(default)]
    enabled: bool,
    image: Option<PathBuf>,
    #[serde(default = "default_splash_duration_ms")]
    duration_ms: u64,
    #[serde(default = "default_opacity")]
    opacity: f32,
}

impl Default for RawStartupSplash {
    fn default() -> Self {
        Self {
            enabled: false,
            image: None,
            duration_ms: default_splash_duration_ms(),
            opacity: default_opacity(),
        }
    }
}

impl RawStartupSplash {
    fn into_splash(self, base_directory: &Path) -> Result<Option<StartupSplash>> {
        if !self.enabled {
            return Ok(None);
        }
        let path = self
            .image
            .context("appearance.startup_splash.image is required when enabled = true")?;
        if self.duration_ms == 0 {
            bail!("appearance.startup_splash.duration_ms must be greater than zero");
        }
        if !self.opacity.is_finite() || !(0.0..=1.0).contains(&self.opacity) {
            bail!("appearance.startup_splash.opacity must be between 0.0 and 1.0");
        }
        Ok(Some(StartupSplash {
            path: resolve_path(base_directory, path),
            duration: Duration::from_millis(self.duration_ms),
            opacity: self.opacity,
        }))
    }
}

fn default_splash_duration_ms() -> u64 {
    1_500
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInput {
    #[serde(default)]
    shortcuts: RawShortcuts,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawShortcuts {
    #[serde(default = "default_events_shortcut")]
    diagnostics_events: String,
    #[serde(default = "default_ticks_shortcut")]
    diagnostics_ticks: String,
    #[serde(default = "default_frames_shortcut")]
    diagnostics_frames: String,
}

impl Default for RawShortcuts {
    fn default() -> Self {
        Self {
            diagnostics_events: default_events_shortcut(),
            diagnostics_ticks: default_ticks_shortcut(),
            diagnostics_frames: default_frames_shortcut(),
        }
    }
}

impl RawShortcuts {
    fn into_shortcuts(self) -> Result<DiagnosticsShortcuts> {
        let shortcuts = DiagnosticsShortcuts {
            events: parse_shortcut(
                "input.shortcuts.diagnostics_events",
                &self.diagnostics_events,
            )?,
            ticks: parse_shortcut("input.shortcuts.diagnostics_ticks", &self.diagnostics_ticks)?,
            frames: parse_shortcut(
                "input.shortcuts.diagnostics_frames",
                &self.diagnostics_frames,
            )?,
        };
        if shortcuts.events == shortcuts.ticks
            || shortcuts.events == shortcuts.frames
            || shortcuts.ticks == shortcuts.frames
        {
            bail!("input.shortcuts diagnostics shortcuts must be distinct");
        }
        Ok(shortcuts)
    }
}

fn default_events_shortcut() -> String {
    "Ctrl+Shift+E".to_owned()
}

fn default_ticks_shortcut() -> String {
    "Ctrl+Shift+T".to_owned()
}

fn default_frames_shortcut() -> String {
    "Ctrl+Shift+F".to_owned()
}

fn parse_shortcut(name: &str, value: &str) -> Result<Shortcut> {
    Shortcut::parse(value).map_err(|error| anyhow::anyhow!("{name}: {error}"))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTerminal {
    #[serde(default)]
    font: RawFont,
    #[serde(default)]
    foreground: RawForeground,
    #[serde(default)]
    background: RawBackground,
}

impl RawTerminal {
    fn into_appearance(self, base_directory: &Path) -> Result<Appearance> {
        Ok(Appearance {
            font: self.font.into_appearance(base_directory)?,
            foreground: self.foreground.into_appearance()?,
            background: self.background.into_appearance(base_directory)?,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFont {
    #[serde(default)]
    source: RawFontSource,
    family: Option<String>,
    path: Option<PathBuf>,
    #[serde(default)]
    face_index: u32,
    #[serde(default = "default_font_size")]
    size: f32,
    #[serde(default = "default_line_height")]
    line_height: f32,
    #[serde(default)]
    cell_width: RawCellWidth,
}

impl Default for RawFont {
    fn default() -> Self {
        Self {
            source: RawFontSource::default(),
            family: None,
            path: None,
            face_index: 0,
            size: default_font_size(),
            line_height: default_line_height(),
            cell_width: RawCellWidth::default(),
        }
    }
}

impl RawFont {
    fn into_appearance(self, base_directory: &Path) -> Result<FontAppearance> {
        let source = match self.source {
            RawFontSource::System => FontSource::System(
                self.family
                    .filter(|family| !family.trim().is_empty())
                    .unwrap_or_else(|| "monospace".to_owned()),
            ),
            RawFontSource::File => {
                let path = self
                    .path
                    .context("terminal.font.path is required for source = \"file\"")?;
                FontSource::File {
                    path: resolve_path(base_directory, path),
                    face_index: self.face_index,
                }
            }
        };
        validate_positive("terminal.font.size", self.size)?;
        validate_positive("terminal.font.line_height", self.line_height)?;

        let cell_width = match self.cell_width.into_value()? {
            RawCellWidth::Auto(_) => CellWidth::Auto,
            RawCellWidth::Fixed(width) => {
                validate_positive("terminal.font.cell_width", width)?;
                CellWidth::Fixed(width)
            }
        };

        Ok(FontAppearance {
            source,
            size: self.size,
            line_height: self.line_height,
            cell_width,
        })
    }
}

fn default_font_size() -> f32 {
    16.0
}

fn default_line_height() -> f32 {
    20.0
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawFontSource {
    #[default]
    System,
    File,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawCellWidth {
    Auto(String),
    Fixed(f32),
}

impl Default for RawCellWidth {
    fn default() -> Self {
        Self::Auto("auto".to_owned())
    }
}

impl RawCellWidth {
    fn into_value(self) -> Result<Self> {
        match self {
            Self::Auto(value) if value == "auto" => Ok(Self::Auto(value)),
            Self::Auto(_) => {
                bail!("terminal.font.cell_width must be \"auto\" or a positive number")
            }
            value @ Self::Fixed(_) => Ok(value),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBackground {
    #[serde(default)]
    kind: RawBackgroundKind,
    color: Option<String>,
    path: Option<PathBuf>,
    #[serde(default = "default_opacity")]
    opacity: f32,
    #[serde(default)]
    fit: ImageFit,
}

impl Default for RawBackground {
    fn default() -> Self {
        Self {
            kind: RawBackgroundKind::default(),
            color: None,
            path: None,
            opacity: default_opacity(),
            fit: ImageFit::default(),
        }
    }
}

impl RawBackground {
    fn into_appearance(self, base_directory: &Path) -> Result<Background> {
        if !self.opacity.is_finite() || !(0.0..=1.0).contains(&self.opacity) {
            bail!("terminal.background.opacity must be between 0.0 and 1.0");
        }

        match self.kind {
            RawBackgroundKind::Color => {
                let color = self.color.unwrap_or_else(|| "#090B0F".to_owned());
                Ok(Background::Color {
                    color: parse_hex_color("terminal.background.color", &color)?,
                    opacity: self.opacity,
                })
            }
            RawBackgroundKind::Image => {
                let path = self
                    .path
                    .context("terminal.background.path is required for kind = \"image\"")?;
                Ok(Background::Image {
                    path: resolve_path(base_directory, path),
                    opacity: self.opacity,
                    fit: self.fit,
                })
            }
        }
    }
}

fn default_opacity() -> f32 {
    1.0
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawBackgroundKind {
    #[default]
    Color,
    Image,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawForeground {
    color: Option<String>,
}

impl RawForeground {
    fn into_appearance(self) -> Result<Foreground> {
        let color = self.color.unwrap_or_else(|| "#E5E7EB".to_owned());
        Ok(Foreground {
            color: parse_hex_color("terminal.foreground.color", &color)?,
        })
    }
}

fn resolve_path(base_directory: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base_directory.join(path)
    }
}

fn parse_hex_color(name: &str, value: &str) -> Result<Rgb> {
    let value = value
        .strip_prefix('#')
        .with_context(|| format!("{name} must start with '#'"))?;
    if value.len() != 6 {
        bail!("{name} must use the #RRGGBB format");
    }

    let component = |range| {
        u8::from_str_radix(&value[range], 16)
            .with_context(|| format!("{name} contains an invalid hexadecimal component"))
    };
    Ok(Rgb::new(
        component(0..2)?,
        component(2..4)?,
        component(4..6)?,
    ))
}

fn validate_positive(name: &str, value: f32) -> Result<()> {
    if !value.is_finite() || value <= 0.0 {
        bail!("{name} must be a positive finite number");
    }
    Ok(())
}

fn validate_finite(name: &str, value: f64) -> Result<()> {
    if !value.is_finite() {
        bail!("{name} must be finite");
    }
    Ok(())
}

fn validate_positive_f64(name: &str, value: f64) -> Result<()> {
    if !value.is_finite() || value <= 0.0 {
        bail!("{name} must be a positive finite number");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{path::Path, time::Duration};

    use super::{
        Background, CellWidth, FontSource, Foreground, ImageFit, Rgb, SceneLayer, StartupSplash,
        TestingNode, TestingScene, parse, parse_config, parse_hex_color,
    };
    use crate::diagnostics::Shortcut;

    #[test]
    fn loads_an_arbitrary_font_file_and_relative_image() {
        let appearance = parse(
            r#"
                [terminal.font]
                source = "file"
                path = "fonts/display.otf"
                face_index = 2
                size = 18.0
                line_height = 23.0
                cell_width = 13.5

                [terminal.background]
                kind = "image"
                path = "images/night.gif"
                opacity = 0.65
                fit = "contain"
            "#,
            Path::new("/tmp/moditty-config"),
        )
        .unwrap();

        assert_eq!(
            appearance.font.source,
            FontSource::File {
                path: "/tmp/moditty-config/fonts/display.otf".into(),
                face_index: 2,
            }
        );
        assert_eq!(appearance.font.cell_width, CellWidth::Fixed(13.5));
        assert_eq!(
            appearance.background,
            Background::Image {
                path: "/tmp/moditty-config/images/night.gif".into(),
                opacity: 0.65,
                fit: ImageFit::Contain,
            }
        );
    }

    #[test]
    fn parses_a_hex_color_and_its_independent_opacity() {
        let appearance = parse(
            r##"
                [terminal.background]
                color = "#0A1B2C"
                opacity = 0.4
            "##,
            Path::new("/tmp/moditty-config"),
        )
        .unwrap();

        assert_eq!(
            appearance.background,
            Background::Color {
                color: Rgb::new(10, 27, 44),
                opacity: 0.4,
            }
        );
    }

    #[test]
    fn rejects_invalid_hex_colors() {
        assert!(parse_hex_color("terminal.background.color", "#12345").is_err());
        assert!(parse_hex_color("terminal.foreground.color", "#GG0000").is_err());
    }

    #[test]
    fn loads_the_default_foreground_color_independently_from_the_background() {
        let appearance = parse(
            r##"
                [terminal.foreground]
                color = "#DDEEFF"

                [terminal.background]
                color = "#0A1B2C"
                opacity = 0.4
            "##,
            Path::new("/tmp/moditty-config"),
        )
        .unwrap();

        assert_eq!(
            appearance.foreground,
            Foreground {
                color: Rgb::new(221, 238, 255),
            }
        );
        assert_eq!(appearance.background.opacity(), 0.4);
    }

    #[test]
    fn rejects_an_invalid_cell_width_mode() {
        assert!(
            parse(
                r#"
                [terminal.font]
                cell_width = "adaptive"
            "#,
                Path::new("/tmp/moditty-config"),
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_unknown_configuration_keys() {
        assert!(
            parse(
                r##"
                [terminal.foreground]
                colour = "#DDEEFF"
            "##,
                Path::new("/tmp/moditty-config"),
            )
            .is_err()
        );
    }

    #[test]
    fn loads_diagnostics_shortcuts() {
        let config = parse_config(
            r#"
                [input.shortcuts]
                diagnostics_events = "Alt+E"
                diagnostics_ticks = "Ctrl+T"
                diagnostics_frames = "Super+F"
            "#,
            Path::new("/tmp/moditty-config"),
        )
        .unwrap();

        assert_eq!(config.diagnostics.events, Shortcut::parse("Alt+E").unwrap());
        assert_eq!(config.diagnostics.ticks, Shortcut::parse("Ctrl+T").unwrap());
        assert_eq!(
            config.diagnostics.frames,
            Shortcut::parse("Super+F").unwrap()
        );
    }

    #[test]
    fn rejects_an_unmodified_diagnostics_shortcut() {
        assert!(
            parse_config(
                r#"
                [input.shortcuts]
                diagnostics_events = "E"
            "#,
                Path::new("/tmp/moditty-config"),
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_duplicate_diagnostics_shortcuts() {
        assert!(
            parse_config(
                r#"
                [input.shortcuts]
                diagnostics_events = "Ctrl+Shift+E"
                diagnostics_ticks = "Ctrl+Shift+E"
            "#,
                Path::new("/tmp/moditty-config"),
            )
            .is_err()
        );
    }

    #[test]
    fn loads_an_enabled_relative_startup_splash() {
        let config = parse_config(
            r#"
                [appearance.startup_splash]
                enabled = true
                image = "images/splash.png"
                duration_ms = 1250
                opacity = 0.65
            "#,
            Path::new("/tmp/moditty-config"),
        )
        .unwrap();

        assert_eq!(
            config.startup_splash,
            Some(StartupSplash {
                path: "/tmp/moditty-config/images/splash.png".into(),
                duration: Duration::from_millis(1250),
                opacity: 0.65,
            })
        );
    }

    #[test]
    fn rejects_an_incomplete_or_invalid_startup_splash() {
        for contents in [
            r#"
                [appearance.startup_splash]
                enabled = true
            "#,
            r#"
                [appearance.startup_splash]
                enabled = true
                image = "splash.png"
                duration_ms = 0
            "#,
            r#"
                [appearance.startup_splash]
                enabled = true
                image = "splash.png"
                opacity = 1.1
            "#,
        ] {
            assert!(parse_config(contents, Path::new("/tmp/moditty-config")).is_err());
        }
    }

    #[test]
    fn loads_a_testing_scene_with_distinct_named_nodes() {
        let config = parse_config(
            r##"
                [testing_tools.scene]
                enabled = true

                [[testing_tools.scene.nodes]]
                id = "source"
                x = 24.0
                y = 48.0
                width = 120.0
                height = 80.0
                color = "#FF6600"
                opacity = 0.75
                z_index = 1

                [[testing_tools.scene.nodes]]
                id = "target"
                x = 200.0
                y = 48.0
                width = 120.0
                height = 80.0
                color = "#0066FF"
            "##,
            Path::new("/tmp/moditty-config"),
        )
        .unwrap();

        assert_eq!(
            config.testing_scene,
            Some(TestingScene {
                nodes: vec![
                    TestingNode {
                        id: "source".to_owned(),
                        parent: None,
                        x: 24.0,
                        y: 48.0,
                        width: 120.0,
                        height: 80.0,
                        color: Rgb::new(255, 102, 0),
                        opacity: 0.75,
                        clip_children: false,
                        z_index: 1,
                        layer: None,
                    },
                    TestingNode {
                        id: "target".to_owned(),
                        parent: None,
                        x: 200.0,
                        y: 48.0,
                        width: 120.0,
                        height: 80.0,
                        color: Rgb::new(0, 102, 255),
                        opacity: 1.0,
                        clip_children: false,
                        z_index: 0,
                        layer: None,
                    },
                ],
            })
        );
    }

    #[test]
    fn loads_testing_scene_hierarchy_and_root_layer() {
        let config = parse_config(
            r##"
                [testing_tools.scene]
                enabled = true

                [[testing_tools.scene.nodes]]
                id = "panel"
                x = 24.0
                y = 48.0
                width = 120.0
                height = 80.0
                color = "#112233"
                clip_children = true
                layer = "underlay"

                [[testing_tools.scene.nodes]]
                id = "child"
                parent = "panel"
                x = 100.0
                y = 12.0
                width = 40.0
                height = 40.0
                color = "#445566"
            "##,
            Path::new("/tmp/moditty-config"),
        )
        .unwrap();
        let scene = config.testing_scene.unwrap();

        assert_eq!(scene.nodes[0].layer, Some(SceneLayer::Underlay));
        assert!(scene.nodes[0].clip_children);
        assert_eq!(scene.nodes[1].parent.as_deref(), Some("panel"));
        assert_eq!(scene.nodes[1].layer, None);
    }

    #[test]
    fn rejects_an_invalid_enabled_testing_scene() {
        for contents in [
            r#"
                [testing_tools.scene]
                enabled = true
            "#,
            r##"
                [testing_tools.scene]
                enabled = true

                [[testing_tools.scene.nodes]]
                id = "same"
                x = 0.0
                y = 0.0
                width = 10.0
                height = 10.0
                color = "#FFFFFF"

                [[testing_tools.scene.nodes]]
                id = "same"
                x = 20.0
                y = 0.0
                width = 10.0
                height = 10.0
                color = "#FFFFFF"
            "##,
            r##"
                [testing_tools.scene]
                enabled = true

                [[testing_tools.scene.nodes]]
                id = "broken"
                x = 0.0
                y = 0.0
                width = 0.0
                height = 10.0
                color = "#FFFFFF"
            "##,
            r##"
                [testing_tools.scene]
                enabled = true

                [[testing_tools.scene.nodes]]
                id = "child"
                parent = "missing"
                x = 0.0
                y = 0.0
                width = 10.0
                height = 10.0
                color = "#FFFFFF"
            "##,
            r##"
                [testing_tools.scene]
                enabled = true

                [[testing_tools.scene.nodes]]
                id = "first"
                parent = "second"
                x = 0.0
                y = 0.0
                width = 10.0
                height = 10.0
                color = "#FFFFFF"

                [[testing_tools.scene.nodes]]
                id = "second"
                parent = "first"
                x = 20.0
                y = 0.0
                width = 10.0
                height = 10.0
                color = "#FFFFFF"
            "##,
            r##"
                [testing_tools.scene]
                enabled = true

                [[testing_tools.scene.nodes]]
                id = "root"
                x = 0.0
                y = 0.0
                width = 10.0
                height = 10.0
                color = "#FFFFFF"

                [[testing_tools.scene.nodes]]
                id = "child"
                parent = "root"
                layer = "underlay"
                x = 0.0
                y = 0.0
                width = 10.0
                height = 10.0
                color = "#FFFFFF"
            "##,
        ] {
            assert!(parse_config(contents, Path::new("/tmp/moditty-config")).is_err());
        }
    }
}
