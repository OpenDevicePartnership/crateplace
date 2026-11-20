use core::fmt;
use object::{Object, ObjectSection, ObjectSymbol};
use regex::RegexSet;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io::{Read, Write};
use std::iter::Peekable;
use std::path::Path;
use std::str::Chars;

use crate::FileConfigData;
use crate::config::{Config, Section};
use crate::deps::DepTree;
use crate::file_error::{FileError, IOToFileResult};
use crate::mangling::ManglingVersion;

#[derive(Debug, Clone)]
pub enum ProblemLevel {
    Warning,
    Error,
    Ignored,
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationProblem {
    #[error(
        "symbol too big: \"{name}\"start: {symbol_start:02x}, end: {symbol_end:02x}, section: \"{section_name}\" end: {section_end:02x}"
    )]
    SymbolTooBig {
        name: String,
        section_name: String,
        symbol_start: u64,
        symbol_end: u64,
        section_end: u64,
    },
    #[error(
        "symbol placed incorrectly: \"{name}\" position: {symbol_position:02x}, section \"{section_name}\" start: {section_start:02x} end: {section_end:02x}"
    )]
    SymbolPlacement {
        name: String,
        section_name: String,
        symbol_position: u64,
        section_start: u64,
        section_end: u64,
    },
    #[error(
        "symbol placed incorrectly: \"{name}\", belonging to {owner} section: \"{correct_section}\", actual section: \"{actual_section}\""
    )]
    SymbolAssignment {
        name: String,
        owner: String,
        correct_section: String,
        actual_section: String,
    },
    #[error("unknown mangling scheme: \"{name}, mangled: \"{mangled_name}\"")]
    UnknownManglingScheme { name: String, mangled_name: String },
    #[error("failed to identify crate of: \"{name}\"")]
    NoCrateName { name: String },
    #[error("failed to classify symbol: \"{name}\"")]
    ClassificationFailure { name: String },
    #[error("symbol \"{name}\" owned by non-existent crate: \"{crate_name}\"")]
    NonExistentCrate { name: String, crate_name: String },
    #[error("crate \"{crate_name}\" assigned to non-existent section: \"{section}\"")]
    NonExistentSectionCrate { crate_name: String, section: String },
    #[error("symbol \"{symbol}\" assigned to non-existent section: \"{section}\"")]
    NonExistentSection { symbol: String, section: String },
    #[error("glob pattern is invalid: \"{pattern}\"")]
    InvalidGlobPattern {
        pattern: String,
        #[source]
        error: glob::PatternError,
    },
    #[error("overflow computing section end: {section} start: {start:02x} length: {length:02x}")]
    SectionOverflow {
        section: String,
        start: u64,
        length: u64,
    },
    #[error("overflow computing symbol end: {symbol} start: {start:02x} length: {length:02x}")]
    SymbolOverflow {
        symbol: String,
        start: u64,
        length: u64,
    },
    #[error("{0}")]
    Ignored(String),
}

impl ValidationProblem {
    pub fn problem_level(&self) -> ProblemLevel {
        match self {
            ValidationProblem::SymbolTooBig { .. } => ProblemLevel::Error,
            ValidationProblem::SymbolPlacement { .. } => ProblemLevel::Error,
            ValidationProblem::SymbolAssignment { .. } => ProblemLevel::Error,
            ValidationProblem::ClassificationFailure { .. } => ProblemLevel::Error,
            ValidationProblem::UnknownManglingScheme { .. } => ProblemLevel::Warning,
            ValidationProblem::NoCrateName { .. } => ProblemLevel::Warning,
            ValidationProblem::NonExistentCrate { .. } => ProblemLevel::Error,
            ValidationProblem::NonExistentSection { .. } => ProblemLevel::Error,
            ValidationProblem::NonExistentSectionCrate { .. } => ProblemLevel::Error,
            ValidationProblem::InvalidGlobPattern { .. } => ProblemLevel::Error,
            ValidationProblem::SectionOverflow { .. } => ProblemLevel::Error,
            ValidationProblem::SymbolOverflow { .. } => ProblemLevel::Error,
            ValidationProblem::Ignored(_) => ProblemLevel::Ignored,
        }
    }
}

