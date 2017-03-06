use board::BoardInfo;
use builder::Builder;
use error::{Result, ResultExt};

use cargo;
use cargo::core::{ColorConfig, MultiShell, Verbosity};
use cargo::ops::MessageFormat;
use cargo::util::ProcessBuilder;

use toml;

use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub struct Config {
    node: Box<ConfigNode>,
    message_format: MessageFormat,
    shell: MultiShell,
    target_board: Option<BoardInfo>
}

impl Config {
    pub fn parse_files(&mut self, current_dir: &Path) -> Result<()> {
        self.node = ConfigNode::load(Some(current_dir))?;
        Ok(())
    }

    pub fn parse_options(&mut self, args: Vec<String>) -> Result<Vec<String>> {
        let mut cargo_args = Vec::<String>::new();
        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                _ if arg.starts_with("--target=") => {
                    self.shell.warn("Do not specify a target triple directly, instead use '--target-board'; option ignored")?;
                }
                "--target" => {
                    iter.next();
                    self.shell.warn("Do not specify a target triple directly, instead use '--target-board'; option ignored")?;
                }

                option if arg.starts_with("--target-board=") => {
                    let board = &option["--target-board=".len()..];
                    if board.is_empty() {
                        bail!("target-board is empty");
                    }
                    self.target_board = Some(BoardInfo::from_fqbn(board)?);
                }
                "--target-board" => {
                    if let Some(board) = iter.next() {
                        self.target_board = Some(BoardInfo::from_fqbn(&board)?);
                    } else {
                        bail!("Expected argument for option '--target-board'")
                    }
                }

                option if arg.starts_with("--message-format=") => {
                    let message_format = &option["--message-format=".len()..];
                    if message_format.to_lowercase() == "json" {
                        self.message_format = MessageFormat::Json;
                    }
                }
                "--message-format" => {
                    if let Some(message_format) = iter.next() {
                        if message_format.to_lowercase() == "json" {
                            self.message_format = MessageFormat::Json;
                        }
                    }
                }

                option if arg.starts_with("--color=") => {
                    let color = &option["--color=".len()..];
                    self.shell.set_color_config(Some(color))?;
                    cargo_args.push(arg.clone());
                }
                "--color" => {
                    cargo_args.push(arg.clone());
                    if let Some(color) = iter.next() {
                        self.shell.set_color_config(Some(&color))?;
                        cargo_args.push(color);
                    }
                }

                "--verbose" | "-v" | "-vv" => {
                    self.shell.set_verbosity(Verbosity::Verbose);
                    cargo_args.push(arg.clone());
                }
                "--quiet" | "-q" => {
                    self.shell.set_verbosity(Verbosity::Quiet);
                    cargo_args.push(arg.clone());
                }

                _ => {
                    cargo_args.push(arg.clone())
                }
            }
        }
        Ok(cargo_args)
    }

    pub fn add_message_format_option<'a>(&self, builder: &'a mut ProcessBuilder) -> &'a mut ProcessBuilder {
        builder.arg("--message-format");
        match self.message_format {
            MessageFormat::Json => builder.arg("json"),
            MessageFormat::Human => builder.arg("human")
        }
    }

    pub fn shell(&mut self) -> &mut MultiShell {
        &mut self.shell
    }

    pub fn target_board(&self) -> Option<&BoardInfo> {
        self.target_board.as_ref().or_else(|| self.node.target_board())
    }

    pub fn create_builder(&self) -> Option<Builder> {
        self.target_board().map(|board| {
            let mut builder = Builder::new(board);

            let home_var = env::var_os("ARDUINO_HOME").map(PathBuf::from);
            if let Some(home) = home_var.as_ref().map(PathBuf::as_path).or_else(|| self.node.home()) {
                builder.home(home);
            }

            for hardware in self.node.hardware() {
                builder.hardware(hardware);
            }

            for tools in self.node.tools() {
                builder.tools(tools);
            }

            for libraries in self.node.libraries() {
                builder.libraries(libraries);
            }

            for (key, value) in self.node.preferences() {
                builder.pref(key, value);
            }

            builder
        })
    }
}

impl Default for Config {
    fn default() -> Config {
        Config {
            node: Default::default(),
            shell: cargo::shell(Verbosity::Normal, ColorConfig::Auto),
            message_format: MessageFormat::Human,
            target_board: None
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ConfigNode {
    parent: Option<Box<ConfigNode>>,
    config: ConfigFile
}

impl ConfigNode {
    fn load(dir: Option<&Path>) -> Result<Box<ConfigNode>> {
        let (path, parent) = if let Some(dir) = dir {
            (Some(PathBuf::from(dir)), ConfigNode::load(dir.parent())?)
        } else {
            (env::home_dir(), Box::new(ConfigNode::default()))
        };

        path.map(|path| path.join(".carguino/config")).and_then(|path| {
            if path.is_file() { Some(path) } else { None }
        }).map(|path| {
            File::open(&path).and_then(|mut file| {
                let mut config = String::new();
                file.read_to_string(&mut config).map(|_| config)
            }).chain_err(|| {
                format!("Could not read configuration file '{}'", path.display())
            }).and_then(|config| {
                toml::from_str(&config).map(|config| {
                    ConfigNode {
                        parent: Some(parent.clone()),
                        config: config
                    }
                }).map(Box::new).chain_err(|| {
                    format!("Could not parse configuration file '{}'", path.display())
                })
            })
        }).unwrap_or_else(|| Ok(parent))
    }

    fn target_board(&self) -> Option<&BoardInfo> {
        self.config.target_board.as_ref().or_else(|| {
            self.parent.as_ref().and_then(|parent| parent.target_board())
        })
    }

    fn home(&self) -> Option<&Path> {
        self.config.arduino_builder.home.as_ref().map(PathBuf::as_path).or_else(|| {
            self.parent.as_ref().and_then(|parent| parent.home())
        })
    }

    fn hardware(&self) -> Vec<&Path> {
        self.parent.iter().flat_map(|parent| parent.hardware()).chain(
            self.config.arduino_builder.hardware.iter().map(PathBuf::as_path)
        ).collect()
    }

    fn tools(&self) -> Vec<&Path> {
        self.parent.iter().flat_map(|parent| parent.tools()).chain(
            self.config.arduino_builder.tools.iter().map(PathBuf::as_path)
        ).collect()
    }

    fn libraries(&self) -> Vec<&Path> {
        self.parent.iter().flat_map(|parent| parent.libraries()).chain(
            self.config.arduino_builder.libraries.iter().map(PathBuf::as_path)
        ).collect()
    }

    fn preferences(&self) -> Vec<(&str, &str)> {
        self.parent.iter().flat_map(|parent| parent.preferences()).chain(
            self.config.arduino_builder.preferences.iter().map(|(key, value)| (key.as_str(), value.as_str()))
        ).collect()
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(rename = "target-board")]
    target_board: Option<BoardInfo>,
    #[serde(default, rename = "arduino-builder")]
    arduino_builder: ArduinoBuilder
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArduinoBuilder {
    home: Option<PathBuf>,
    hardware: Vec<PathBuf>,
    tools: Vec<PathBuf>,
    libraries: Vec<PathBuf>,
    #[serde(default)]
    preferences: HashMap<String, String>
}
