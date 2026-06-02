use cargo_metadata::{DependencyKind, MetadataCommand, semver::Version};
use cargo_metadata::{NodeDep, Package, TargetKind};
use clap::builder::styling::{AnsiColor, Color, Style};
use std::fmt;
use std::fmt::Debug;
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

const DIM: Style = Style::new().dimmed();
const STAR: Style = Style::new()
    .dimmed()
    .fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
const ASSIGN: Style = Style::new()
    .dimmed()
    .fg_color(Some(Color::Ansi(AnsiColor::Yellow)));

const BOLD: Style = Style::new().bold();

#[derive(thiserror::Error, Debug)]
pub enum DepsError {
    #[error("Failed to retrieve dependencies from cargo")]
    CargoError(
        #[source]
        #[from]
        cargo_metadata::Error,
    ),
    #[error("No dependencies found")]
    NoDeps,
    #[error("Missing root package")]
    Noroot,
    #[error("Missing root package")]
    CrateNotFound(String),
}

#[derive(Clone, Debug)]
pub struct SectionAssignment {
    pub name: String,
    pub priority: u32,
    pub user_assigned: bool,
}

#[derive(Clone, Debug, Ord, PartialEq, PartialOrd, Eq, Default)]
pub enum DepKind {
    #[default]
    Normal,
    Dev,
}

#[derive(Clone, Debug, Ord, PartialEq, PartialOrd, Eq)]
pub struct Dep {
    pub id: String,
    pub kind: DepKind,
}

#[derive(Clone, Debug)]
pub struct Crate {
    pub name: String,
    pub version: Version,
    pub dependencies: Vec<Dep>,
    pub assignment: Option<SectionAssignment>,
}

#[derive(Clone, Debug)]
pub enum Inverted {
    Not,
    Inverted(String),
}

#[derive(Debug, Clone)]
pub struct DepTree {
    display_unspecified: bool,
    no_dedupe: bool,
    inverted: Inverted,
    pub(crate) root: String,
    pub(crate) crates: BTreeMap<String, Crate>,
}

impl DepTree {
    pub fn take_dep_by_name(&mut self, name: &str) -> Option<(String, Crate)> {
        let (id, _) = self.crates.iter().find(|(_, dep)| dep.name == name)?;
        let id = id.clone();
        self.crates.remove_entry(&id)
    }

    pub fn get_root(&self) -> &str {
        self.root.as_str()
    }

    pub fn display_unspecified(&mut self, display_unspecified: bool) {
        self.display_unspecified = display_unspecified;
    }

    pub fn no_dedupe(&mut self, dedupe: bool) {
        self.no_dedupe = dedupe;
    }

    pub fn inverted(&mut self, inverted: Inverted) {
        self.inverted = inverted;
    }

    pub fn get_crates(&self) -> &BTreeMap<String, Crate> {
        &self.crates
    }

    fn fmt_deptree(
        &self,
        f: &mut fmt::Formatter<'_>,
        dep_id: &str,
        drawn: &mut HashSet<String>,
        lines: &mut Vec<bool>,
    ) -> fmt::Result {
        let dep = self.crates.get(dep_id).ok_or(fmt::Error)?;
        fmt_lines(f, lines)?;
        if (!drawn.contains(dep_id)) || self.no_dedupe {
            fmt_dep(f, dep, false)?;
            let mut dep_iter = dep
                .dependencies
                .iter()
                .filter(|dep| {
                    self.display_unspecified
                        || self
                            .crates
                            .get(&dep.id)
                            .map(|node| node.assignment.is_some())
                            .unwrap_or(false)
                })
                .peekable();
            while let Some(dep) = dep_iter.next() {
                lines.push(dep_iter.peek().is_some());
                self.fmt_deptree(f, &dep.id, drawn, lines)?;
                lines.pop();
            }
            drawn.insert(dep_id.to_owned());
        } else {
            fmt_dep(f, dep, true)?;
        }
        Ok(())
    }

