use std::borrow::Cow;
use std::path::PathBuf;
use std::process::Output;

error_chain! {
    errors {
        Process(name: PathBuf, output: Output) {
            description("process exited unexpectedly")
            display("Process '{}' exited with code {}", name.display(),
                    output.status.code().map_or(Cow::Borrowed("<none>"), |code| Cow::Owned(code.to_string())))
        }
    }
}
