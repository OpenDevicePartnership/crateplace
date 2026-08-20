use crate::{
    config::Config,
    deps::{DepKind, DepTree, SectionAssignment},
};

#[derive(Debug, Clone, thiserror::Error)]
pub enum AssignmentError {
    #[error("failed to find {0} in dependencies")]
    CrateNotFound(String),
    #[error("section not defined in [sections]: \"{section_name}\", assigned to \"{crate_name}\"")]
    SectionMissing {
        crate_name: String,
        section_name: String,
    },
}

fn apply_default(config: &Config, deps: &mut DepTree) {
    if let Some((name, section)) = config
        .sections
        .iter()
        .find_map(|(name, section)| section.default.then_some((name, section.clone())))
    {
        for node in deps.crates.values_mut() {
            if node.assignment.is_none() {
                node.assignment = Some(SectionAssignment {
                    name: name.clone(),
                    priority: section.priority,
                    user_assigned: false,
                })
            }
        }
    }
}

fn try_assign(
    crate_dep: &mut crate::deps::Crate,
    name: String,
    priority: u32,
    user_assigned: bool,
) {
    if let Some(dep_assignment) = crate_dep.assignment.as_mut() {
        if dep_assignment.priority > priority {
            dep_assignment.name = name;
            dep_assignment.priority = priority;
            dep_assignment.user_assigned = user_assigned;
        }
    } else {
        crate_dep.assignment = Some(SectionAssignment {
            name,
            priority,
            user_assigned,
        })
    }
}

fn assign_subtree(
    section_name: &str,
    priority: u32,
    dep: &crate::deps::Crate,
    deps: &mut DepTree,
) -> Result<(), AssignmentError> {
    let mut to_assign = dep.dependencies.clone();
    while let Some(crate_dep) = to_assign.pop() {
        if crate_dep.kind == DepKind::Dev {
            continue;
        }
        let crate_dep = deps
            .crates
            .get_mut(&crate_dep.id)
            .ok_or_else(|| AssignmentError::CrateNotFound(crate_dep.id.clone()))?;
        try_assign(crate_dep, section_name.to_string(), priority, false);
        to_assign.extend_from_slice(&crate_dep.dependencies);
    }
    Ok(())
}

