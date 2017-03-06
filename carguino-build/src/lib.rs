#[macro_use] extern crate error_chain;
extern crate bindgen;
#[macro_use] extern crate lazy_static;
extern crate regex;
#[macro_use] extern crate serde_derive;
extern crate serde_json;

pub use error::*;
pub use config::Config;
pub use prefs::Preferences;

#[doc(hidden)]
pub mod config;
mod error;
mod prefs;
