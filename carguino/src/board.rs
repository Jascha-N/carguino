use Result;

use regex::Regex;

use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};
use std::iter::FromIterator;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardInfo {
    vendor: String,
    arch: String,
    board: String,
    #[serde(default)]
    params: HashMap<String, String>
}

impl BoardInfo {
    pub fn from_fqbn(fqbn: &str) -> Result<BoardInfo> {
        lazy_static! {
            static ref REGEX: Regex = Regex::new(
                r#"^(\S+?):(\S+?):(\S+?)(?::((?:\S+?=\S+?)(?:,?:\S+?=\S+?)*))?$"#
            ).unwrap();
        }
        REGEX.captures(fqbn).map(|captures| {
            let params = HashMap::from_iter(captures.get(4).iter().flat_map(|capture| {
            capture.as_str().split(',')
            }).map(|pair| {
                let mut iter = pair.split('=');
                (iter.next().unwrap().to_string(), iter.next().unwrap().to_string())
            }));
            BoardInfo {
                vendor: captures[1].to_string(),
                arch: captures[2].to_string(),
                board: captures[3].to_string(),
                params: params
            }
        }).map_or_else(|| Err("Invalid fully-qualified board name".into()), Ok)
    }

    pub fn vendor(&self) -> &str {
        &self.vendor
    }

    pub fn arch(&self) -> &str {
        &self.arch
    }

    pub fn board(&self) -> &str {
        &self.board
    }
}

impl Display for BoardInfo {
    fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
        write!(fmt, "{}:{}:{}", self.vendor, self.arch, self.board)?;
        if !self.params.is_empty() {
            write!(fmt, ":{}", self.params.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join(","))?;
        }
        Ok(())
    }
}
