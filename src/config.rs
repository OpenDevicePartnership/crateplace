use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::num::ParseIntError;
use std::path::Path;
use std::str::FromStr;
use toml_edit::{DocumentMut, Formatted, InlineTable, Item, Table, TomlError, Value};

use crate::FileConfigData;
use crate::file_error::{FileError, IOToFileResult};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy)]
pub struct ByteUnit {
    bytes: u64,
    format: ByteFormat,
}

#[derive(Debug, Clone, Copy)]
pub enum ByteFormat {
    Hex,
    Bytes,
    Kibi,
    Mebi,
    Gibi,
}

#[derive(thiserror::Error, Debug, Clone)]
pub enum UnitParseError {
    #[error("overflowed while converting: {0}, the number is too big")]
    OverFlow(String),
    #[error("failed to parse number: {0}")]
    Parsing(String, #[source] ParseIntError),
}

impl FromStr for ByteUnit {
    type Err = UnitParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            return Ok(ByteUnit {
                bytes: u64::from_str_radix(hex, 16)
                    .map_err(|err| UnitParseError::Parsing(s.to_owned(), err))?,
                format: ByteFormat::Hex,
            });
        }
        Ok(match s.chars().last().map(|c| c.to_ascii_uppercase()) {
            Some('K') => ByteUnit {
                bytes: u64::from_str(&s[..s.len() - 1])
                    .map_err(|err| UnitParseError::Parsing(s.to_string(), err))?
                    .checked_mul(1024)
                    .ok_or_else(|| UnitParseError::OverFlow(s.to_string()))?,
                format: ByteFormat::Kibi,
            },
            Some('M') => ByteUnit {
                bytes: u64::from_str(&s[..s.len() - 1])
                    .map_err(|err| UnitParseError::Parsing(s.to_string(), err))?
                    .checked_mul(1024 * 1024)
                    .ok_or_else(|| UnitParseError::OverFlow(s.to_string()))?,
                format: ByteFormat::Mebi,
            },
            Some('G') => ByteUnit {
                bytes: u64::from_str(&s[..s.len() - 1])
                    .map_err(|err| UnitParseError::Parsing(s.to_string(), err))?
                    .checked_mul(1024 * 1024 * 1024)
                    .ok_or_else(|| UnitParseError::OverFlow(s.to_string()))?,
                format: ByteFormat::Gibi,
            },
            _ => ByteUnit {
                bytes: u64::from_str(s)
                    .map_err(|err| UnitParseError::Parsing(s.to_string(), err))?,
                format: ByteFormat::Bytes,
            },
        })
    }
}

impl ByteUnit {
    pub fn new(bytes: u64, format: ByteFormat) -> Self {
        Self { bytes, format }
    }

    pub fn as_bytes(&self) -> u64 {
        self.bytes
    }
}

struct ByteUnitVisitor;

impl<'de> serde::de::Visitor<'de> for ByteUnitVisitor {
    type Value = ByteUnit;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a byte size or length: an integer, or a string like \"512K\" / \"0x1000\"")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        ByteUnit::from_str(v).map_err(E::custom)
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(ByteUnit {
            bytes: u64::try_from(v).map_err(|_| E::custom("byte unit cannot be negative"))?,
            format: ByteFormat::Bytes,
        })
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(ByteUnit {
            bytes: v,
            format: ByteFormat::Bytes,
        })
    }
}

impl std::fmt::Display for ByteUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.format {
            ByteFormat::Hex => write!(f, "0x{:02X}", self.bytes),
            ByteFormat::Bytes => write!(f, "{}", self.bytes),
            ByteFormat::Kibi => write!(f, "{}K", self.bytes / 1024),
            ByteFormat::Mebi => write!(f, "{}M", self.bytes / (1024 * 1024)),
            ByteFormat::Gibi => write!(f, "{}G", self.bytes / (1024 * 1024 * 1024)),
        }
    }
}

