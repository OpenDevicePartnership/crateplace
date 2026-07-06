use core::fmt;
use object::Object;
use object::ObjectSection;
use object::ObjectSymbol;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::iter::Peekable;
use std::path::Path;
use std::str::Chars;

use crate::config;
use crate::config::Config;
use crate::config::Section;
use crate::deps::DepTree;
use crate::mangling;

#[derive(Debug, Clone)]
pub enum ProblemLevel {
    Warning,
    Error,
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationProblem {
    #[error(
        "Symbol too big: \"{name}\"start: {symbol_start:02x}, end: {symbol_end:02x}, section: \"{section_name}\" end: {section_end:02x}"
    )]
    SymbolTooBig {
        name: String,
        section_name: String,
        symbol_start: u64,
        symbol_end: u64,
        section_end: u64,
    },
    #[error(
        "Symbol placed incorrectly: \"{name}\" position: {symbol_position:02x}, section \"{section_name}\" start: {section_start:02x} end: {section_end:02x}"
    )]
    SymbolPlacement {
        name: String,
        section_name: String,
        symbol_position: u64,
        section_start: u64,
        section_end: u64,
    },
    #[error(
        "Symbol placed incorrectly: \"{name}\", belonging to {owner} section: \"{correct_section}\", actual section: \"{actual_section}\""
    )]
    SymbolAssignment {
        name: String,
        owner: String,
        correct_section: String,
        actual_section: String,
    },
    #[error("Unknown mangling scheme: \"{name}, mangled: \"{mangled_name}\"")]
    UnknownManglingScheme { name: String, mangled_name: String },
    #[error("Failed to identify crate of: \"{name}\"")]
    NoCrateName { name: String },
    #[error("Failed to classify symbol: \"{name}\"")]
    ClassificationFailure { name: String },
    #[error("Symbol \"{name}\" owned by non-existent crate: \"{crate_name}\"")]
    NonExistentCrate { name: String, crate_name: String },
    #[error("Crate \"{crate_name}\" assigned to non-existent section: \"{section}\"")]
    NonExistentSectionCrate { crate_name: String, section: String },
    #[error("Symbol \"{symbol}\" assigned to non-existent section: \"{section}\"")]
    NonExistentSection { symbol: String, section: String },
    #[error("Glob pattern is invalid: \"{pattern}\"")]
    InvalidGlobPattern {
        pattern: String,
        #[source]
        error: glob::PatternError,
    },
    #[error("Invalid number in config: \"{number}\"")]
    InvalidNumber { number: String },
    #[error("Overflow computing section end: {section} start: {start:02x} length: {length:02x}")]
    SectionOverflow {
        section: String,
        start: u64,
        length: u64,
    },
    #[error("Overflow computing symbol end: {symbol} start: {start:02x} length: {length:02x}")]
    SymbolOverflow {
        symbol: String,
        start: u64,
        length: u64,
    },
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
            ValidationProblem::InvalidNumber { .. } => ProblemLevel::Error,
            ValidationProblem::SectionOverflow { .. } => ProblemLevel::Error,
            ValidationProblem::SymbolOverflow { .. } => ProblemLevel::Error,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ValidationError {
    #[error("File error: {path}")]
    FileError {
        #[source]
        err: std::io::Error,
        path: String,
    },
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
}

trait IOToValidationError<T> {
    fn file_error(self, path: &Path) -> Result<T, ValidationError>;
}

impl<T> IOToValidationError<T> for Result<T, io::Error> {
    fn file_error(self, path: &Path) -> Result<T, ValidationError> {
        self.map_err(|err| ValidationError::FileError {
            err,
            path: path.to_string_lossy().to_string(),
        })
    }
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
    Defmt,
    Main,
    CortexMShenanigans,
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
            SymbolClass::Defmt => {
                write!(f, ": Defmt")?;
            }
            SymbolClass::Main => write!(f, ": Main")?,
            SymbolClass::OtherLang => write!(f, ": OtherLanguage")?,
            SymbolClass::RustCrateLess => write!(f, ": RustCrateLess")?,
            SymbolClass::CortexMShenanigans => write!(f, ": CortexMSpecial")?,
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
    mangling::ManglingVersion::from_mangling_string_prefix(symbol)
        .and_then(|scheme| match scheme {
            mangling::ManglingVersion::Legacy => legacy_extract(symbol),
            mangling::ManglingVersion::V0 => v0_extract(symbol),
        })
        .ok_or(ValidationProblem::NoCrateName {
            name: symbol.to_string(),
        })
}

