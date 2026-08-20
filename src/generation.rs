use crate::{config::Config, deps::DepTree};
use indoc::formatdoc;

fn section_name_to_target(name: &str) -> String {
    name.replace("-", "").replace("_", "").to_uppercase()
}

#[derive(Debug, Copy, Clone)]
pub enum ManglingMatches {
    #[allow(dead_code)]
    Legacy,
    V0,
    All,
}

fn v0_matches(name: &str) -> Vec<String> {
    (0..26)
        .map(|num| format!("_R{}_{}{name}", "[a-zA-Z0-9_]".repeat(num), name.len()))
        .collect()
}

fn legacy_matches(name: &str) -> Vec<String> {
    let mut res = vec![format!("_ZN{}{name}", name.len())];
    res.extend((0..32).map(|num| format!("_ZN{}${name}", "[a-zA-Z0-9_$]".repeat(num))));
    res
}

fn generate_mangling_matches(name: &str, mangling: ManglingMatches) -> Vec<String> {
    match mangling {
        ManglingMatches::Legacy => legacy_matches(name),
        ManglingMatches::V0 => v0_matches(name),
        ManglingMatches::All => {
            let mut matches = legacy_matches(name);
            matches.append(&mut v0_matches(name));
            matches
        }
    }
}

fn generate_memory(config: &Config) -> String {
    config
        .sections
        .iter()
        .map(|(name, section)| {
            let name = section_name_to_target(name);
            let origin = &section.origin;
            let length = &section.length;
            formatdoc! {"
            {name} : ORIGIN = {origin}, LENGTH = {length}"}
        })
        .collect::<Vec<_>>()
        .join("\n    ")
}

fn generate_dep_matches(section_name: &str, deps: &DepTree, mangling: ManglingMatches) -> String {
    let res = deps
        .crates
        .values()
        .filter(|dep| {
            dep.assignment
                .as_ref()
                .map(|assignment| assignment.name == *section_name)
                .unwrap_or(false)
        })
        .map(|dep| {
            let dep_name = dep.name.replace("-", "_");
            let mangled = generate_mangling_matches(&dep_name, mangling);
            mangled
                .iter()
                .map(|mangled| {
                    formatdoc! {"
                        *(.text.{mangled}*)
                                *(.text.unlikely.{mangled}*)
                                *(.rodata.{mangled}*)
                                *(.data.rel.ro.{mangled}*)
                    "}
                })
                .collect::<Vec<String>>()
                .join("        ")
        })
        .collect::<Vec<_>>();
    if res.is_empty() {
        String::new()
    } else {
        res.join("        ") + "        "
    }
}

fn generate_symbol_matches(section_name: &str, config: &Config) -> Option<String> {
    let symbols = config.symbols.as_ref()?;
    let text = symbols
        .iter()
        .filter(|(_, symbol)| symbol.section == section_name)
        .map(|(glob, symbol)| {
            let mut res = String::new();
            if symbol.symbol_types.text {
                res += &format!("        *(.text.{glob})\n");
                res += &format!("        *(.text.unlikely.{glob})\n");
            };
            if symbol.symbol_types.rodata {
                res += &format!("        *(.rodata.{glob})\n");
            };
            if symbol.symbol_types.datarel {
                res += &format!("        *(.data.rel.ro.{glob})\n");
            };
            res
        })
        .collect::<Vec<_>>();
    if text.is_empty() {
        None
    } else {
        Some(text.join("") + "        ")
    }
}

fn generate_crate_sections(config: &Config, deps: &DepTree, mangling: ManglingMatches) -> String {
    let res = config
        .sections
        .keys()
        .map(|section_name| {
            let dep_matches = generate_dep_matches(section_name, deps, mangling);
            let section_target = section_name_to_target(section_name);
            formatdoc! {"
                .{section_name} : {{          
                        {dep_matches}. = ALIGN(4);
                    }} > {section_target}
            "}
        })
        .collect::<Vec<_>>();
    if !res.is_empty() {
        format!("\n    {}", res.join("\n    "))
    } else {
        String::new()
    }
}

fn generate_user_sections(config: &Config) -> String {
    let res = config
        .sections
        .keys()
        .filter_map(|section_name| {
            let symbol_matches = generate_symbol_matches(section_name, config);
            let section_target = section_name_to_target(section_name);
            symbol_matches.map(|symbol_matches| {
                formatdoc! {"
                .{section_name} : {{          
                {symbol_matches}. = ALIGN(4);
                    }} > {section_target}
            "}
            })
        })
        .collect::<Vec<_>>();
    if !res.is_empty() {
        format!("\n    {}", res.join("\n    "))
    } else {
        String::new()
    }
}

pub fn generate_script(
    config: &Config,
    deps: &DepTree,
    mangling: ManglingMatches,
    pre: Option<&str>,
    post: Option<&str>,
) -> String {
    let memory = generate_memory(config);
    let crate_sections = generate_crate_sections(config, deps, mangling);
    let user_sections = generate_user_sections(config);
    let ram_origin = &config.ram.origin;
    let ram_len = &config.ram.length;
    let mut pre_str = None;
    let pre = pre
        .map(|pre| pre_str.insert(format!("INCLUDE {pre}\n")).as_str())
        .unwrap_or("");
    let mut post_str = None;
    let post = post
        .map(|post| post_str.insert(format!("\nINCLUDE {post}")).as_str())
        .unwrap_or("");
    formatdoc! {"
        ### Generated by crateplace
        ### do not modify
        {pre}
        MEMORY {{
            RAM : ORIGIN = {ram_origin}, LENGTH = {ram_len}
            {memory}
        }}

        SECTIONS {{{user_sections}{crate_sections}}} INSERT AFTER .text
        {post}
    "}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{ByteFormat, ByteUnit, CratePlacement, Ram, Section, SymPlacement, SymbolTypes},
        deps::{Crate, SectionAssignment},
    };
    use cargo_metadata::semver::Version;
    use std::collections::{BTreeMap, HashMap};

    #[test]
    fn section_targets_remove_separators_and_use_uppercase() {
        assert_eq!(section_name_to_target("fast-flash_bank"), "FASTFLASHBANK");
    }

    #[test]
    fn mangling_modes_generate_the_expected_patterns() {
        let legacy = generate_mangling_matches("my_crate", ManglingMatches::Legacy);
        let v0 = generate_mangling_matches("my_crate", ManglingMatches::V0);
        let all = generate_mangling_matches("my_crate", ManglingMatches::All);

        assert_eq!(legacy.len(), 33);
        assert_eq!(legacy[0], "_ZN8my_crate");
        assert_eq!(v0.len(), 26);
        assert_eq!(v0[0], "_R_8my_crate");
        assert_eq!(all.len(), legacy.len() + v0.len());
        assert!(all.starts_with(&legacy));
        assert!(all.ends_with(&v0));
    }

    #[test]
    fn script_contains_includes_crate_matches_and_enabled_symbol_types() {
        let section_name = "fast-flash".to_string();
        let config = Config {
            ram: Ram {
                origin: ByteUnit::new(0x2000_0000, ByteFormat::Hex),
                length: ByteUnit::new(64 * 1024, ByteFormat::Kibi),
            },
            sections: HashMap::from([(
                section_name.clone(),
                Section {
                    origin: ByteUnit::new(0x0800_0000, ByteFormat::Hex),
                    length: ByteUnit::new(512 * 1024, ByteFormat::Kibi),
                    priority: 0,
                    default: false,
                },
            )]),
            crates: Some(HashMap::from([(
                "my-crate".to_string(),
                CratePlacement {
                    section: section_name.clone(),
                    include_dependencies: false,
                },
            )])),
            symbols: Some(HashMap::from([(
                "interrupt*".to_string(),
                SymPlacement {
                    section: section_name.clone(),
                    symbol_types: SymbolTypes {
                        text: true,
                        rodata: false,
                        datarel: false,
                    },
                },
            )])),
        };
        let deps = DepTree::from_crates(
            "my-crate-id".to_string(),
            BTreeMap::from([(
                "my-crate-id".to_string(),
                Crate {
                    name: "my-crate".to_string(),
                    version: Version::new(1, 0, 0),
                    dependencies: vec![],
                    assignment: Some(SectionAssignment {
                        name: section_name,
                        priority: 0,
                        user_assigned: true,
                    }),
                },
            )]),
        );

        let script = generate_script(
            &config,
            &deps,
            ManglingMatches::Legacy,
            Some("device.x"),
            Some("sections.x"),
        );

        assert!(script.contains("INCLUDE device.x\n\nMEMORY"));
        assert!(script.contains("RAM : ORIGIN = 0x20000000, LENGTH = 64K"));
        assert!(script.contains("FASTFLASH : ORIGIN = 0x8000000, LENGTH = 512K"));
        assert!(script.contains("*(.text._ZN8my_crate*)"));
        assert!(script.contains("*(.text.interrupt*)"));
        assert!(script.contains("*(.text.unlikely.interrupt*)"));
        assert!(!script.contains("*(.rodata.interrupt*)"));
        assert!(!script.contains("*(.data.rel.ro.interrupt*)"));
        assert!(script.ends_with("INCLUDE sections.x\n"));
    }
}
