use std::ffi::{OsStr, OsString};
use std::iter;
use std::slice;

pub struct Tool {
    command: PathBuf,
    args: Vec<OsString>
}

impl Tool {
    pub fn command(&self) -> &Path {
        &self.command
    }

    pub fn args(&self) -> Args {
        self.args.iter()
    }

    pub fn run() -> Result<Output, Output> {

    }
}

pub struct Args<'a>(slice::Iter<'a, OsString>);

impl<'a> Iterator for Args<'a> {
    type Item = &'a OsStr;

    fn next(&mut self) -> Option<&'a OsStr> {
        self.0.next().map(OsString::as_os_str)
    }
}
