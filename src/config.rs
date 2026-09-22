use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

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
    pub path: Option<PathBuf>,
}

pub fn load() -> Result<LoadedConfig> {
    let Some(path) = config_path() else {
        return Ok(LoadedConfig {
            appearance: Appearance::default(),
            path: None,
        });
    };

    if !path.exists() {
        return Ok(LoadedConfig {
            appearance: Appearance::default(),
            path: None,
        });
    }

    let contents = fs::read_to_string(&path)
        .with_context(|| format!("could not read configuration file {}", path.display()))?;
    let base_directory = path.parent().unwrap_or_else(|| Path::new("."));
    let appearance = parse(&contents, base_directory)
        .with_context(|| format!("invalid configuration in {}", path.display()))?;

    Ok(LoadedConfig {
        appearance,
        path: Some(path),
    })
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("MODITTY_CONFIG") {
        return Some(PathBuf::from(path));
    }

    #[cfg(target_os = "macos")]
    {
        return home_directory().map(|home| {
            home.join("Library")
                .join("Application Support")
                .join("moditty")
                .join("moditty.toml")
        });
    }

    #[cfg(not(target_os = "macos"))]
    {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home_directory().map(|home| home.join(".config")))
            .map(|directory| directory.join("moditty").join("moditty.toml"))
    }
}

fn home_directory() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn parse(contents: &str, base_directory: &Path) -> Result<Appearance> {
    let raw: RawConfig = toml::from_str(contents).context("could not parse TOML")?;
    raw.terminal.into_appearance(base_directory)
}

#[derive(Default, Deserialize)]
struct RawConfig {
    #[serde(default)]
    terminal: RawTerminal,
}

#[derive(Default, Deserialize)]
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        Background, CellWidth, FontSource, Foreground, ImageFit, Rgb, parse, parse_hex_color,
    };

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
}