static DEFAULT_IGNORELIST: &[&str] = &[
    "^\\{\"package\":.*,\"tag\":\"defmt.*,\"data\":",
    "^\\.Lanon",
    "^__aeabi",
    "^__defmt",
    "^_defmt",
    "^_critical_section",
    "^Reset$",
    "^__DEFMT",
    "^_MergedGlobals$",
    "^__pre_init$",
    "^pre_init$",
    "^defmt_",
    "^default_",
    "^DEFMT_",
    "^main$",
    "^S$",
    "^__ONCE__$",
    "^__cortex_m_rt",
];

#[derive(Debug, Clone)]
pub struct IgnoreList {
    entries: regex::RegexSet,
}

impl FileConfigData for IgnoreList {
    type Error = ValidationError;

    fn from_file(path: &Path) -> Result<Self, Self::Error> {
        Self::from_file(path)
    }
}

impl IgnoreList {
    pub fn new<I, S>(exprs: I) -> Result<Self, ValidationError>
    where
        S: AsRef<str>,
        I: IntoIterator<Item = S>,
    {
        Ok(Self {
            entries: regex::RegexSet::new(exprs)?,
        })
    }

    pub fn from_file(path: &Path) -> Result<Self, ValidationError> {
        let mut file = File::open(path).into_in_result(path)?;
        let mut content = String::new();
        file.read_to_string(&mut content).into_in_result(path)?;
        Ok(Self {
            entries: regex::RegexSet::new(content.lines())?,
        })
    }

    pub fn patterns(&self) -> &[String] {
        self.entries.patterns()
    }

    pub fn matches(&self, symbol: &str) -> bool {
        self.entries.is_match(symbol)
    }

    pub fn to_file(&self, path: &Path) -> Result<(), std::io::Error> {
        let mut file = File::create(path)?;
        let res = self
            .entries
            .patterns()
            .iter()
            .map(String::as_str)
            .collect::<Vec<&str>>()
            .join("\n");
        file.write_all(res.as_bytes())?;
        Ok(())
    }
}

impl From<RegexSet> for IgnoreList {
    fn from(value: RegexSet) -> Self {
        Self { entries: value }
    }
}

impl Default for IgnoreList {
    fn default() -> Self {
        Self {
            entries: RegexSet::new(DEFAULT_IGNORELIST)
                .expect("Default ignorelist contains an error"),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ValidationError {
    #[error("Failed to parse object file")]
    ObjectError(
        #[source]
        #[from]
        object::Error,
    ),
    #[error("Failed to retrieve debug info")]
    GimliError(
        #[source]
        #[from]
        gimli::Error,
    ),
    #[error("Invalid regex in ignorelist")]
    InvalidRegex(
        #[source]
        #[from]
        regex::Error,
    ),

    #[error("File error")]
    FileError(
        #[source]
        #[from]
        FileError,
    ),
}

#[derive(Clone, Debug)]
pub enum SymbolClass {
    RustMangled {
        demangled: String,
        crate_name: String,
    },
    RustNonMangled {
        name: String,
        crate_name: String,
    },
    RustCrateLess,
    OtherLang,
    Ignored,
}

#[derive(Clone, Debug)]
struct ValidationSymbol {
    name: String,
    class: SymbolClass,
    section: String,
    address: u64,
    size: u64,
}

impl fmt::Display for ValidationSymbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)?;
        match &self.class {
            SymbolClass::RustMangled {
                demangled,
                crate_name,
            } => {
                write!(
                    f,
                    ": RustMangled{{\"{demangled}\", crate: \"{crate_name}\"}}"
                )?;
            }
            SymbolClass::RustNonMangled { name, crate_name } => {
                write!(f, ": RustNonMangled{{\"{name}\", crate: \"{crate_name}\"}}")?;
            }
            SymbolClass::OtherLang => write!(f, ": OtherLanguage")?,
            SymbolClass::RustCrateLess => write!(f, ": RustCrateLess")?,
            SymbolClass::Ignored => write!(f, ": Ignored")?,
        }
        write!(
            f,
            " at 0x{:02x} size 0x{:02x} in {}",
            self.address, self.size, self.section
        )?;
        Ok(())
    }
}

