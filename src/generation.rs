use crate::{config::Config, deps::DepTree};
use indoc::formatdoc;

fn section_name_to_target(name: &str) -> String {
    name.replace("-", "").replace("_", "").to_uppercase()
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
        + "\n"
}

fn generate_dep_matches(section_name: &str, deps: &DepTree) -> String {
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
            let dep_len = dep.name.len();
            formatdoc! {"
                        *(.text._ZN{dep_len}{dep_name}*)
                                *(.rodata._ZN{dep_len}{dep_name}*)
                                *(.data.rel.ro._ZN{dep_len}{dep_name}*)
                    "}
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
                res += &format!("*(.text.{glob})");
            };
            if symbol.symbol_types.rodata {
                res += &format!("*(.rodata.{glob})");
            };
            if symbol.symbol_types.datarel {
                res += &format!("*(.data.rel.{glob})");
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

fn generate_crate_sections(config: &Config, deps: &DepTree) -> String {
    let res = config
        .sections
        .keys()
        .map(|section_name| {
            let dep_matches = generate_dep_matches(section_name, deps);
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

pub fn generate_script(config: &Config, deps: &DepTree) -> String {
    let memory = generate_memory(config);
    let crate_sections = generate_crate_sections(config, deps);
    let user_sections = generate_user_sections(config);
    let ram_origin = &config.ram.origin;
    let ram_len = &config.ram.length;
    formatdoc! {"
        MEMORY {{
            RAM : ORIGIN = {ram_origin}, LENGTH = {ram_len}
            {memory}}}

        SECTIONS {{{user_sections}{crate_sections}}} INSERT AFTER .text
    "}
}
