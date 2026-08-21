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
    #[error("failed to retrieve dependencies from cargo")]
    CargoError(
        #[source]
        #[from]
        cargo_metadata::Error,
    ),
    #[error("no dependencies found")]
    NoDeps,
    #[error("missing root package")]
    NoRoot,
    #[error("failed to find crate \"{0}\" in dependencies")]
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

impl Crate {
    fn base_dep(name: String) -> Self {
        Crate {
            name,
            version: Version::new(0, 0, 0),
            dependencies: Vec::new(),
            assignment: None,
        }
    }
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

#[derive(Debug, Clone)]
struct FmtNode<'t> {
    node: &'t Crate,
    deps: Option<Vec<FmtNode<'t>>>,
}

impl<'t> FmtNode<'t> {
    fn fmt_node(&self, f: &mut fmt::Formatter<'_>, lines: &mut Vec<bool>) -> fmt::Result {
        fmt_lines(f, lines)?;
        fmt_dep(f, self.node, self.deps.is_none())?;
        if let Some(deps) = &self.deps {
            let mut iter = deps.iter().peekable();
            while let Some(dep) = iter.next() {
                lines.push(iter.peek().is_some());
                dep.fmt_node(f, lines)?;
                lines.pop();
            }
        }
        Ok(())
    }
}

struct FmtTree<'t> {
    root: FmtNode<'t>,
}

impl<'t> fmt::Display for FmtTree<'t> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut lines = Vec::new();
        self.root.fmt_node(f, &mut lines)?;
        Ok(())
    }
}

impl DepTree {
    #[cfg(test)]
    pub(crate) fn from_crates(root: String, crates: BTreeMap<String, Crate>) -> Self {
        Self {
            display_unspecified: false,
            no_dedupe: false,
            inverted: Inverted::Not,
            root,
            crates,
        }
    }

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