pub fn assign(config: &Config, deps: &mut DepTree) -> Result<(), AssignmentError> {
    let crates = match &config.crates {
        Some(crates) => crates,
        None => {
            return Ok(());
        }
    };

    for (name, crate_config) in crates.iter() {
        let (dep_id, mut dep) = deps
            .take_dep_by_name(name)
            .ok_or_else(|| AssignmentError::CrateNotFound(name.clone()))?;
        let section =
            config
                .sections
                .get(&crate_config.section)
                .ok_or(AssignmentError::SectionMissing {
                    crate_name: name.clone(),
                    section_name: crate_config.section.clone(),
                })?;

        try_assign(
            &mut dep,
            crate_config.section.clone(),
            section.priority,
            true,
        );

        if crate_config.include_dependencies {
            assign_subtree(&crate_config.section, section.priority, &dep, deps)?;
        }
        deps.crates.insert(dep_id.clone(), dep);
    }
    apply_default(config, deps);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{ByteFormat, ByteUnit, CratePlacement, Ram, Section},
        deps::{Crate, Dep},
    };
    use cargo_metadata::semver::Version;
    use std::collections::{BTreeMap, HashMap};

    fn section(priority: u32, default: bool) -> Section {
        Section {
            origin: ByteUnit::new(0, ByteFormat::Bytes),
            length: ByteUnit::new(1, ByteFormat::Bytes),
            priority,
            default,
        }
    }

    fn config(
        sections: impl IntoIterator<Item = (&'static str, Section)>,
        crates: impl IntoIterator<Item = (&'static str, CratePlacement)>,
    ) -> Config {
        Config {
            ram: Ram {
                origin: ByteUnit::new(0, ByteFormat::Bytes),
                length: ByteUnit::new(1, ByteFormat::Bytes),
            },
            sections: sections
                .into_iter()
                .map(|(name, section)| (name.to_string(), section))
                .collect::<HashMap<_, _>>(),
            crates: Some(
                crates
                    .into_iter()
                    .map(|(name, placement)| (name.to_string(), placement))
                    .collect(),
            ),
            symbols: None,
        }
    }

    fn crate_node(name: &str, dependencies: Vec<Dep>) -> Crate {
        Crate {
            name: name.to_string(),
            version: Version::new(1, 0, 0),
            dependencies,
            assignment: None,
        }
    }

    fn dep_tree(crates: impl IntoIterator<Item = (&'static str, Crate)>) -> DepTree {
        DepTree::from_crates(
            "app-id".to_string(),
            crates
                .into_iter()
                .map(|(id, node)| (id.to_string(), node))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    #[test]
    fn missing_crate_returns_contextual_error() {
        let config = config(
            [("flash", section(0, false))],
            [(
                "missing",
                CratePlacement {
                    section: "flash".to_string(),
                    include_dependencies: false,
                },
            )],
        );
        let mut deps = dep_tree([]);

        let error = assign(&config, &mut deps).unwrap_err();

        assert!(matches!(
            error,
            AssignmentError::CrateNotFound(name) if name == "missing"
        ));
    }

    #[test]
    fn missing_section_returns_crate_and_section_names() {
        let config = config(
            [],
            [(
                "app",
                CratePlacement {
                    section: "missing".to_string(),
                    include_dependencies: false,
                },
            )],
        );
        let mut deps = dep_tree([("app-id", crate_node("app", vec![]))]);

        let error = assign(&config, &mut deps).unwrap_err();

        assert!(matches!(
            error,
            AssignmentError::SectionMissing { crate_name, section_name }
                if crate_name == "app" && section_name == "missing"
        ));
    }

    #[test]
    fn assigns_dependencies_by_priority_and_defaults_unassigned_crates() {
        let config = config(
            [
                ("fast", section(1, false)),
                ("slow", section(10, false)),
                ("fallback", section(100, true)),
            ],
            [
                (
                    "app",
                    CratePlacement {
                        section: "slow".to_string(),
                        include_dependencies: true,
                    },
                ),
                (
                    "leaf",
                    CratePlacement {
                        section: "fast".to_string(),
                        include_dependencies: false,
                    },
                ),
            ],
        );
        let mut deps = dep_tree([
            (
                "app-id",
                crate_node(
                    "app",
                    vec![
                        Dep {
                            id: "shared-id".to_string(),
                            kind: DepKind::Normal,
                        },
                        Dep {
                            id: "dev-id".to_string(),
                            kind: DepKind::Dev,
                        },
                    ],
                ),
            ),
            (
                "shared-id",
                crate_node(
                    "shared",
                    vec![Dep {
                        id: "leaf-id".to_string(),
                        kind: DepKind::Normal,
                    }],
                ),
            ),
            ("leaf-id", crate_node("leaf", vec![])),
            ("dev-id", crate_node("dev-only", vec![])),
            ("other-id", crate_node("other", vec![])),
        ]);

        assign(&config, &mut deps).unwrap();

        let crates = deps.get_crates();
        let app = crates["app-id"].assignment.as_ref().unwrap();
        assert_eq!(app.name, "slow");
        assert_eq!(app.priority, 10);
        assert!(app.user_assigned);

        let shared = crates["shared-id"].assignment.as_ref().unwrap();
        assert_eq!(shared.name, "slow");
        assert!(!shared.user_assigned);

        let leaf = crates["leaf-id"].assignment.as_ref().unwrap();
        assert_eq!(leaf.name, "fast");
        assert_eq!(leaf.priority, 1);
        assert!(leaf.user_assigned);

        for id in ["dev-id", "other-id"] {
            let assignment = crates[id].assignment.as_ref().unwrap();
            assert_eq!(assignment.name, "fallback");
            assert_eq!(assignment.priority, 100);
            assert!(!assignment.user_assigned);
        }
    }
}