impl<'de> serde::Deserialize<'de> for ByteUnit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ByteUnitVisitor)
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
    #[error("toml parsing")]
    TomlParseError(
        #[source]
        #[from]
        toml::de::Error,
    ),
    #[error("file error")]
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
            &fs::read_to_string(path).into_in_result(path)?,
        )?)
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ConfigValidationError {
    #[error("section \"{1}\" overlaps with \"{0}\"")]
    Overlap(String, String),
    #[error("failed to parse \"{0}\" as a memory offset")]
    ParseError(
        #[source]
        #[from]
        UnitParseError,
    ),
    #[error("section has length of zero: \"{0}\"")]
    ZeroSection(String),
    #[error("section overflowed when calculating end position: \"{0}\"")]
    OverFlow(String),
    #[error("section has a priority which was already used: \"{0}\" with priority: {1}")]
    DoublePrio(String, u32),
    #[error("\"{0}\" was assigned non-existent section: \"{1}\"")]
    NonExistentSection(String, String),
    #[error("multiple sections are marked as default")]
    MultipleDefaults,
    #[error(
        "symbol assigned to emit no sections: {0}, symbol should have at least one of: text, rodata, or reldata set to true"
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
        if self.sections.contains_key(name) {
            return Err(ConfigModificationError::NameExists(name.to_string()));
        }
        self.sections.insert(
            name.to_string(),
            Section {
                origin,
                length,
                priority,
                default,
            },
        );
        self.validate()?;
        let mut toml: DocumentMut = fs::read_to_string(config_path)
            .into_in_result(config_path)?
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
        fs::write(config_path, toml.to_string().into_bytes()).into_out_result(config_path)?;
        Ok(())
    }

    pub fn remove_section(
        &mut self,
        config_path: &Path,
        name: &str,
    ) -> Result<(), ConfigModificationError> {
        self.sections
            .remove(name)
            .ok_or_else(|| ConfigModificationError::NameDoesNotExist(name.to_string()))?;
        self.validate()?;
        let mut toml: DocumentMut = fs::read_to_string(config_path)
            .into_in_result(config_path)?
            .parse()?;
        if let Item::Table(table) = toml
            .get_mut("sections")
            .ok_or(ConfigModificationError::FailedToFind("sections"))?
        {
            table
                .remove(name)
                .ok_or_else(|| ConfigModificationError::NameDoesNotExist(name.to_string()))?;
        } else {
            Err(ConfigModificationError::UnexpectedType("sections"))?
        }
        fs::write(config_path, toml.to_string().into_bytes()).into_out_result(config_path)?;
        Ok(())
    }

    pub fn add_crate(
        &mut self,
        config_path: &Path,
        name: &str,
        section: &str,
        include_dependencies: bool,
    ) -> Result<(), ConfigModificationError> {
        let crates = self.crates.get_or_insert_default();
        if crates.contains_key(name) {
            return Err(ConfigModificationError::NameExists(name.to_string()));
        }
        crates.insert(
            name.to_string(),
            CratePlacement {
                section: section.to_string(),
                include_dependencies,
            },
        );
        self.validate()?;
        let mut toml: DocumentMut = fs::read_to_string(config_path)
            .into_in_result(config_path)?
            .parse()?;

        let mut entry = InlineTable::new();
        entry.insert(
            "section",
            Value::String(Formatted::new(section.to_string())),
        );
        if include_dependencies {
            entry.insert("include_dependencies", Value::Boolean(Formatted::new(true)));
        }
        let res = Item::Value(Value::InlineTable(entry));
        match toml.get_mut("crates") {
            Some(element) => match element {
                Item::Table(table) => {
                    table.insert(name, res);
                }
                _ => {
                    return Err(ConfigModificationError::UnexpectedType("crates"));
                }
            },
            None => {
                let mut table = Table::new();
                table.insert(name, res);
                toml.insert("crates", Item::Table(table));
            }
        };
        fs::write(config_path, toml.to_string().into_bytes()).into_out_result(config_path)?;
        Ok(())
    }

    pub fn remove_crate(
        &mut self,
        config_path: &Path,
        name: &str,
    ) -> Result<(), ConfigModificationError> {
        let crates = self.crates.get_or_insert_default();
        if crates.remove(name).is_none() {
            return Err(ConfigModificationError::NameDoesNotExist(name.to_string()));
        }
        self.validate()?;
        let mut toml: DocumentMut = fs::read_to_string(config_path)
            .into_in_result(config_path)?
            .parse()?;
        match toml.get_mut("crates") {
            Some(table) => match table {
                Item::Table(table) => table
                    .remove(name)
                    .ok_or_else(|| ConfigModificationError::NameDoesNotExist(name.to_string()))?,
                _ => return Err(ConfigModificationError::UnexpectedType("crates")),
            },
            None => {
                return Err(ConfigModificationError::NameDoesNotExist(
                    "crates".to_string(),
                ));
            }
        };
        fs::write(config_path, toml.to_string().into_bytes()).into_out_result(config_path)?;
        Ok(())
    }

    pub fn add_symbol(
        &mut self,
        config_path: &Path,
        pattern: &str,
        section: &str,
        text: bool,
        rodata: bool,
        datarel: bool,
    ) -> Result<(), ConfigModificationError> {
        let symbols = self.symbols.get_or_insert_default();
        if symbols.contains_key(pattern) {
            return Err(ConfigModificationError::NameExists(pattern.to_string()));
        }
        symbols.insert(
            pattern.to_string(),
            SymPlacement {
                section: section.to_string(),
                symbol_types: SymbolTypes {
                    text,
                    rodata,
                    datarel,
                },
            },
        );
        self.validate()?;

        let mut toml: DocumentMut = fs::read_to_string(config_path)
            .into_in_result(config_path)?
            .parse()?;
        let mut entry = InlineTable::new();
        entry.insert(
            "section",
            Value::String(Formatted::new(section.to_string())),
        );
        if !text {
            entry.insert("text", Value::Boolean(Formatted::new(false)));
        }
        if !rodata {
            entry.insert("rodata", Value::Boolean(Formatted::new(false)));
        }
        if !datarel {
            entry.insert("datarel", Value::Boolean(Formatted::new(false)));
        }
        let res = Item::Value(Value::InlineTable(entry));
        match toml.get_mut("symbols") {
            Some(element) => match element {
                Item::Table(table) => {
                    table.insert(pattern, res);
                }
                _ => {
                    return Err(ConfigModificationError::UnexpectedType("symbols"));
                }
            },
            None => {
                let mut table = Table::new();
                table.insert(pattern, res);
                toml.insert("symbols", Item::Table(table));
            }
        };
        fs::write(config_path, toml.to_string().into_bytes()).into_out_result(config_path)?;
        Ok(())
    }
    pub fn remove_symbol(
        &mut self,
        config_path: &Path,
        pattern: &str,
    ) -> Result<(), ConfigModificationError> {
        let symbols = self.symbols.get_or_insert_default();
        if symbols.remove(pattern).is_none() {
            return Err(ConfigModificationError::NameDoesNotExist(
                pattern.to_string(),
            ));
        }
        self.validate()?;
        let mut toml: DocumentMut = fs::read_to_string(config_path)
            .into_in_result(config_path)?
            .parse()?;
        match toml.get_mut("symbols") {
            Some(table) => match table {
                Item::Table(table) => table.remove(pattern).ok_or_else(|| {
                    ConfigModificationError::NameDoesNotExist(pattern.to_string())
                })?,
                _ => return Err(ConfigModificationError::UnexpectedType("symbols")),
            },
            None => {
                return Err(ConfigModificationError::NameDoesNotExist(
                    "symbols".to_string(),
                ));
            }
        };
        fs::write(config_path, toml.to_string().into_bytes()).into_out_result(config_path)?;
        Ok(())
    }

    pub fn set_ram(
        &mut self,
        config_path: &Path,
        origin: ByteUnit,
        length: ByteUnit,
    ) -> Result<(), ConfigModificationError> {
        self.ram = Ram { origin, length };
        self.validate()?;
        let mut toml: DocumentMut = fs::read_to_string(config_path)
            .into_in_result(config_path)?
            .parse()?;
        let mut entry = InlineTable::new();
        entry.insert("origin", Value::String(Formatted::new(origin.to_string())));
        entry.insert("length", Value::String(Formatted::new(length.to_string())));
        toml.insert("ram", Item::Value(Value::InlineTable(entry)));
        fs::write(config_path, toml.to_string().into_bytes()).into_out_result(config_path)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigModificationError {
    #[error("name already exists: {0}")]
    NameExists(String),
    #[error("name does not exist: {0}")]
    NameDoesNotExist(String),
    #[error("validation")]
    Validation(
        #[source]
        #[from]
        ConfigValidationError,
    ),
    #[error("file error: {0}")]
    FileError(
        #[source]
        #[from]
        FileError,
    ),
    #[error("toml error: {0}")]
    TomlError(
        #[source]
        #[from]
        TomlError,
    ),
    #[error("failed to find: {0}")]
    FailedToFind(&'static str),
    #[error("unexpected type: {0}")]
    UnexpectedType(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_from_str(input: &str) -> Config {
        toml::from_str(input).unwrap()
    }

    fn valid_config() -> Config {
        config_from_str(
            r#"
                ram = { origin = "0x20000000", length = "64K" }

                [sections]
                flash = { origin = "0x08000000", length = "64K", priority = 1, default = true }
            "#,
        )
    }

    fn temporary_config_path(test_name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "crateplace-{test_name}-{}-Memory.toml",
            std::process::id()
        ))
    }

    #[test]
    fn byte_units_parse_to_bytes() {
        let cases = [
            ("0x1000", 4096),
            ("0Xff", 255),
            ("512", 512),
            ("2K", 2 * 1024),
            ("2k", 2 * 1024),
            ("3M", 3 * 1024 * 1024),
            ("3m", 3 * 1024 * 1024),
            ("4G", 4 * 1024 * 1024 * 1024),
            ("4g", 4 * 1024 * 1024 * 1024),
        ];

        for (input, expected) in cases {
            assert_eq!(ByteUnit::from_str(input).unwrap().as_bytes(), expected);
        }
    }

    #[test]
    fn byte_units_reject_invalid_values() {
        for input in ["", "0x", "K", "-1", "1.5M"] {
            assert!(ByteUnit::from_str(input).is_err(), "accepted {input:?}");
        }
    }

    #[test]
    fn byte_units_preserve_their_format_category() {
        for (input, expected) in [
            ("0x1000", "0x1000"),
            ("512", "512"),
            ("2K", "2K"),
            ("3M", "3M"),
            ("4G", "4G"),
        ] {
            assert_eq!(ByteUnit::from_str(input).unwrap().to_string(), expected);
        }

        assert_eq!(ByteUnit::new(2048, ByteFormat::Kibi).to_string(), "2K");
    }

    #[test]
    fn byte_units_deserialize_from_integers_and_strings() {
        #[derive(serde::Deserialize)]
        struct Values {
            integer: ByteUnit,
            string: ByteUnit,
        }

        let values: Values = toml::from_str("integer = 4096\nstring = \"4K\"").unwrap();

        assert_eq!(values.integer.as_bytes(), 4096);
        assert_eq!(values.string.as_bytes(), 4096);
        assert!(toml::from_str::<Values>("integer = -1\nstring = \"4K\"").is_err());
    }

    #[test]
    fn valid_configuration_passes_validation() {
        valid_config().validate().unwrap();
    }

    #[test]
    fn configuration_rejects_invalid_assignments() {
        let missing_crate_section = config_from_str(
            r#"
                ram = { origin = 0, length = "1K" }
                [sections]
                flash = { origin = "2K", length = "1K" }
                [crates]
                app = { section = "missing" }
            "#,
        );
        assert!(matches!(
            missing_crate_section.validate(),
            Err(ConfigValidationError::NonExistentSection(name, section))
                if name == "app" && section == "missing"
        ));

        let disabled_symbol = config_from_str(
            r#"
                ram = { origin = 0, length = "1K" }
                [sections]
                flash = { origin = "2K", length = "1K" }
                [symbols]
                handler = { section = "flash", text = false, rodata = false, datarel = false }
            "#,
        );
        assert!(matches!(
            disabled_symbol.validate(),
            Err(ConfigValidationError::SymbolWithoutSections(name)) if name == "handler"
        ));
    }

    #[test]
    fn configuration_rejects_multiple_default_sections() {
        let config = config_from_str(
            r#"
                ram = { origin = 0, length = "1K" }
                [sections]
                first = { origin = "2K", length = "1K", priority = 1, default = true }
                second = { origin = "3K", length = "1K", priority = 2, default = true }
            "#,
        );

        assert!(matches!(
            config.validate(),
            Err(ConfigValidationError::MultipleDefaults)
        ));
    }

    #[test]
    fn configuration_mutations_are_persisted() {
        let path = temporary_config_path("mutations");
        let mut config = valid_config();
        fs::write(&path, toml::to_string(&config).unwrap()).unwrap();

        config
            .add_section(
                &path,
                "boot",
                ByteUnit::from_str("0x08010000").unwrap(),
                ByteUnit::from_str("32K").unwrap(),
                2,
                false,
            )
            .unwrap();
        config.add_crate(&path, "app", "boot", true).unwrap();
        config
            .add_symbol(&path, "handler*", "boot", true, false, false)
            .unwrap();
        config
            .set_ram(
                &path,
                ByteUnit::from_str("0x30000000").unwrap(),
                ByteUnit::from_str("128K").unwrap(),
            )
            .unwrap();

        let persisted = Config::from_file(&path).unwrap();
        persisted.validate().unwrap();
        assert_eq!(persisted.ram.origin.as_bytes(), 0x30000000);
        assert_eq!(persisted.ram.length.as_bytes(), 128 * 1024);
        let boot = persisted.sections.get("boot").unwrap();
        assert_eq!(boot.origin.as_bytes(), 0x08010000);
        assert_eq!(boot.length.as_bytes(), 32 * 1024);
        assert_eq!(boot.priority, 2);
        assert!(!boot.default);
        let app = persisted.crates.as_ref().unwrap().get("app").unwrap();
        assert_eq!(app.section, "boot");
        assert!(app.include_dependencies);
        let handler = persisted.symbols.as_ref().unwrap().get("handler*").unwrap();
        assert_eq!(handler.section, "boot");
        assert!(handler.symbol_types.text);
        assert!(!handler.symbol_types.rodata);
        assert!(!handler.symbol_types.datarel);

        config.remove_symbol(&path, "handler*").unwrap();
        config.remove_crate(&path, "app").unwrap();
        config.remove_section(&path, "boot").unwrap();

        let persisted = Config::from_file(&path).unwrap();
        assert!(!persisted.sections.contains_key("boot"));
        assert!(!persisted.crates.unwrap().contains_key("app"));
        assert!(!persisted.symbols.unwrap().contains_key("handler*"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn configuration_mutations_reject_duplicate_or_missing_names() {
        let path = temporary_config_path("mutation-errors");
        let mut config = valid_config();
        fs::write(&path, toml::to_string(&config).unwrap()).unwrap();

        assert!(matches!(
            config.add_section(
                &path,
                "flash",
                ByteUnit::new(0, ByteFormat::Bytes),
                ByteUnit::new(1, ByteFormat::Bytes),
                2,
                false,
            ),
            Err(ConfigModificationError::NameExists(name)) if name == "flash"
        ));
        config.add_crate(&path, "app", "flash", true).unwrap();
        assert!(matches!(
            config.add_crate(&path, "app", "flash", false),
            Err(ConfigModificationError::NameExists(name)) if name == "app"
        ));
        config
            .add_symbol(&path, "handler*", "flash", true, true, true)
            .unwrap();
        assert!(matches!(
            config.add_symbol(&path, "handler*", "flash", true, false, false),
            Err(ConfigModificationError::NameExists(name)) if name == "handler*"
        ));
        assert!(matches!(
            config.remove_section(&path, "missing"),
            Err(ConfigModificationError::NameDoesNotExist(name)) if name == "missing"
        ));
        assert!(matches!(
            config.remove_crate(&path, "missing"),
            Err(ConfigModificationError::NameDoesNotExist(name)) if name == "missing"
        ));
        assert!(matches!(
            config.remove_symbol(&path, "missing"),
            Err(ConfigModificationError::NameDoesNotExist(name)) if name == "missing"
        ));

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn adjacent_ranges_do_not_overlap() {
        let mut checker = ConfigChecker::new();

        checker
            .check("first".into(), 0x1000, 0x100, Some(1))
            .unwrap();
        checker
            .check("second".into(), 0x1100, 0x100, Some(2))
            .unwrap();
    }

    #[test]
    fn overlapping_ranges_are_rejected() {
        let mut checker = ConfigChecker::new();
        checker
            .check("first".into(), 0x1000, 0x100, Some(1))
            .unwrap();

        let error = checker
            .check("second".into(), 0x1080, 0x100, Some(2))
            .unwrap_err();

        assert!(matches!(
            error,
            ConfigValidationError::Overlap(name, occupied)
                if name == "second" && occupied == "first"
        ));
    }

    #[test]
    fn invalid_range_lengths_are_rejected() {
        let mut checker = ConfigChecker::new();

        assert!(matches!(
            checker.check("empty".into(), 0x1000, 0, None),
            Err(ConfigValidationError::ZeroSection(name)) if name == "empty"
        ));
        assert!(matches!(
            checker.check("overflow".into(), u64::MAX, 1, None),
            Err(ConfigValidationError::OverFlow(name)) if name == "overflow"
        ));
    }

    #[test]
    fn duplicate_priorities_are_rejected() {
        let mut checker = ConfigChecker::new();
        checker
            .check("first".into(), 0x1000, 0x100, Some(7))
            .unwrap();

        assert!(matches!(
            checker.check("second".into(), 0x2000, 0x100, Some(7)),
            Err(ConfigValidationError::DoublePrio(name, 7)) if name == "second"
        ));
    }
}