fn skip_until(chars: &mut Peekable<Chars>, char: char) -> Option<()> {
    while chars.next()? != char {}
    Some(())
}

fn read_number(chars: &mut Peekable<Chars>) -> Option<usize> {
    let mut num = String::new();
    loop {
        let char = chars.next()?;
        num.push(char);
        if !chars.peek()?.is_numeric() {
            break;
        }
    }
    num.parse().ok()
}

fn v0_extract(symbol: &str) -> Option<String> {
    let mut chars = symbol.chars().peekable();
    skip_until(&mut chars, 'C')?;
    skip_until(&mut chars, '_')?;
    let len = read_number(&mut chars)?;
    if chars.peek() == Some(&'_') {
        chars.next();
    }
    Some(chars.take(len).collect())
}

fn legacy_extract(symbol: &str) -> Option<String> {
    let mut chars = symbol.chars().peekable();
    skip_until(&mut chars, 'N')?;
    let len = read_number(&mut chars)?;
    let name: String = chars.take(len).collect();
    if name.starts_with('_') {
        Some(name.split("..").next()?.split("$").last()?.to_string())
    } else {
        Some(name)
    }
}

fn extract_crate_name(symbol: &str) -> Result<String, ValidationProblem> {
    ManglingVersion::from_mangling_string_prefix(symbol)
        .and_then(|scheme| match scheme {
            ManglingVersion::Legacy => legacy_extract(symbol),
            ManglingVersion::V0 => v0_extract(symbol),
        })
        .ok_or(ValidationProblem::NoCrateName {
            name: symbol.to_string(),
        })
}

fn classify_rust_symbol(
    name: &str,
    ignorelist: &IgnoreList,
    linkage_name: &str,
    alternative: Option<&str>,
) -> Result<SymbolClass, ValidationProblem> {
    if ignorelist.matches(name) {
        return Ok(SymbolClass::Ignored);
    }
    if linkage_name == name {
        return Ok(SymbolClass::RustNonMangled {
            name: name.to_string(),
            crate_name: alternative.map(ToString::to_string).ok_or_else(|| {
                ValidationProblem::NoCrateName {
                    name: name.to_string(),
                }
            })?,
        });
    }
    if let Ok(rust_demangled) = rustc_demangle::try_demangle(linkage_name) {
        let demangled = rust_demangled.to_string();
        let crate_name = match extract_crate_name(linkage_name) {
            Ok(name) => name.to_string(),
            Err(err) => {
                if let Some(namespace_root) = alternative {
                    namespace_root.to_string()
                } else {
                    return Err(err);
                }
            }
        };
        return Ok(SymbolClass::RustMangled {
            demangled,
            crate_name: crate_name.to_string(),
        });
    }
    Err(ValidationProblem::UnknownManglingScheme {
        name: name.to_string(),
        mangled_name: linkage_name.to_string(),
    })
}

