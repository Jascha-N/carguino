extern crate cargo;
extern crate carguino_build;
extern crate docopt;
#[macro_use] extern crate error_chain;
#[macro_use] extern crate lazy_static;
extern crate regex;
extern crate rustc_serialize;
#[macro_use] extern crate serde_derive;
extern crate serde_json;
extern crate tempdir;
extern crate term;
extern crate toml;

use board::BoardInfo;
use config::Config;
use error::{Result, ResultExt};

use cargo::CargoResult;
use cargo::core::{MultiShell, Verbosity};
use cargo::util;

use carguino_build::config as build_config;

use docopt::Docopt;

use regex::Regex;

use serde_json::Value;

use tempdir::TempDir;

use term::color;

use std::collections::HashMap;
use std::env;
use std::fmt::Display;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Cursor, Write};
use std::iter::FromIterator;
use std::path::{Path, PathBuf};
use std::process;

mod board;
mod builder;
mod config;
mod error;

const VERSION_STRING: &'static str = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"));

trait MultiShellExt {
    fn status_ext<T: Display, U: Display>(&mut self, status: T, message: U) -> CargoResult<()>;
}

impl MultiShellExt for MultiShell {
    fn status_ext<T: Display, U: Display>(&mut self, status: T, message: U) -> CargoResult<()> {
        if self.get_verbose() != Verbosity::Quiet {
            self.err().say_status(status, message, color::CYAN, true)?;
        }
        Ok(())
    }
}

const USAGE: &'static str = "
Cargo wrapper for Arduino projects.

Usage:
    carguino <command> [options] [<args>...]
    carguino -h | --help
    carguino -V | --version

Options:
    --target-board BOARD   Fully-qualified Arduino board name to compile for
    --serial-port PORT     Serial port to upload to
    -h, --help             Show this message
    -V, --version          Print version info and exit

The supported cargo subcommands are: `build`, `check`, `clean`, `doc`, `rustc`,
`rustdoc` and `clippy` (if installed). Any other commands are passed as-is to
cargo.
";

#[derive(Debug, RustcDecodable)]
struct Args {
    arg_command: String,
    arg_args: Vec<String>,
    flag_target_board: String,
    flag_serial_port: String
}

fn main() {
    let mut config = Config::default();

    if let Err(error) = run(&mut config) {
        config.shell().error(error).unwrap();
        process::exit(1);
    }
}

fn run(config: &mut Config) -> Result<()> {
    let docopt = Docopt::new(USAGE).unwrap()
                        .options_first(true)
                        .help(true)
                        .version(Some(VERSION_STRING.to_string()));

    let Args { arg_command, arg_args, .. } = match docopt.decode::<Args>() {
        Err(ref error) if !error.fatal() => {
            writeln!(config.shell().err(), "{}", error).unwrap();
            return Ok(());
        }
        args => args
    }?;

    let cargo_args = config.parse_options(arg_args)?;
    let current_dir = env::current_dir().chain_err(|| "Unable to access current directory")?;
    config.parse_files(&current_dir)?;

    cargo_run(&arg_command, &cargo_args, config)
}

