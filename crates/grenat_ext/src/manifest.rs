//! What a library says of itself: its ABI version, its functions (names,
//! documentation, Grenat types, effects, purity) and the structs they use.
//! Grenat reads it once, when the facet is installed, and writes the
//! declarations it stands for.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub abi: u32,
    pub functions: Vec<Function>,
    pub structs: Vec<Struct>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Function {
    /// The name Grenat calls it by.
    pub name: String,
    /// Its entry point (`grenat_ext_v1_<name>`).
    pub symbol: String,
    pub doc: Option<String>,
    pub params: Vec<Field>,
    /// A Grenat type: `Array(Array(String))`, `Int?`, `Cell`, `Nil`.
    pub returns: String,
    /// As Grenat writes them: `fs.read`, `net("api.x.com")`.
    pub effects: Vec<String>,
    /// The result depends on the arguments only: trusted, not tainted.
    pub pure: bool,
    /// The Grenat error an `Err` raises.
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Struct {
    pub name: String,
    pub doc: Option<String>,
    pub fields: Vec<Field>,
}

/// A parameter, or a struct's field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub doc: Option<String>,
}

impl Manifest {
    pub fn function(&self, name: &str) -> Option<&Function> {
        self.functions.iter().find(|f| f.name == name)
    }
}
