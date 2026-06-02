use crate::{
    config::Config,
    deps::{DepKind, DepTree, SectionAssignment},
};

#[derive(Debug, Clone, thiserror::Error)]
pub enum AssignmentError {
    #[error("Failed to find {0} in dependencies")]
    CrateNotFound(String),
    #[error("Section not defined in [sections]: \"{section_name}\", assigned to \"{crate_name}\"")]
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
        for (_, node) in deps.crates.iter_mut() {
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
            dep_assignment.user_assigned = false;
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
