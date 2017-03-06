use {Result, ResultExt};
use prefs::Preferences;

use bindgen::{self, Builder as BindgenBuilder};

use regex::{Captures, Regex};

use serde_json;

use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, BufReader, Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    core: String,
    arch: String,
    board: String,
    llvm_target: String,

    core_path: PathBuf,
    variant_path: PathBuf,

    library_paths: HashMap<String, PathBuf>,

    c_system_includes: Vec<PathBuf>,
    cpp_system_includes: Vec<PathBuf>,

    c_compiler: Recipe,
    cpp_compiler: Recipe,
    assembler: Recipe,
    archiver: Recipe
}

impl Config {
    #[doc(hidden)]
    pub fn serialize(mut prefs: Preferences, llvm_target: &str, arch: &str, library_paths: HashMap<String, PathBuf>) -> Result<String> {
        prefs.set("source_file", "%source_file");
        prefs.set("object_file", "%object_file");
        prefs.set("includes", "%includes");
        prefs.set("archive_file", "%archive_file");
        prefs.set("archive_file_path", "%archive_file");

        let core = prefs.get::<String>("build.core")
                        .map_or_else(|| Err("'build.core' missing from preferences"), Ok)?;

        let board = prefs.get::<String>("build.board")
                         .map(|s| s.to_lowercase())
                         .map_or_else(|| Err("'build.board' missing from preferences"), Ok)?;

        let core_path = prefs.get::<String>("build.core.path")
                             .map(PathBuf::from)
                             .map_or_else(|| Err("'build.core.path' missing from preferences"), Ok)?;
        let variant_path = prefs.get::<String>("build.variant.path")
                                .map(PathBuf::from)
                                .map_or_else(|| Err("'build.variant.path' missing from preferences"), Ok)?;

        let c_compiler = Recipe::from_prefs(&prefs, "c.o");
        let cpp_compiler = Recipe::from_prefs(&prefs, "cpp.o");
        let assembler = Recipe::from_prefs(&prefs, "S.o");
        let archiver = Recipe::from_prefs(&prefs, "ar");

        let c_system_includes = get_system_includes(c_compiler.command().as_os_str(), &["-w", "-v", "-E", "-xc", "-"]);
        let cpp_system_includes = get_system_includes(cpp_compiler.command().as_os_str(), &["-w", "-v", "-E", "-xc++", "-"]);

        let config = Config {
            core: core,
            arch: arch.to_string(),
            board: board,
            llvm_target: llvm_target.to_string(),
            core_path: core_path,
            variant_path: variant_path,
            library_paths: library_paths,
            c_system_includes: c_system_includes,
            cpp_system_includes: cpp_system_includes,
            c_compiler: c_compiler,
            cpp_compiler: cpp_compiler,
            assembler: assembler,
            archiver: archiver
        };

        serde_json::to_string(&config).chain_err(|| "Unable to serialize configuration")
    }

    pub fn new() -> Result<Config> {
        env::var("CARGUINO_CONFIG").chain_err(|| {
            "Could not read $CARGUINO_CONFIG variable (is carguino running?)"
        }).and_then(|config_var| {
            serde_json::from_str(&config_var).chain_err(|| "Unable to deserialize configuration")
        })
    }

    pub fn core(&self) -> &str {
        &self.core
    }

    pub fn arch(&self) -> &str {
        &self.arch
    }

    fn base_includes(&self) -> Vec<PathBuf> {
        vec![self.core_path.clone(), self.variant_path.clone()]
    }

