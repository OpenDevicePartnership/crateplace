use serde::Serialize;
use serde::de::Error as DeSerializationError;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::str::FromStr;
use toml_edit::{DocumentMut, Formatted, InlineTable, Item, TomlError, Value};

use crate::FileConfigData;
use crate::file_error::{FileError, IOToFileResult};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy)]
pub enum ByteUnit {
    H(u64),
    B(u64),
    K(u64),
    M(u64),
    G(u64),
}

#[derive(thiserror::Error, Debug, Clone)]
#[error("Failed to parse byte unit: {0}")]
pub struct UnitParseError(String);

impl FromStr for ByteUnit {
    type Err = UnitParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            return Ok(ByteUnit::H(
                u64::from_str_radix(hex, 16).map_err(|_| UnitParseError(s.to_owned()))?,
            ));
        }
        Ok(match s.chars().last() {
            Some('K') => ByteUnit::K(
                u64::from_str(&s[..s.len() - 1]).map_err(|_| UnitParseError(s.to_string()))?,
            ),
            Some('M') => ByteUnit::M(
                u64::from_str(&s[..s.len() - 1]).map_err(|_| UnitParseError(s.to_string()))?,
            ),
            Some('G') => ByteUnit::G(
                u64::from_str(&s[..s.len() - 1]).map_err(|_| UnitParseError(s.to_string()))?,
            ),
            _ => ByteUnit::B(u64::from_str(s).map_err(|_| UnitParseError(s.to_string()))?),
        })
    }
}

impl ByteUnit {
    pub fn as_bytes(&self) -> u64 {
        match self {
            ByteUnit::H(value) => *value,
            ByteUnit::B(value) => *value,
            ByteUnit::K(value) => value * 1024,
            ByteUnit::M(value) => value * 1024 * 1024,
            ByteUnit::G(value) => value * 1024 * 1024 * 1024,
        }
    }
}

impl std::fmt::Display for ByteUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ByteUnit::H(value) => write!(f, "{value:02X}"),
            ByteUnit::B(value) => write!(f, "{value}"),
            ByteUnit::K(value) => write!(f, "{value}K"),
            ByteUnit::M(value) => write!(f, "{value}M"),
            ByteUnit::G(value) => write!(f, "{value}G"),
        }
    }
}

impl<'de> serde::Deserialize<'de> for ByteUnit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: &str = serde::Deserialize::deserialize(deserializer)?;
        Self::from_str(s).map_err(D::Error::custom)
    }
}