    fn find_dependents(&self, id: &str) -> Vec<String> {
        self.crates
            .iter()
            .filter(|(_, node)| node.dependencies.iter().any(|dep| dep.id == id))
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn fmt_inverted_deptree(
        &self,
        f: &mut fmt::Formatter<'_>,
        dep_id: &str,
        drawn: &mut HashSet<String>,
        lines: &mut Vec<bool>,
    ) -> fmt::Result {
        let dep = self.crates.get(dep_id).ok_or(fmt::Error)?;
        fmt_lines(f, lines)?;
        if (!drawn.contains(dep_id)) || self.no_dedupe {
            fmt_dep(f, dep, false)?;
            let dependents = self.find_dependents(dep_id);
            let mut dep_iter = dependents
                .iter()
                .filter(|id| {
                    self.display_unspecified
                        || self
                            .crates
                            .get(id.as_str())
                            .map(|node| node.assignment.is_some())
                            .unwrap_or(false)
                })
                .peekable();
            while let Some(id) = dep_iter.next() {
                lines.push(dep_iter.peek().is_some());
                self.fmt_inverted_deptree(f, id, drawn, lines)?;
                lines.pop();
            }
            drawn.insert(dep_id.to_owned());
        } else {
            fmt_dep(f, dep, true)?;
        }
        Ok(())
    }
}

fn fmt_dep(f: &mut fmt::Formatter<'_>, dep: &Crate, star: bool) -> fmt::Result {
    write!(
        f,
        "{} v{}.{}.{}",
        dep.name, dep.version.major, dep.version.minor, dep.version.patch
    )?;
    if star {
        write!(f, " {STAR}(*){STAR:#}")?;
    }
    write!(f, " →")?;
    match &dep.assignment {
        Some(assignment) => {
            if assignment.user_assigned {
                writeln!(
                    f,
                    " {ASSIGN}{BOLD}{}{BOLD:#} {ASSIGN}prio:{}{ASSIGN:#} ",
                    assignment.name, assignment.priority
                )
            } else {
                writeln!(f, " {BOLD}{}{BOLD:#}", assignment.name)
            }
        }
        None => writeln!(f, " {DIM}unspecified{DIM:#}"),
    }
}

fn fmt_lines(f: &mut fmt::Formatter<'_>, lines: &[bool]) -> fmt::Result {
    if lines.is_empty() {
        return Ok(());
    }
    let len = lines.len();
    for pre in lines.iter().take(len - 1) {
        if *pre {
            write!(f, "{DIM}│   {DIM:#}")?;
        } else {
            write!(f, "    ")?;
        }
    }
    if lines[len - 1] {
        write!(f, "{DIM}├── {DIM:#}")?;
    } else {
        write!(f, "{DIM}└── {DIM:#}")?;
    }
    Ok(())
}

impl fmt::Display for DepTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut lines = Vec::new();
        let mut drawn = HashSet::new();
        match &self.inverted {
            Inverted::Not => self.fmt_deptree(f, &self.root, &mut drawn, &mut lines),
            Inverted::Inverted(root) => self.fmt_inverted_deptree(f, root, &mut drawn, &mut lines),
        }
    }
}

fn get_depkind(dep: &NodeDep, packages: &[Package], package: &Package) -> DepKind {
    if let Some(package_dep) = package
        .dependencies
        .iter()
        .find(|p_dep| p_dep.name == dep.name)
        && matches!(
            package_dep.kind,
            DependencyKind::Development | DependencyKind::Build
        )
    {
        return DepKind::Dev;
    }
    if let Some(dep_package) = packages.iter().find(|pck| pck.id == dep.pkg)
        && dep_package
            .targets
            .iter()
            .any(|target| target.kind.contains(&TargetKind::ProcMacro))
    {
        return DepKind::Dev;
    }
    DepKind::Normal
}

pub fn get_deps(manifest_path: Option<&Path>) -> Result<DepTree, DepsError> {
    let mut command = MetadataCommand::new();
    if let Some(manifest_path) = manifest_path {
        command.manifest_path(manifest_path);
    }
    let meta = command.exec()?;
    let root = meta
        .root_package()
        .ok_or(DepsError::Noroot)?
        .id
        .repr
        .clone();
    let deps = meta.resolve.ok_or(DepsError::NoDeps)?;
    let res = deps
        .nodes
        .iter()
        .map(|node| -> Result<(String, Crate), DepsError> {
            let package = meta
                .packages
                .iter()
                .find(|package| package.id.repr == node.id.repr)
                .ok_or(DepsError::CrateNotFound(node.id.repr.clone()))?;
            Ok((
                node.id.repr.clone(),
                Crate {
                    dependencies: node
                        .deps
                        .iter()
                        .map(|dep| Dep {
                            id: dep.pkg.repr.clone(),
                            kind: get_depkind(dep, &meta.packages, package),
                        })
                        .collect(),
                    assignment: None,
                    name: package.name.to_string(),
                    version: package.version.clone(),
                },
            ))
        })
        .collect::<Result<BTreeMap<String, Crate>, DepsError>>()?;
    Ok(DepTree {
        root,
        crates: res,
        display_unspecified: false,
        no_dedupe: false,
        inverted: Inverted::Not,
    })
}