fn classify_symbols(
    problems: &mut Vec<ValidationProblem>,
    ignorelist: &IgnoreList,
    obj: &object::File,
) -> Result<HashMap<String, SymbolClass>, ValidationError> {
    let dwarf = gimli::DwarfSections::load(|section| {
        let data = obj
            .section_by_name(section.name())
            .and_then(|s| s.data().ok())
            .unwrap_or(&[]);
        Ok::<_, gimli::Error>(Cow::Borrowed(data))
    })?;

    let endianness = match obj.endianness() {
        object::Endianness::Little => gimli::RunTimeEndian::Little,
        object::Endianness::Big => gimli::RunTimeEndian::Big,
    };
    let dwarf = dwarf.borrow(|section| gimli::EndianSlice::new(section, endianness));
    let mut units = dwarf.units();
    let mut res = HashMap::new();
    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;
        let mut entries = unit.entries();
        let root = match entries.next_dfs()? {
            Some(root) => root,
            None => {
                continue;
            }
        };
        let unit_is_rust = root
            .attr_value(gimli::DW_AT_language)
            .map(|lang| lang == gimli::AttributeValue::Language(gimli::DW_LANG_Rust))
            .unwrap_or_default();
        let compile_unit_root_name: Option<&str> =
            root.attr_value(gimli::DW_AT_name).and_then(|name| {
                dwarf.attr_string(&unit, name).ok().and_then(|attr| {
                    attr.to_string()
                        .ok()
                        .and_then(|name| name.split(".").next())
                })
            });
        let mut alternative_crate_name: Option<&str> = None;
        while let Some(entry) = entries.next_dfs()? {
            let tag = entry.tag();
            if unit_is_rust {
                if entry.depth() == 1 {
                    if tag == gimli::DW_TAG_namespace
                        && let Some(name) = entry.attr_value(gimli::DW_AT_name)
                    {
                        alternative_crate_name = dwarf
                            .attr_string(&unit, name)
                            .ok()
                            .and_then(|attr| attr.to_string().ok());
                        continue;
                    } else {
                        alternative_crate_name = compile_unit_root_name;
                    }
                }
            } else {
                alternative_crate_name = compile_unit_root_name;
            }

            if !(tag == gimli::DW_TAG_subprogram || tag == gimli::DW_TAG_variable) {
                continue;
            }

            let name = entry
                .attr_value(gimli::DW_AT_name)
                .and_then(|attr| dwarf.attr_string(&unit, attr).ok()?.to_string().ok());

            let linkage_name = entry
                .attr_value(gimli::DW_AT_linkage_name)
                .and_then(|attr| dwarf.attr_string(&unit, attr).ok()?.to_string().ok());

            if unit_is_rust {
                if let Some(name) = name {
                    if let Some(linkage_name) = linkage_name {
                        match classify_rust_symbol(
                            name,
                            ignorelist,
                            linkage_name,
                            alternative_crate_name,
                        ) {
                            Ok(class) => {
                                res.insert(linkage_name.to_owned(), class);
                            }
                            Err(problem) => problems.push(problem),
                        }
                    } else {
                        let crate_name = match alternative_crate_name {
                            Some(name) => name,
                            None => {
                                res.insert(name.to_string(), SymbolClass::RustCrateLess);
                                continue;
                            }
                        };
                        res.insert(
                            name.to_string(),
                            SymbolClass::RustNonMangled {
                                name: name.to_string(),
                                crate_name: crate_name.to_string(),
                            },
                        );
                    }
                }
            } else if let Some(name) = linkage_name.or(name) {
                res.insert(name.to_string(), SymbolClass::OtherLang);
            }
        }
    }
    Ok(res)
}

fn late_classify(name: &str, ignorelist: &IgnoreList) -> Result<SymbolClass, ValidationProblem> {
    if name.starts_with("_R") || name.starts_with("_ZN") {
        classify_rust_symbol("", ignorelist, name, None)
    } else {
        Err(ValidationProblem::ClassificationFailure {
            name: name.to_string(),
        })
    }
}

fn load_binary(
    problems: &mut Vec<ValidationProblem>,
    file: &Path,
    ignorelist: &IgnoreList,
) -> Result<Vec<ValidationSymbol>, ValidationError> {
    let binary_data = fs::read(file).into_in_result(file)?;
    let obj = object::File::parse(&*binary_data)?;
    let mut classifications = classify_symbols(problems, ignorelist, &obj)?;
    Ok(obj
        .symbols()
        .filter_map(|symbol| {
            let name = symbol.name().ok()?.to_string();
            let name = if name.starts_with(".L")
                && let Some(trimmed_name) = name.find("_").and_then(|pos| name.get(pos..))
            {
                trimmed_name
            } else {
                &name
            };
            if symbol.size() == 0 || ignorelist.matches(name) {
                problems.push(ValidationProblem::Ignored(name.to_string()));
                return None;
            }
            let class = match classifications.remove(name) {
                Some(SymbolClass::RustCrateLess) => {
                    problems.push(ValidationProblem::NoCrateName {
                        name: name.to_string(),
                    });
                    return None;
                }
                Some(class) => class,
                None => match late_classify(name, ignorelist) {
                    Ok(class) => class,
                    Err(problem) => {
                        problems.push(problem);
                        return None;
                    }
                },
            };
            let address = symbol.address();
            Some(ValidationSymbol {
                name: name.to_string(),
                class,
                section: symbol
                    .section()
                    .index()
                    .and_then(|index| {
                        obj.section_by_index(index)
                            .and_then(|section| section.name())
                            .ok()
                    })
                    .map(ToString::to_string)?,
                address,
                size: symbol.size(),
            })
        })
        .collect())
}