fn classify_rust_symbol(
    name: &str,
    linkage_name: &str,
    namespace_root: Option<&str>,
) -> Result<SymbolClass, ValidationProblem> {
    if ignore(name) {
        return Ok(SymbolClass::Ignored);
    }
    if let Ok(rust_demangled) = rustc_demangle::try_demangle(linkage_name) {
        let demangled = rust_demangled.to_string();
        let crate_name = match extract_crate_name(linkage_name) {
            Ok(name) => name.to_string(),
            Err(err) => {
                if let Some(namespace_root) = namespace_root {
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
    if ((name == "DEFMT_LOG_STATEMENT" || name == "S")
        && linkage_name.contains("{\"package\":")
        && linkage_name.contains("\"tag\":"))
        || [
            "defmt_timestamp",
            "default_panic",
            "default_timestamp",
            "DEFMT_ENCODING",
            "DEFMT_VERSION",
            "DEFMT_VERSION",
        ]
        .contains(&name)
            && linkage_name.contains("_defmt")
    {
        return Ok(SymbolClass::Defmt);
    }
    if linkage_name == "main" {
        return Ok(SymbolClass::Main);
    }
    if name.starts_with("__cortex_m_rt") || name == "__ONCE__" {
        return Ok(SymbolClass::CortexMShenanigans);
    }
    Err(ValidationProblem::UnknownManglingScheme {
        name: name.to_string(),
        mangled_name: linkage_name.to_string(),
    })
}

fn classify_symbols(
    problems: &mut Vec<ValidationProblem>,
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
        let mut namespace_root: Option<&str> = None;
        while let Some(entry) = entries.next_dfs()? {
            let tag = entry.tag();
            if unit_is_rust {
                if entry.depth() == 1 {
                    if tag == gimli::DW_TAG_namespace
                        && let Some(name) = entry.attr_value(gimli::DW_AT_name)
                    {
                        namespace_root = dwarf
                            .attr_string(&unit, name)
                            .ok()
                            .and_then(|attr| attr.to_string().ok());
                        continue;
                    } else {
                        namespace_root = None;
                    }
                }
            } else {
                namespace_root = None;
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

            if let Some(name) = name {
                if unit_is_rust {
                    if let Some(linkage_name) = linkage_name {
                        match classify_rust_symbol(name, linkage_name, namespace_root) {
                            Ok(class) => {
                                res.insert(linkage_name.to_owned(), class);
                            }
                            Err(problem) => problems.push(problem),
                        }
                    } else {
                        let crate_name = match namespace_root {
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
                } else {
                    res.insert(name.to_string(), SymbolClass::OtherLang);
                }
            }
        }
    }
    Ok(res)
}

fn ignore(name: &str) -> bool {
    name.starts_with(".Lanon")
        || name.starts_with("__aeabi")
        || name.starts_with("__defmt")
        || name.starts_with("_defmt")
        || name.starts_with("_critical_section")
        || name == "Reset"
        || name == "__DEFMT_MARKER_TIMESTAMP_WAS_DEFINED"
        || name == "_MergedGlobals"
        || name == "__pre_init"
        || name == "pre_init"
}

fn late_classify(name: &str) -> Result<SymbolClass, ValidationProblem> {
    if name.starts_with("_R") || name.starts_with("_ZN") {
        classify_rust_symbol("", name, None)
    } else {
        Err(ValidationProblem::ClassificationFailure {
            name: name.to_string(),
        })
    }
}

fn load_binary(
    problems: &mut Vec<ValidationProblem>,
    file: &Path,
) -> Result<Vec<ValidationSymbol>, ValidationError> {
    let binary_data = fs::read(file).file_error(file)?;
    let obj = object::File::parse(&*binary_data)?;
    let mut classifications = classify_symbols(problems, &obj)?;
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
            if symbol.size() == 0 || ignore(name) {
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
                None => match late_classify(name) {
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
            SymbolClass::NotMangled
            | SymbolClass::RustCrateLess
            | SymbolClass::OtherLang
            | SymbolClass::Main
            | SymbolClass::Defmt
            | SymbolClass::Ignored
            | SymbolClass::CortexMShenanigans => return Ok(()),
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

    let assigned_origin = config::parse_offset(&assignment.section.origin).map_err(|_| {
        ValidationProblem::InvalidNumber {
            number: assignment.section.origin.to_string(),
        }
    })?;

    let assigned_length = config::parse_offset(&assignment.section.length).map_err(|_| {
        ValidationProblem::InvalidNumber {
            number: assignment.section.origin.to_string(),
        }
    })?;

    let assigned_end = assigned_origin
        .checked_add(assigned_length)
        .ok_or_else(|| ValidationProblem::SectionOverflow {
            section: assignment.section_name.to_string(),
            start: assigned_origin,
            length: assigned_length,
        })?;

    if symbol.address < assigned_origin || symbol.address > assigned_end {
        return Err(ValidationProblem::SymbolPlacement {
            name: symbol.name.to_string(),
            section_name: assignment.section_name.to_string(),
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
            section_name: assignment.section_name.to_string(),
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
) -> Result<Vec<ValidationProblem>, ValidationError> {
    let mut problems = Vec::new();
    let binary = load_binary(&mut problems, binary_file)?;
    for symbol in &binary {
        if let Err(problem) = validate_placement(symbol, assignments, config) {
            problems.push(problem);
        }
    }
    Ok(problems)
}
