error_chain! {
    links {
        Build(::carguino_build::Error, ::carguino_build::ErrorKind);
    }

    foreign_links {
        Docopt(::docopt::Error);
        Cargo(Box<::cargo::CargoError>);
    }
}

impl From<::cargo::util::ProcessError> for Error {
    fn from(error: ::cargo::util::ProcessError) -> Error {
        ErrorKind::Cargo(Box::new(error)).into()
    }
}
