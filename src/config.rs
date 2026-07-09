use std::collections::{HashMap, HashSet};
use std::fs;
use std::str::FromStr;

use crate::FileConfigData;
use crate::file_error::{FileError, IOToFileError};

fn default_true() -> bool {
    true
}

pub(crate) fn parse_offset(value: &str) -> Result<u64, ConfigValidationError> {
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return u64::from_str_radix(hex, 16)
            .map_err(|_| ConfigValidationError::ParseError(value.to_owned()));
    }
    let (digits, mult) = match value.chars().last() {
        Some('K') => (&value[..value.len() - 1], 1024),
        Some('M') => (&value[..value.len() - 1], 1024 * 1024),
        Some('G') => (&value[..value.len() - 1], 1024 * 1024 * 1024),
        _ => (value, 1),
    };
    Ok(
        u64::from_str(digits).map_err(|_| ConfigValidationError::ParseError(value.to_owned()))?
            * mult,
    )
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
    pub origin: String,
    pub length: String,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub default: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Ram {
    pub(crate) origin: String,
    pub(crate) length: String,
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

    fn from_file(path: &std::path::Path) -> Result<Self, Self::Error> {
        Ok(toml::from_str(&fs::read_to_string(path).read_error(path)?)?)
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ConfigValidationError {
    #[error("Section \"{1}\" overlaps with \"{0}\"")]
    Overlap(String, String),
    #[error("Failed to parse \"{0}\" as a memory offset")]
    ParseError(String),
    #[error("Section has a size of zero: \"{0}\"")]
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
            parse_offset(&self.ram.origin)?,
            parse_offset(&self.ram.length)?,
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
                parse_offset(&section.origin)?,
                parse_offset(&section.length)?,
                Some(section.priority),
            )?;
        }
        Ok(())
    }
}