    fn compile(&self, source_file: &Path, object_file: &Path, include_dirs: &[PathBuf]) -> Result<()> {
        let recipe = match source_file {
            path if is_c_source(path) => &self.c_compiler,
            path if is_cpp_source(path) => &self.cpp_compiler,
            path if is_asm_source(path) => &self.assembler,
            _ => unreachable!()
        };
        fs::create_dir_all(object_file.parent().unwrap()).chain_err(|| "Unable to create directory")?;

        let includes = self.base_includes().iter().chain(include_dirs).fold(String::new(), |acc, include| {
            format!(r#"{} "-I{}""#, acc, include.display())
        });

        recipe.run(RecipeParams {
            source_file: source_file.to_string_lossy().to_string(),
            object_file: object_file.to_string_lossy().to_string(),
            includes: includes,
            .. RecipeParams::default()
        }).map(|_| ())
    }

    fn archive(&self, object_file: &Path, archive_file: &Path) -> Result<()> {
        fs::create_dir_all(archive_file.parent().unwrap()).chain_err(|| "Unable to create directory")?;

        self.archiver.run(RecipeParams {
            object_file: object_file.to_string_lossy().to_string(),
            archive_file: archive_file.to_string_lossy().to_string(),
            .. RecipeParams::default()
        }).map(|_| ())
    }

    fn generate_bindings(&self, builder: BindgenBuilder, header_file: &Path, include_dirs: &[PathBuf], target_dir: &Path) -> Result<()> {
        let builder = builder.header(header_file.to_string_lossy())
                             .use_core()
                             .clang_arg("-target").clang_arg(self.llvm_target.as_str());

        let (compiler, system_includes) = match header_file {
            path if is_c_header(path) => (&self.c_compiler, &self.c_system_includes),
            path if is_cpp_header(path) => (&self.cpp_compiler, &self.cpp_system_includes),
            _ => bail!("Unknown header extension")
        };

        let builder = system_includes.iter().fold(builder, |builder, include| {
            builder.clang_arg("-isystem").clang_arg(include.to_string_lossy())
        });

        let include_dirs = self.base_includes().iter().chain(include_dirs).fold(String::new(), |acc, include| {
            format!(r#"{} "-I{}""#, acc, include.display())
        });

        let (_, args) = compiler.substitute(RecipeParams {
            includes: include_dirs,
            .. RecipeParams::default()
        });

        let builder = args.iter().fold(builder, |builder, arg| match arg.as_str() {
            arg if arg.starts_with("-std=") ||
                   arg.starts_with("-m") ||
                   arg.starts_with("-I") ||
                   arg.starts_with("-D") => builder.clang_arg(arg),
            _ => builder
        });

        let bindings = builder.generate().map_err(|_| "Unable to generate bindings")?;
        let bindings_file = target_dir.join(header_file.with_extension("rs").file_name().unwrap());
        bindings.write_to_file(bindings_file).chain_err(|| "Unable to write bindings")
    }

    pub fn builder(&self) -> Builder {
        Builder {
            config: self,
            sources: Vec::new(),
            include_dirs: Vec::new(),
            target_dir: env::var_os("OUT_DIR").map(PathBuf::from).unwrap()
        }
    }

    pub fn bindgen(&self) -> Bindgen {
        Bindgen {
            config: self,
            include_dirs: Vec::new(),
            target_dir: env::var_os("OUT_DIR").map(PathBuf::from).unwrap(),
            options: bindgen::builder()
        }
    }
}

pub struct Builder<'a> {
    config: &'a Config,
    sources: Vec<PathBuf>,
    include_dirs: Vec<PathBuf>,
    target_dir: PathBuf
}

impl<'a> Builder<'a> {
    pub fn source<P: Into<PathBuf>>(mut self, source: P) -> Builder<'a> {
        self.sources.push(source.into());
        self
    }

    pub fn core_sources(mut self) -> Builder<'a> {
        collect_sources(&self.config.core_path, true, &mut self.sources);
        collect_sources(&self.config.variant_path, true, &mut self.sources);
        self
    }

    pub fn include_dir<P: Into<PathBuf>>(mut self, include_dir: P) -> Builder<'a> {
        self.include_dirs.push(include_dir.into());
        self
    }

    pub fn target_dir<P: Into<PathBuf>>(mut self, target_dir: P) -> Builder<'a> {
        self.target_dir = target_dir.into();
        self
    }

    pub fn build<S: Into<String>>(self, lib_name: S) -> Result<()> {
        let lib_name = lib_name.into();

        for source_file in self.sources {
            let object_file = self.target_dir.join(&lib_name).join(source_file.file_name().unwrap()).with_extension("o");
            self.config.compile(&source_file, &object_file, &self.include_dirs)?;
            self.config.archive(&object_file, &self.target_dir.join(format!("lib{}.a", lib_name)))?;
            //println!("cargo:rerun-if-changed={}", source_file.display());
        }

        println!("cargo:rustc-link-search=native={}", self.target_dir.display());
        println!("cargo:rustc-link-lib=static={}", lib_name);

        Ok(())
    }
}

pub struct Bindgen<'a> {
    config: &'a Config,
    include_dirs: Vec<PathBuf>,
    target_dir: PathBuf,
    options: BindgenBuilder
}

impl<'a> Bindgen<'a> {
    pub fn include_dir<P: Into<PathBuf>>(mut self, include_dir: P) -> Bindgen<'a> {
        self.include_dirs.push(include_dir.into());
        self
    }

    pub fn target_dir<P: Into<PathBuf>>(mut self, target_dir: P) -> Bindgen<'a> {
        self.target_dir = target_dir.into();
        self
    }

    pub fn options<F: FnOnce(BindgenBuilder) -> BindgenBuilder>(mut self, f: F) -> Bindgen<'a> {
        self.options = f(self.options);
        self
    }

    pub fn generate<P: Into<PathBuf>>(self, header_file: P) -> Result<()> {
        let header_file = header_file.into();
        self.config.generate_bindings(self.options, &header_file, &self.include_dirs, &self.target_dir)?;
        //println!("cargo:rerun-if-changed={}", header_file.display());

        Ok(())
    }
}



