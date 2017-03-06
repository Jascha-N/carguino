use regex::{Captures, Regex};

use std::fmt::{self, Display, Formatter};
use std::cell::{Ref, RefCell};
use std::collections::btree_map::{self, BTreeMap};
use std::str::FromStr;

#[derive(Clone, Debug, Default)]
pub struct Preferences {
    unexpanded: BTreeMap<String, String>,
    expanded: RefCell<Option<BTreeMap<String, String>>>
}

impl Preferences {
    pub fn new() -> Preferences {
        Preferences::default()
    }

    pub fn parse<S: AsRef<str>>(string: S) -> Preferences {
        let mut prefs = BTreeMap::new();
        for line in string.as_ref().lines() {
            // if line.starts_with("===") || !line.contains('=') {
            //     continue;
            // }
            let mut splits = line.splitn(2, '=');
            let key = splits.next().unwrap();
            let value = splits.next().unwrap();
            prefs.insert(key.to_string(), value.to_string());
        }
        Preferences {
            unexpanded: prefs,
            expanded: RefCell::new(None)
        }
    }

    pub fn set<V: ToString>(&mut self, key: &str, value: V) {
        self.unexpanded.insert(key.to_string(), value.to_string());
        self.expanded.borrow_mut().take();
    }

    pub fn unset(&mut self, key: &str) {
        self.unexpanded.remove(key);
        self.expanded.borrow_mut().take();
    }

    pub fn get_unexpanded<R: FromStr>(&self, key: &str) -> Option<R> {
        self.unexpanded.get(key).and_then(|value| value.parse().ok())
    }

    pub fn get<R: FromStr>(&self, key: &str) -> Option<R> {
        self.expanded().get(key).and_then(|value| value.parse().ok())
    }

    fn expanded(&self) -> Ref<BTreeMap<String, String>> {
        {
            let mut expanded = self.expanded.borrow_mut();
            if expanded.is_none() {
                let mut prefs = self.unexpanded.clone();
                lazy_static! {
                    static ref REGEX: Regex = Regex::new(r#"\{(\S+?)\}"#).unwrap();
                }
                for _ in 0 .. 10 {
                    let mut new_prefs = BTreeMap::new();
                    for (key, value) in &prefs {
                        new_prefs.insert(key.clone(), REGEX.replace_all(value, |captures: &Captures| {
                            prefs.get(&captures[1])
                                .cloned()
                                .unwrap_or_else(|| captures[0].to_string())
                        }).replace("{{", "{").replace("}}", "}"));
                    }
                    if prefs == new_prefs {
                        break;
                    }
                    prefs = new_prefs;
                }
                *expanded = Some(prefs);
            }
        }
        let expanded = self.expanded.borrow();
        Ref::map(expanded, |expanded| expanded.as_ref().unwrap())
    }

    pub fn keys(&self) -> btree_map::Keys<String, String> {
        self.unexpanded.keys()
    }

    // pub fn tool(&self, name: &str) -> Preferences {
    //     self.expand()
    // }
}

impl Display for Preferences {
    fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
        for (key, value) in self.expanded().iter() {
            writeln!(fmt, "{}={}", key, value)?;
        }
        Ok(())
    }
}