fn get_assignment_with_crate<'c, 'd>(
    symbol_name: &str,
    crate_name: &str,
    assignments: &'d DepTree,
    config: &'c Config,
) -> Result<Option<Assignment<'d, 'd, 'c>>, ValidationProblem> {
    let assigned_crate = assignments
        .get_crates()
        .values()
        .find(|dep| dep.name.replace("-", "_") == crate_name)
        .ok_or_else(|| ValidationProblem::NonExistentCrate {
            name: symbol_name.to_string(),
            crate_name: crate_name.to_string(),
        })?;
    let assigned_section = match &assigned_crate.assignment {
        Some(section) => section,
        None => {
            return Ok(None);
        }
    };
    Ok(Some(Assignment {
        section_name: &assigned_section.name,
        section: config.sections.get(&assigned_section.name).ok_or_else(|| {
            ValidationProblem::NonExistentSectionCrate {
                crate_name: assigned_crate.name.to_string(),
                section: assigned_section.name.to_string(),
            }
        })?,
        owner: &assigned_crate.name,
        owner_type: OwnerType::Crate,
    }))
}
#[derive(Debug)]
enum OwnerType {
    Crate,
    Custom,
}
impl fmt::Display for OwnerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OwnerType::Crate => write!(f, "crate"),
            OwnerType::Custom => write!(f, "custom pattern"),
        }
    }
}

#[derive(Debug)]
struct Assignment<'o, 'n, 's> {
    owner: &'o str,
    section_name: &'n str,
    section: &'s Section,
    owner_type: OwnerType,
}

fn check_custom_symbol_placement<'c>(
    symbol_name: &str,
    config: &'c Config,
) -> Result<Option<Assignment<'c, 'c, 'c>>, ValidationProblem> {
    if let Some(symbols) = &config.symbols {
        for (pattern_str, assignment) in symbols {
            let pattern = glob::Pattern::new(pattern_str).map_err(|err| {
                ValidationProblem::InvalidGlobPattern {
                    pattern: pattern_str.to_string(),
                    error: err,
                }
            })?;
            if pattern.matches(symbol_name) {
                return Ok(Some(Assignment {
                    section_name: &assignment.section,
                    section: (config.sections.get(&assignment.section).ok_or(
                        ValidationProblem::NonExistentSection {
                            symbol: symbol_name.to_string(),
                            section: assignment.section.to_string(),
                        },
                    )?),
                    owner: pattern_str,
                    owner_type: OwnerType::Custom,
                }));
            }
        }
    }
    Ok(None)
}

fn validate_placement(
    symbol: &ValidationSymbol,
    assignments: &DepTree,
    config: &Config,
) -> Result<(), ValidationProblem> {
    if let Some(name) = symbol.section.strip_prefix('.')
        && let Some(section) = config.noload_sections.get(name)
    {
        return validate_symbol_range(
            symbol,
            name,
            section.origin.as_bytes(),
            section.length.as_bytes(),
        );
    }

    let assignment = match check_custom_symbol_placement(&symbol.name, config)? {
        Some(assignment) => Some(assignment),
        None => match &symbol.class {
            SymbolClass::RustMangled {
                demangled: _,
                crate_name,
            } => get_assignment_with_crate(&symbol.name, crate_name, assignments, config)?,
            SymbolClass::RustNonMangled {
                name: _,
                crate_name,
            } => get_assignment_with_crate(&symbol.name, crate_name, assignments, config)?,
            SymbolClass::Ignored | SymbolClass::RustCrateLess => {
                return Err(ValidationProblem::Ignored(symbol.name.to_string()));
            }
            SymbolClass::OtherLang => {
                return Ok(());
            }
        },
    };
    let assignment = match assignment {
        Some(assigned_section) => assigned_section,
        None => return Ok(()),
    };

    let actual_section = symbol.section.trim_start_matches(".");

    if assignment.section_name != actual_section {
        return Err(ValidationProblem::SymbolAssignment {
            name: symbol.name.to_string(),
            correct_section: assignment.section_name.to_string(),
            actual_section: actual_section.to_string(),
            owner: format!("{}: \"{}\"", assignment.owner_type, assignment.owner),
        });
    }

    validate_symbol_range(
        symbol,
        assignment.section_name,
        assignment.section.origin.as_bytes(),
        assignment.section.length.as_bytes(),
    )
}