    fn construct_fmt_node<'t>(
        &'t self,
        id: &'t str,
        node: &'t Crate,
        seen: &mut HashSet<&'t str>,
    ) -> FmtNode<'t> {
        let deps = (self.no_dedupe || !seen.contains(id)).then(|| {
            self.get_node_deps(node, id)
                .iter()
                .filter_map(|id| Some((id, self.crates.get(*id)?)))
                .filter_map(|(id, dep)| {
                    if self.display_unspecified || self.any_deps_assigned_and_unseen(id, seen) {
                        Some(self.construct_fmt_node(id, dep, seen))
                    } else {
                        None
                    }
                })
                .collect()
        });
        seen.insert(id);
        FmtNode { node, deps }
    }

    fn construct_fmt_tree<'t>(&'t self) -> Option<FmtTree<'t>> {
        let mut seen = HashSet::<&str>::new();
        let root = match &self.inverted {
            Inverted::Not => &self.root,
            Inverted::Inverted(root) => root,
        };
        Some(FmtTree {
            root: self.construct_fmt_node(root, self.crates.get(root)?, &mut seen),
        })
    }

    fn get_node_deps<'t>(&'t self, node: &'t Crate, id: &str) -> Vec<&'t str> {
        match self.inverted {
            Inverted::Inverted(_) => self
                .crates
                .iter()
                .filter(|(_, node)| node.dependencies.iter().any(|dep| dep.id == id))
                .map(|(id, _)| id.as_str())
                .collect(),
            Inverted::Not => node
                .dependencies
                .iter()
                .filter(|dep| matches!(dep.kind, DepKind::Normal))
                .map(|dep| dep.id.as_str())
                .collect(),
        }
    }

    fn any_deps_assigned_and_unseen(&self, id: &str, seen: &HashSet<&str>) -> bool {
        if seen.contains(id) {
            return false;
        }
        self.crates
            .get(id)
            .map(|node| {
                node.assignment.is_some()
                    || self
                        .get_node_deps(node, id)
                        .iter()
                        .any(|id| self.any_deps_assigned_and_unseen(id, seen))
            })
            .unwrap_or(false)
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
        self.construct_fmt_tree().ok_or(fmt::Error)?.fmt(f)
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
        .ok_or(DepsError::NoRoot)?
        .id
        .repr
        .clone();
    let deps = meta.resolve.ok_or(DepsError::NoDeps)?;
    let mut res = deps
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

    res.extend(
        ["core", "std", "compiler_builtins", "__rustc"]
            .iter()
            .map(|name| (name.to_string(), Crate::base_dep(name.to_string()))),
    );

    Ok(DepTree {
        root,
        crates: res,
        display_unspecified: false,
        no_dedupe: false,
        inverted: Inverted::Not,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dependency(id: &str) -> Dep {
        Dep {
            id: id.to_string(),
            kind: DepKind::Normal,
        }
    }

    fn development_dependency(id: &str) -> Dep {
        Dep {
            id: id.to_string(),
            kind: DepKind::Dev,
        }
    }

    fn package(name: &str, dependencies: Vec<Dep>) -> Crate {
        Crate {
            name: name.to_string(),
            version: Version::new(1, 2, 3),
            dependencies,
            assignment: None,
        }
    }

    fn sample_tree() -> DepTree {
        DepTree::from_crates(
            "app".to_string(),
            BTreeMap::from([
                (
                    "app".to_string(),
                    package("app", vec![dependency("shared"), dependency("feature")]),
                ),
                (
                    "feature".to_string(),
                    package("feature", vec![dependency("shared")]),
                ),
                (
                    "shared".to_string(),
                    package("shared", vec![dependency("leaf")]),
                ),
                ("leaf".to_string(), package("leaf", Vec::new())),
            ]),
        )
    }

    #[test]
    fn discovers_dependencies_from_the_project_manifest() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");

        let tree = get_deps(Some(&manifest)).unwrap();
        let root = tree.get_crates().get(tree.get_root()).unwrap();

        assert_eq!(root.name, "crateplace");
        assert!(
            tree.get_crates()
                .values()
                .any(|package| package.name == "clap")
        );
        for built_in in ["core", "std", "compiler_builtins", "__rustc"] {
            assert!(tree.get_crates().contains_key(built_in));
        }
    }

    #[test]
    fn takes_a_dependency_by_package_name() {
        let mut tree = sample_tree();

        let (id, package) = tree.take_dep_by_name("feature").unwrap();

        assert_eq!(id, "feature");
        assert_eq!(package.name, "feature");
        assert!(tree.take_dep_by_name("missing").is_none());
    }

    #[test]
    fn formats_assigned_dependency_paths_and_deduplicates_repeats() {
        let mut tree = sample_tree();
        tree.crates.get_mut("feature").unwrap().assignment = Some(SectionAssignment {
            name: "fast".to_string(),
            priority: 7,
            user_assigned: true,
        });
        tree.crates.get_mut("shared").unwrap().assignment = Some(SectionAssignment {
            name: "default".to_string(),
            priority: 0,
            user_assigned: false,
        });
        tree.display_unspecified(true);

        let output = tree.to_string();

        assert!(output.contains("app v1.2.3"));
        assert!(output.contains("feature v1.2.3"));
        assert!(output.contains("fast"));
        assert!(output.contains("prio:7"));
        assert!(output.contains("shared v1.2.3"));
        assert!(output.contains("default"));
        assert!(output.contains("(*)"));
        assert_eq!(output.matches("leaf v1.2.3").count(), 1);
    }

    #[test]
    fn display_options_include_unspecified_and_repeated_dependencies() {
        let mut tree = sample_tree();
        tree.display_unspecified(true);
        tree.no_dedupe(true);

        let output = tree.to_string();

        assert!(output.contains("unspecified"));
        assert_eq!(output.matches("shared v1.2.3").count(), 2);
        assert_eq!(output.matches("leaf v1.2.3").count(), 2);
    }

    #[test]
    fn filters_development_and_unassigned_dependencies() {
        let mut tree = DepTree::from_crates(
            "app".to_string(),
            BTreeMap::from([
                (
                    "app".to_string(),
                    package(
                        "app",
                        vec![
                            dependency("assigned"),
                            dependency("unassigned"),
                            development_dependency("development"),
                        ],
                    ),
                ),
                ("assigned".to_string(), package("assigned", Vec::new())),
                (
                    "development".to_string(),
                    package("development", Vec::new()),
                ),
                ("unassigned".to_string(), package("unassigned", Vec::new())),
            ]),
        );
        tree.crates.get_mut("assigned").unwrap().assignment = Some(SectionAssignment {
            name: "flash".to_string(),
            priority: 1,
            user_assigned: true,
        });
        tree.crates.get_mut("development").unwrap().assignment = Some(SectionAssignment {
            name: "flash".to_string(),
            priority: 2,
            user_assigned: true,
        });

        let output = tree.to_string();

        assert!(output.contains("assigned v1.2.3"));
        assert!(!output.contains("unassigned v1.2.3"));
        assert!(!output.contains("development v1.2.3"));
    }

    #[test]
    fn inverted_tree_shows_reverse_dependencies() {
        let mut tree = sample_tree();
        tree.display_unspecified(true);
        tree.inverted(Inverted::Inverted("shared".to_string()));

        let output = tree.to_string();
        let lines = output.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("shared v1.2.3"));
        assert!(lines[1].contains("├── ") && lines[1].contains("app v1.2.3"));
        assert!(lines[2].contains("└── ") && lines[2].contains("feature v1.2.3"));
        assert!(lines[3].contains("    ") && lines[3].contains("└── "));
        assert!(lines[3].contains("app v1.2.3"));
        assert!(lines[3].contains("(*)"));
    }

    #[test]
    fn missing_display_root_returns_a_formatting_error() {
        let mut tree = sample_tree();
        tree.inverted(Inverted::Inverted("missing".to_string()));

        let mut output = String::new();
        let result = fmt::write(&mut output, format_args!("{tree}"));

        assert!(result.is_err());
        assert!(output.is_empty());
    }
}