fn cargo_run(command: &str, args: &[String], config: &mut Config) -> Result<()> {
    let builder = if let Some(builder) = config.create_builder() {
        builder
    } else {
        config.shell().warn("No target-board was specified; running cargo normally.")?;
        let mut cargo = util::process("cargo");
        config.add_message_format_option(&mut cargo);
        cargo.arg(command).args(args).exec()?;
        return Ok(());
    };

    config.shell().verbose(|shell| {
        shell.status_ext("Retrieving", format_args!("build settings"))
    })?;

    let prefs = {
        let temp_dir = TempDir::new("carguino").chain_err(|| "Could not create temporary directory")?;
        let temp_file = temp_dir.path().join("project.c");
        File::create(&temp_file).chain_err(|| "Could not create temporary project file")?;

        builder.dump_prefs(&temp_file)?
    };

    let board_name = prefs.get::<String>("name")
                               .map_or_else(|| Err("'name' missing from preferences"), Ok)?;

    config.shell().status_ext("Configuring", board_name)?;

    let target_mcu = prefs.get::<String>("build.mcu")
                               .map_or_else(|| Err("'build.mcu' missing from preferences"), Ok)?;
    let target_arch = prefs.get::<String>("build.arch")
                                .map(|s| s.to_lowercase())
                                .map_or_else(|| Err("'build.arch' missing from preferences"), Ok)?;

    let linker_recipe = prefs.get::<String>("recipe.c.combine.pattern")
                                  .map_or_else(|| Err("'recipe.c.combine.pattern' missing from preferences"), Ok)?;

    let platform_dir = prefs.get::<String>("runtime.platform.path")
                                 .map(PathBuf::from)
                                 .map_or_else(|| Err("'runtime.platform.path' missing from preferences"), Ok)?;

    let objcopy_regex = Regex::new(r#"^recipe\.objcopy\.(\w+)\.pattern"#).unwrap();
    let objcopy_recipes = prefs.keys().filter_map(|key| {
        objcopy_regex.captures(key).map(|captures| {
            let (command, mut args) = build_config::split_command_line(&prefs.get::<String>(key).unwrap());
            let len = args.len();
            args.truncate(len - 2);
            (captures[1].to_string(), command, args)
        })
    }).collect::<Vec<_>>();

    let mut library_paths = HashMap::new();
    detect_libraries(&platform_dir.join("libraries"), &mut library_paths, config.shell())?;

    let linker_options = parse_linker_options(&linker_recipe);

    let base_flags = &[
        format!(r#"--cfg arduino_arch="{}""#, target_arch),
        format!(r#"--cfg arduino_mcu="{}""#, target_mcu)
    ];

    let mut rustdocflags = Vec::from_iter(env::var("RUSTDOCFLAGS"));
    rustdocflags.extend_from_slice(base_flags);

    let mut rustflags = Vec::from_iter(env::var("RUSTFLAGS"));
    rustflags.extend_from_slice(base_flags);

    let mut cargo_metadata = util::process("cargo");
    cargo_metadata.arg("metadata").arg("--no-deps");

    config.shell().verbose(|shell| {
        shell.status_ext("Running", &cargo_metadata)
    })?;

    let output = cargo_metadata.exec_with_output()?;
    let metadata = serde_json::from_slice::<Value>(&output.stdout).unwrap();
    let package_id = metadata["packages"][0]["id"].as_str().unwrap().to_string();
    let targets_dir = env::home_dir().unwrap().join(".carguino/targets");
    fs::create_dir_all(&targets_dir).chain_err(|| "Could not create targets directory")?;
    let (llvm_target, target) = create_target_spec(config, &linker_options, &targets_dir, &target_arch, &target_mcu)?;

    let mut xargo_base = util::process("xargo");
    xargo_base.env("CARGUINO_CONFIG", build_config::Config::serialize(prefs, llvm_target, &target_arch, library_paths)?)
              .env("RUSTFLAGS", rustflags.join(" "))
              .env("RUSTDOCFLAGS", rustdocflags.join(" "))
              .env("RUST_TARGET_PATH", targets_dir)
              .arg(command)
              .arg("--target").arg(target);

    let mut xargo_pass1 = xargo_base.clone();
    config.add_message_format_option(&mut xargo_pass1);
    xargo_pass1.args(args);
    config.shell().verbose(|shell| {
        shell.status_ext("Running", &xargo_pass1)
    })?;
    xargo_pass1.exec()?;

    let mut xargo_pass2 = xargo_base;
    xargo_pass2.arg("--message-format").arg("json")
               .args(args);

    let output = xargo_pass2.exec_with_output()?;

    let stdout = BufReader::new(Cursor::new(output.stdout));
    let artifacts = stdout.lines().filter_map(|line| {
        line.ok().and_then(|line| {
            serde_json::from_str::<Value>(&line).ok()
        })
    }).filter(|message| {
        message["reason"].as_str() == Some("compiler-artifact")
        && message["package_id"].as_str() == Some(package_id.as_str())
        && message["target"]["kind"].as_array().unwrap().iter().any(|kind| kind.as_str() == Some("bin"))
    }).flat_map(|message| {
        message["filenames"].as_array().unwrap().clone()
    }).map(|artifact| {
        PathBuf::from(artifact.as_str().unwrap())
    }).collect::<Vec<_>>();

    if !artifacts.is_empty() {
        for &(ref extension, ref command, ref options) in &objcopy_recipes {
            config.shell().status_ext("Extracting", format_args!("{} data for {}", extension, package_id))?;

            for artifact in &artifacts {
                let mut objcopy = util::process(command);
                objcopy.args(options)
                       .arg(artifact)
                       .arg(artifact.with_extension(extension));

                config.shell().verbose(|shell| {
                    shell.status_ext("Running", &objcopy)
                })?;

                objcopy.exec()?;
            }
        }
    }

    Ok(())
}

fn detect_libraries(dir: &Path, library_dirs: &mut HashMap<String, PathBuf>, shell: &mut MultiShell) -> Result<()> {
    match fs::read_dir(dir) {
        Ok(iter) => {
            for entry in iter {
                let path = entry.chain_err(|| "Could not read library path entry")?.path();
                if path.is_dir() {
                    let library_name = path.file_name().unwrap().to_string_lossy().to_string();
                    if library_dirs.insert(library_name.clone(), path).is_some() {
                        shell.warn(format_args!("Library directory for '{}' overridden", library_name)).unwrap();
                    }
                }
            }
        }
        Err(error) => {
            shell.warn(format_args!("Skipping library directory '{}': {}", dir.display(), error)).unwrap();
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
struct LinkerOptions {
    command: String,
    script: Option<String>,
    specs: Vec<String>,
    library_search_path: Vec<String>,
    libraries: Vec<String>,
    platform_options: Vec<String>
}

fn parse_linker_options(command_line: &str) -> LinkerOptions {
    let (command, args) = build_config::split_command_line(command_line);
    let mut result = LinkerOptions {
        command: command.to_str().unwrap().to_string(),
        .. Default::default()
    };
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--specs" | "-specs" => {
                result.specs.push(iter.next().unwrap())
            }
            arg if arg.starts_with("--specs=") || arg.starts_with("-specs=") => {
                let splits = arg.splitn(2, '=').collect::<Vec<_>>();
                result.specs.push(splits[1].to_string());
            }

            "-T" => {
                result.script = Some(iter.next().unwrap());
            }
            arg if arg.starts_with("-T") => {
                result.script = Some(arg[2..].to_string());
            }

            "-L" => {
                let path = iter.next().unwrap();
                if Path::new(&path).is_dir() {
                    result.library_search_path.push(path);
                }
            }
            arg if arg.starts_with("-L") => {
                let path = &arg[2..];
                if Path::new(&path).is_dir() {
                    result.library_search_path.push(path.to_string());
                }
            }

            "-l" => {
                result.libraries.push(iter.next().unwrap().clone());
            }
            arg if arg.starts_with("-l") => {
                result.libraries.push(arg[2..].to_string());
            }

            arg if arg.starts_with("-m") => {
                result.platform_options.push(arg.to_string());
            }

            _ => {}
        }
    }
    result
}

fn create_target_spec(config: &mut Config, linker_options: &LinkerOptions, targets_dir: &Path,
                      arch: &str, cpu: &str, ) -> Result<(&'static str, String)> {
    let target = match arch {
        "avr" => "avr-atmel-none",
        "samd" => "thumbv6m-none-eabi",
        "sam" => match cpu {
            "cortex-m0" | "cortex-m0plus" | "cortex-m1" => "thumbv6m-none-eabi",
            "cortex-m3" => "thumbv7m-none-eabi",
            "cortex-m4" | "cortex-m7" => "thumbv7em-none-eabi",
            cpu => bail!("Unsupported SAM CPU: {}", cpu)
        },
        arch => {
            bail!("Unsupported architecture: {}", arch);
        }
    };

    let spec_name = {
        let board = config.target_board().unwrap();
        let arch = board.arch().to_lowercase().replace('-', "_");
        let vendor = board.vendor().to_lowercase().replace('-', "_");
        let name = board.board().to_lowercase().replace('-', "_");

        format!("{}-{}-{}", arch, vendor, name)
    };
    let spec_path = targets_dir.join(&spec_name).with_extension("json");

    if !spec_path.is_file() {
        let mut rustc = util::process("rustc");
        rustc.arg("-Z").arg("unstable-options")
            .arg("--target").arg(target)
            .arg("--print").arg("target-spec-json");

        config.shell().verbose(|shell| {
            shell.status_ext("Running", &rustc)
        })?;

        let output = rustc.exec_with_output()?;
        let mut spec = serde_json::from_slice::<Value>(&output.stdout).unwrap();
        spec["is-builtin"] = Value::Bool(false);
        spec["linker"] = Value::String(linker_options.command.clone());
        spec["linker-is-gnu"] = Value::Bool(true);
        spec["no-default-libraries"] = Value::Bool(false);
        spec["cpu"] = Value::String(cpu.to_string());

        let mut pre_link_args = spec["pre-link-args"].as_array().cloned().unwrap_or_default();
        pre_link_args.extend(linker_options.specs.iter().map(|specs| {
            Value::String(format!("-specs={}", specs))
        }));
        pre_link_args.extend(linker_options.platform_options.iter().map(|option| {
            Value::String(option.clone())
        }));
        if let Some(ref script) = linker_options.script {
            pre_link_args.push(Value::String(format!("-T{}", script)));
        }
        pre_link_args.extend(linker_options.library_search_path.iter().map(|lib_path| {
            Value::String(format!("-L{}", lib_path))
        }));
        spec["pre-link-args"] = Value::Array(pre_link_args);

        let mut late_link_args = spec["late-link-args"].as_array().cloned().unwrap_or_default();
        late_link_args.extend(linker_options.libraries.iter().map(|lib| {
            Value::String(format!("-l{}", lib))
        }));
        spec["late-link-args"] = Value::Array(late_link_args);

        let mut spec_file = File::create(&spec_path).chain_err(|| "Could not create target spec file")?;
        serde_json::to_writer_pretty(&mut spec_file, &spec).chain_err(|| "Could not serialize to target spec file")?;
    }

    Ok((target, spec_name))
}