impl Serialize for ByteUnit {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct SymbolTypes {
    #[serde(default = "default_true")]
    pub text: bool,
    #[serde(default = "default_true")]
    pub rodata: bool,
    #[serde(default = "default_true")]
    pub datarel: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct CratePlacement {
    pub section: String,
    #[serde(default = "default_true")]
    pub include_dependencies: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct SymPlacement {
    pub section: String,
    #[serde(flatten)]
    pub symbol_types: SymbolTypes,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Section {
    pub origin: ByteUnit,
    pub length: ByteUnit,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub default: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Ram {
    pub(crate) origin: ByteUnit,
    pub(crate) length: ByteUnit,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Config {
    pub(crate) ram: Ram,
    pub(crate) sections: HashMap<String, Section>,
    pub(crate) crates: Option<HashMap<String, CratePlacement>>,
    pub(crate) symbols: Option<HashMap<String, SymPlacement>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigLoadError {
    #[error("Toml parse error")]
    TomlParseError(
        #[source]
        #[from]
        toml::de::Error,
    ),
    #[error("File error")]
    FileError(
        #[source]
        #[from]
        FileError,
    ),
}

impl FileConfigData for Config {
    type Error = ConfigLoadError;

    fn from_file(path: &Path) -> Result<Self, Self::Error> {
        Ok(toml::from_str(
            &fs::read_to_string(path).file_in_result(path)?,
        )?)
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ConfigValidationError {
    #[error("Section \"{1}\" overlaps with \"{0}\"")]
    Overlap(String, String),
    #[error("Failed to parse \"{0}\" as a memory offset")]
    ParseError(
        #[source]
        #[from]
        UnitParseError,
    ),
    #[error("Parse error")]
    ZeroSection(String),
    #[error("Section overflowed when calculating end position: \"{0}\"")]
    OverFlow(String),
    #[error("Section has a priority which was already used: \"{0}\" with priority: {1}")]
    DoublePrio(String, u32),
    #[error("\"{0}\" was assigned non-existent section: \"{1}\"")]
    NonExistentSection(String, String),
    #[error("Multiple sections are marked as default")]
    MultipleDefaults,
    #[error(
        "Symbol assigned to emit no sections: {0}, symbol should have at least one of: text, rodata, or reldata set to true"
    )]
    SymbolWithoutSections(String),
}

struct OccupiedSpace {
    name: String,
    origin: u64,
    end: u64,
}

struct ConfigChecker {
    occupied: Vec<OccupiedSpace>,
    prios: HashSet<u32>,
}

impl ConfigChecker {
    fn new() -> Self {
        Self {
            occupied: Vec::new(),
            prios: HashSet::new(),
        }
    }

    fn check(
        &mut self,
        name: String,
        origin: u64,
        len: u64,
        prio: Option<u32>,
    ) -> Result<(), ConfigValidationError> {
        if let Some(prio) = prio {
            if self.prios.contains(&prio) {
                return Err(ConfigValidationError::DoublePrio(name, prio));
            }
            self.prios.insert(prio);
        }
        if len == 0 {
            return Err(ConfigValidationError::ZeroSection(name));
        }
        let end = origin
            .checked_add(len)
            .ok_or_else(|| ConfigValidationError::OverFlow(name.clone()))?;
        for section in &self.occupied {
            if section.origin < end && origin < section.end {
                return Err(ConfigValidationError::Overlap(name, section.name.clone()));
            }
        }
        self.occupied.push(OccupiedSpace { name, origin, end });
        Ok(())
    }
}

impl Config {
    fn section_existense(&self) -> Result<(), ConfigValidationError> {
        if let Some(symbols) = &self.symbols {
            for (name, sym) in symbols {
                if !self.sections.contains_key(&sym.section) {
                    return Err(ConfigValidationError::NonExistentSection(
                        name.clone(),
                        sym.section.clone(),
                    ));
                }
            }
        }
        if let Some(crates) = &self.crates {
            for (name, p_crate) in crates {
                if !self.sections.contains_key(&p_crate.section) {
                    return Err(ConfigValidationError::NonExistentSection(
                        name.clone(),
                        p_crate.section.clone(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn check_symbol_emit(&self) -> Result<(), ConfigValidationError> {
        if let Some(symbols) = &self.symbols {
            for (name, symbol) in symbols {
                if !symbol.symbol_types.text
                    && !symbol.symbol_types.rodata
                    && !symbol.symbol_types.datarel
                {
                    return Err(ConfigValidationError::SymbolWithoutSections(name.clone()));
                }
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        self.section_existense()?;
        self.check_symbol_emit()?;
        let mut checker = ConfigChecker::new();
        checker.check(
            "ram".to_string(),
            self.ram.origin.as_bytes(),
            self.ram.length.as_bytes(),
            None,
        )?;
        let mut default_found = false;
        for (name, section) in &self.sections {
            if section.default {
                if default_found {
                    return Err(ConfigValidationError::MultipleDefaults);
                } else {
                    default_found = true;
                }
            }
            checker.check(
                name.to_string(),
                section.origin.as_bytes(),
                section.length.as_bytes(),
                Some(section.priority),
            )?;
        }
        Ok(())
    }

    pub fn add_section(
        &mut self,
        config_path: &Path,
        name: &str,
        origin: ByteUnit,
        length: ByteUnit,
        priority: u32,
        default: bool,
    ) -> Result<(), ConfigModificationError> {
        let new_section = Section {
            origin,
            length,
            priority,
            default,
        };
        if self.sections.contains_key(name) {
            return Err(ConfigModificationError::SectionNameExists(name.to_string()));
        }
        self.sections.insert(name.to_string(), new_section);
        self.validate()?;
        let mut toml: DocumentMut = fs::read_to_string(config_path)
            .file_in_result(config_path)?
            .parse()?;
        if let Item::Table(table) = toml
            .get_mut("sections")
            .ok_or(ConfigModificationError::FailedToFind("sections"))?
        {
            let mut entry = InlineTable::new();
            entry.insert("origin", Value::String(Formatted::new(origin.to_string())));
            entry.insert("length", Value::String(Formatted::new(length.to_string())));
            entry.insert("priority", Value::Integer(Formatted::new(priority.into())));
            if default {
                entry.insert("default", Value::Boolean(Formatted::new(true)));
            }
            table.insert(name, Item::Value(Value::InlineTable(entry)));
        } else {
            return Err(ConfigModificationError::UnexpectedType("sections"));
        }
        fs::write(config_path, toml.to_string().into_bytes()).file_out_result(config_path)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigModificationError {
    #[error("Section name already exists: {0}")]
    SectionNameExists(String),
    #[error("Validation")]
    Validation(
        #[source]
        #[from]
        ConfigValidationError,
    ),
    #[error("File error: {0}")]
    FileError(
        #[source]
        #[from]
        FileError,
    ),
    #[error("Toml error: {0}")]
    TomlError(
        #[source]
        #[from]
        TomlError,
    ),
    #[error("Failed to find: {0}")]
    FailedToFind(&'static str),
    #[error("Unexpected type: {0}")]
    UnexpectedType(&'static str),
}