fn validate_symbol_range(
    symbol: &ValidationSymbol,
    section_name: &str,
    assigned_origin: u64,
    assigned_length: u64,
) -> Result<(), ValidationProblem> {
    let assigned_end = assigned_origin
        .checked_add(assigned_length)
        .ok_or_else(|| ValidationProblem::SectionOverflow {
            section: section_name.to_string(),
            start: assigned_origin,
            length: assigned_length,
        })?;

    if symbol.address < assigned_origin || symbol.address > assigned_end {
        return Err(ValidationProblem::SymbolPlacement {
            name: symbol.name.to_string(),
            section_name: section_name.to_string(),
            symbol_position: symbol.address,
            section_start: assigned_origin,
            section_end: assigned_end,
        });
    }

    let symbol_end = symbol.address.checked_add(symbol.size).ok_or_else(|| {
        ValidationProblem::SymbolOverflow {
            symbol: symbol.name.to_string(),
            start: symbol.address,
            length: symbol.size,
        }
    })?;

    if symbol_end > assigned_end {
        return Err(ValidationProblem::SymbolTooBig {
            name: symbol.name.to_string(),
            section_name: section_name.to_string(),
            symbol_start: symbol.address,
            symbol_end,
            section_end: assigned_end,
        });
    }
    Ok(())
}

