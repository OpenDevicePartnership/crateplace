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
    let symbols = match &config.symbols {
        Some(symbols) => symbols,
        None => return None,
    };
    let text = symbols
        .iter()
        .filter(|(_, symbol)| symbol.section == section_name)
        .map(|(glob, symbol)| {
            let mut res = String::new();
            if symbol.symbol_types.text {
                res += &format!("        *(.text.{glob})\n");
            };
            if symbol.symbol_types.rodata {
                res += &format!("        *(.rodata.{glob})\n");
            };
            if symbol.symbol_types.datarel {
                res += &format!("        *(.data.rel.{glob})\n");
            };
            res
        })
        .collect::<Vec<_>>();
    if text.is_empty() {
        None
    } else {
        Some(text.join("        ") + "        ")
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
        {pre}
        MEMORY {{
            RAM : ORIGIN = {ram_origin}, LENGTH = {ram_len}
            {memory}
        }}

        SECTIONS {{{user_sections}{crate_sections}}} INSERT AFTER .text
        {post}
    "}
}
