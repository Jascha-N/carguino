use {BoardInfo, Result};

use cargo::util::{self, ProcessBuilder};
use carguino_build::Preferences;

use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Builder {
    prefs: Vec<String>,
    board: String,
    home: Option<PathBuf>,
    hardware: Vec<PathBuf>,
    tools: Vec<PathBuf>,
    libraries: Vec<PathBuf>,
    built_in_libraries: Vec<PathBuf>
}

impl Builder {
    pub fn new(board: &BoardInfo) -> Builder {
        Builder {
            prefs: Vec::new(),
            board: board.to_string(),
            home: None,
            hardware: Vec::new(),
            tools: Vec::new(),
            libraries: Vec::new(),
            built_in_libraries: Vec::new()
        }
    }

    pub fn home<P: Into<PathBuf>>(&mut self, path: P) -> &mut Builder {
        self.home = Some(path.into());
        self
    }

    pub fn hardware<P: Into<PathBuf>>(&mut self, path: P) -> &mut Builder {
        self.hardware.push(path.into());
        self
    }

    pub fn tools<P: Into<PathBuf>>(&mut self, path: P) -> &mut Builder {
        self.tools.push(path.into());
        self
    }

    pub fn libraries<P: Into<PathBuf>>(&mut self, path: P) -> &mut Builder {
        self.libraries.push(path.into());
        self
    }

    pub fn pref<K: AsRef<str>, V: ToString>(&mut self, key: K, value: V) -> &mut Builder {
        self.prefs.push(format!("{}={}", key.as_ref(), value.to_string()));
        self
    }

    fn base_command(&self) -> ProcessBuilder {
        let mut command = if let Some(ref home) = self.home { //self.home.or_else(|| env::var_os("ARDUINO_HOME").map(PathBuf::from)) {
            let mut command = util::process(home.join("arduino-builder"));
            command.arg("-built-in-libraries").arg(home.join("libraries"))
                   .arg("-hardware").arg(home.join("hardware"))
                   .arg("-tools").arg(home.join("hardware/tools/avr"))
                   .arg("-tools").arg(home.join("tools-builder"));
            command
        } else {
            util::process("arduino-builder")
        };

        for path in &self.hardware {
            command.arg("-hardware").arg(path);
        }
        for path in &self.tools {
            command.arg("-tools").arg(path);
        }
        for path in &self.built_in_libraries {
            command.arg("-built-in-libraries").arg(path);
        }
        for path in &self.libraries {
            command.arg("-libraries").arg(path);
        }

        command.arg("-fqbn").arg(self.board.to_string());
        command.arg("-warnings").arg("all");
        command.arg("-prefs").arg("compiler.warning_flags={compiler.warning_flags.all}");

        for pref in &self.prefs {
            command.arg("-prefs").arg(pref);
        }

        command
    }

    pub fn dump_prefs(&self, src: &Path) -> Result<Preferences> {
        let output = self.base_command()
                         .arg("-dump-prefs")
                         .arg(src)
                         .exec_with_output()?;

        let prefs = Preferences::parse(String::from_utf8_lossy(&output.stdout));

        Ok(prefs)
    }
}