pub(crate) fn validate(
    binary_file: &Path,
    assignments: &DepTree,
    config: &Config,
    ignorelist: &IgnoreList,
) -> Result<Vec<ValidationProblem>, ValidationError> {
    let mut problems = Vec::new();
    let binary = load_binary(&mut problems, binary_file, ignorelist)?;
    for symbol in &binary {
        if let Err(problem) = validate_placement(symbol, assignments, config) {
            problems.push(problem);
        }
    }
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ByteFormat, ByteUnit};
    use crate::deps::{Crate, SectionAssignment};
    use cargo_metadata::semver::Version;
    use std::collections::BTreeMap;

    fn noload_fixture() -> (Config, DepTree, ValidationSymbol) {
        let config: Config = toml::from_str(
            r#"
ram = { origin = "0x20000000", length = "636K" }
[sections.flash]
origin = "0x10000"
length = "512K"
[noload_sections.retained_nvram]
origin = "0x2009FFD0"
length = "48"
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let assignments = DepTree::from_crates(
            "my-crate-id".to_string(),
            BTreeMap::from([(
                "my-crate-id".to_string(),
                Crate {
                    name: "my-crate".to_string(),
                    version: Version::new(1, 0, 0),
                    dependencies: vec![],
                    assignment: Some(SectionAssignment {
                        name: "flash".to_string(),
                        priority: 0,
                        user_assigned: true,
                    }),
                },
            )]),
        );
        let symbol = ValidationSymbol {
            name: "_ZN8my_crate14RETAINED_NVRAME".to_string(),
            class: SymbolClass::RustMangled {
                demangled: "my_crate::RETAINED_NVRAM".to_string(),
                crate_name: "my_crate".to_string(),
            },
            section: ".retained_nvram".to_string(),
            address: 0x2009_FFD0,
            size: 48,
        };
        (config, assignments, symbol)
    }

    #[test]
    fn noload_static_overrides_its_crates_flash_assignment() {
        let (mut config, assignments, mut symbol) = noload_fixture();

        assert!(validate_placement(&symbol, &assignments, &config).is_ok());

        config.symbols = Some(HashMap::from([(
            symbol.name.clone(),
            toml::from_str("section = \"flash\"").unwrap(),
        )]));
        assert!(validate_placement(&symbol, &assignments, &config).is_ok());
        config.symbols = None;

        config.noload_sections.clear();
        assert!(matches!(
            validate_placement(&symbol, &assignments, &config),
            Err(ValidationProblem::SymbolAssignment {
                correct_section,
                actual_section,
                ..
            }) if correct_section == "flash" && actual_section == "retained_nvram"
        ));

        symbol.section = ".flash".to_string();
        symbol.address = 0x10000;
        assert!(validate_placement(&symbol, &assignments, &config).is_ok());
    }

    #[test]
    fn noload_symbols_must_fit_entirely_in_the_reservation() {
        let (config, assignments, mut symbol) = noload_fixture();
        let origin = symbol.address;

        for address in [origin - 1, origin + 49] {
            symbol.address = address;
            symbol.size = 1;
            assert!(matches!(
                validate_placement(&symbol, &assignments, &config),
                Err(ValidationProblem::SymbolPlacement { section_name, .. })
                    if section_name == "retained_nvram"
            ));
        }
        for (address, size) in [(origin + 47, 2), (origin + 48, 1)] {
            symbol.address = address;
            symbol.size = size;
            assert!(matches!(
                validate_placement(&symbol, &assignments, &config),
                Err(ValidationProblem::SymbolTooBig { section_name, .. })
                    if section_name == "retained_nvram"
            ));
        }
        for (address, size) in [(origin + 44, 4), (origin + 48, 0)] {
            symbol.address = address;
            symbol.size = size;
            assert!(validate_placement(&symbol, &assignments, &config).is_ok());
        }
    }

    #[test]
    fn noload_range_overflow_returns_contextual_errors() {
        let (mut config, assignments, mut symbol) = noload_fixture();
        let section = config.noload_sections.get_mut("retained_nvram").unwrap();
        section.origin = ByteUnit::new(u64::MAX - 7, ByteFormat::Bytes);
        section.length = ByteUnit::new(7, ByteFormat::Bytes);
        symbol.address = u64::MAX - 1;
        symbol.size = 2;

        assert!(matches!(
            validate_placement(&symbol, &assignments, &config),
            Err(ValidationProblem::SymbolOverflow { start, length, .. })
                if start == u64::MAX - 1 && length == 2
        ));

        config
            .noload_sections
            .get_mut("retained_nvram")
            .unwrap()
            .length = ByteUnit::new(8, ByteFormat::Bytes);
        assert!(matches!(
            validate_placement(&symbol, &assignments, &config),
            Err(ValidationProblem::SectionOverflow { section, start, length })
                if section == "retained_nvram" && start == u64::MAX - 7 && length == 8
        ));
    }

    #[test]
    fn extracts_crate_names_from_supported_mangling_schemes() {
        assert_eq!(extract_crate_name("_ZN8my_crate3fooE").unwrap(), "my_crate");
        assert_eq!(
            extract_crate_name("_RCabc_8my_crate3foo").unwrap(),
            "my_crate"
        );
    }

    #[test]
    fn malformed_mangled_name_reports_missing_crate_name() {
        let error = extract_crate_name("_Rmissing_markers").unwrap_err();

        assert!(matches!(
            error,
            ValidationProblem::NoCrateName { name } if name == "_Rmissing_markers"
        ));
    }

    #[test]
    fn classification_honors_ignorelist_before_mangling() {
        let ignorelist = IgnoreList::new(["^ignored$"]).unwrap();

        let class = classify_rust_symbol("ignored", &ignorelist, "not-mangled", None).unwrap();

        assert!(matches!(class, SymbolClass::Ignored));
    }

    #[test]
    fn non_mangled_symbol_uses_alternative_crate_name() {
        let ignorelist = IgnoreList::new(std::iter::empty::<&str>()).unwrap();

        let class =
            classify_rust_symbol("handler", &ignorelist, "handler", Some("my_crate")).unwrap();

        assert!(matches!(
            class,
            SymbolClass::RustNonMangled { name, crate_name }
                if name == "handler" && crate_name == "my_crate"
        ));
    }

    #[test]
    fn unknown_mangling_scheme_preserves_symbol_context() {
        let ignorelist = IgnoreList::new(std::iter::empty::<&str>()).unwrap();

        let error = classify_rust_symbol("handler", &ignorelist, "not-mangled", None).unwrap_err();

        assert!(matches!(
            error,
            ValidationProblem::UnknownManglingScheme { name, mangled_name }
                if name == "handler" && mangled_name == "not-mangled"
        ));
    }
}