#[derive(Clone, Debug, Deserialize, Serialize)]
struct Recipe(String);

impl Recipe {
    fn from_prefs(prefs: &Preferences, name: &str) -> Recipe {
        Recipe(prefs.get::<String>(&format!("recipe.{}.pattern", name)).unwrap())
    }

    fn command(&self) -> PathBuf {
        let (command_path, _) = split_command_line(&self.0);
        command_path
    }

    fn substitute(&self, params: RecipeParams) -> (PathBuf, Vec<String>) {
        lazy_static! {
            static ref REGEX: Regex = Regex::new(r#"%(\w+)"#).unwrap();
        }

        let expanded = REGEX.replace_all(&self.0, |captures: &Captures| {
            params.substitute(&captures[1])
        });

        split_command_line(&expanded)
    }

    fn run(&self, params: RecipeParams) -> Result<Output> {
        let (command_path, args) = self.substitute(params);

        let mut command = Command::new(&command_path);
        command.args(args.as_slice());

        println!("{:?}", command);

        let output = command.output().chain_err(|| "Unable to start process")?;
        if output.status.success() {
            {
                let reader = BufReader::new(Cursor::new(&output.stderr));
                for warning in reader.lines().filter_map(|line| line.ok()).filter(|line| line.contains("warning:")) {
                    println!("cargo:warning={}", warning);
                }
            }
            Ok(output)
        } else {
            io::stderr().write_all(output.stderr.as_slice()).unwrap();
            Err(format!("Process '{}' exited with code {}", command_path.file_name().unwrap().to_string_lossy(),
                        output.status.code().map_or("<none>".to_string(), |code| code.to_string())).into())
        }
    }
}

#[derive(Default)]
struct RecipeParams {
    source_file: String,
    object_file: String,
    object_files: String,
    archive_file: String,
    includes: String
}

impl RecipeParams {
    fn substitute(&self, pattern: &str) -> String {
        match pattern {
            "source_file" => self.source_file.clone(),
            "object_file" => self.object_file.clone(),
            "object_files" => self.object_files.clone(),
            "archive_file" => self.archive_file.clone(),
            "includes" => self.includes.clone(),
            text => text.to_string()
        }
    }
}

pub fn split_command_line(line: &str) -> (PathBuf, Vec<String>) {
    lazy_static! {
        static ref REGEX: Regex = Regex::new(r#"\s*(?:'(.*?)')|(?:"(.*?)")|(\S+)"#).unwrap();
    }

    let mut parts = REGEX.captures_iter(line).map(|capture| {
        capture.get(1)
               .or_else(|| capture.get(2))
               .map_or_else(|| &capture[3], |capture| capture.as_str())
               .to_string()
    });

    let command = PathBuf::from(parts.next().unwrap());
    let args = parts.collect();

    (command, args)
}

fn collect_sources(dir: &Path, recursive: bool, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if recursive {
                collect_sources(&path, recursive, sources);
            }
        } else if is_source(&path) {
            sources.push(path);
        }
    }
}

fn get_system_includes(command: &OsStr, args: &[&str]) -> Vec<PathBuf> {
    Command::new(command).args(args).output().ok().map(|output| {
        let reader = BufReader::new(Cursor::new(&output.stderr));

        reader.lines()
              .filter_map(|line| line.ok())
              .skip_while(|line| !line.starts_with("#include <...>"))
              .skip(1)
              .take_while(|line| line.starts_with(' '))
              .map(|line| PathBuf::from(&line[1..]))
              .collect()
    }).unwrap_or_default()
}

fn is_asm_source(path: &Path) -> bool {
    path.is_file() && path.extension().and_then(OsStr::to_str).map_or(false, |extension| match extension {
        "s" | "S" | "sx" => true,
        _ => false
    })
}

fn is_c_source(path: &Path) -> bool {
    path.is_file() && path.extension().and_then(OsStr::to_str).map_or(false, |extension| match extension {
        "c" | "i" => true,
        _ => false
    })
}

fn is_cpp_source(path: &Path) -> bool {
    path.is_file() && path.extension().and_then(OsStr::to_str).map_or(false, |extension| match extension {
        "cc" | "cp" | "cxx" | "cpp" | "CPP" | "c++" | "C" | "ii" => true,
        _ => false
    })
}

fn is_source(path: &Path) -> bool {
    is_asm_source(path) || is_c_source(path) || is_cpp_source(path)
}

fn is_c_header(path: &Path) -> bool {
    path.is_file() && path.extension().and_then(OsStr::to_str).map_or(false, |extension| extension == "h")
}

fn is_cpp_header(path: &Path) -> bool {
    path.is_file() && path.extension().and_then(OsStr::to_str).map_or(false, |extension| match extension {
        "hh" | "H" | "hp" | "hxx" | "hpp" | "HPP" | "h++" | "tcc" => true,
        _ => false
    })
}
