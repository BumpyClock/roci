use roci::skills::{ManagedSkillScope, ManagedSkillSourceKind, SkillManager, SkillSource};

use crate::cli::SkillsCommands;

pub async fn handle_skills(args: crate::cli::SkillsArgs) -> Result<(), Box<dyn std::error::Error>> {
    let manager = SkillManager::new();
    let cwd = std::env::current_dir()?;

    match args.command {
        SkillsCommands::Install(args) => {
            let scope = skill_scope(args.local);
            let result = manager.install(&cwd, scope, &args.source)?;
            println!(
                "Installed {} skill(s) in {} scope.",
                result.installed.len(),
                skill_scope_label(scope)
            );
            for record in result.installed {
                println!(
                    "{} [{}] {}",
                    record.name,
                    managed_source_kind_label(record.source.kind),
                    record.source.value
                );
            }
        }
        SkillsCommands::Remove(args) => {
            let scope = skill_scope(args.local);
            let result = manager.remove(&cwd, scope, &args.name)?;
            if let Some(removed) = result.removed {
                println!(
                    "Removed '{}' from {} scope.",
                    removed.name,
                    skill_scope_label(scope)
                );
            } else {
                println!(
                    "No managed skill named '{}' in {} scope.",
                    args.name,
                    skill_scope_label(scope)
                );
            }
        }
        SkillsCommands::Update(args) => {
            let scope = skill_scope(args.local);
            let result = manager.update(&cwd, scope, args.name.as_deref())?;
            println!(
                "Updated {} skill(s) in {} scope.",
                result.updated.len(),
                skill_scope_label(scope)
            );
            for record in result.updated {
                println!(
                    "{} [{}] {}",
                    record.name,
                    managed_source_kind_label(record.source.kind),
                    record.source.value
                );
            }
        }
        SkillsCommands::List => {
            let result = manager.list(&cwd)?;

            println!("Managed skills:");
            if result.managed.is_empty() {
                println!("(none)");
            } else {
                for item in result.managed {
                    let status = if item.exists_on_disk { "ok" } else { "missing" };
                    println!(
                        "{} [{}] {} [{}] {}",
                        item.record.name,
                        skill_scope_label(item.scope),
                        item.install_path.display(),
                        status,
                        item.record.source.value
                    );
                }
            }

            println!();
            println!("Discovered skills:");
            if result.discovered.is_empty() {
                println!("(none)");
            } else {
                for item in result.discovered {
                    let managed_state = if item.managed.is_some() {
                        "managed"
                    } else {
                        "unmanaged"
                    };
                    println!(
                        "{} [{}] {} {}",
                        item.skill.name,
                        managed_state,
                        skill_source_label(item.skill.source),
                        item.skill.file_path.display()
                    );
                }
            }

            for diagnostic in result.diagnostics {
                eprintln!(
                    "Warning: skill {}: {}",
                    diagnostic.path.display(),
                    diagnostic.message
                );
            }
        }
    }

    Ok(())
}

fn skill_scope(local: bool) -> ManagedSkillScope {
    if local {
        ManagedSkillScope::Project
    } else {
        ManagedSkillScope::Global
    }
}

fn skill_scope_label(scope: ManagedSkillScope) -> &'static str {
    match scope {
        ManagedSkillScope::Project => "project",
        ManagedSkillScope::Global => "global",
    }
}

fn managed_source_kind_label(kind: ManagedSkillSourceKind) -> &'static str {
    match kind {
        ManagedSkillSourceKind::LocalPath => "local",
        ManagedSkillSourceKind::GitUrl => "git",
    }
}

fn skill_source_label(source: SkillSource) -> &'static str {
    match source {
        SkillSource::Explicit => "explicit",
        SkillSource::ProjectRoci => "project-roci",
        SkillSource::ProjectAgents => "project-agents",
        SkillSource::GlobalRoci => "global-roci",
        SkillSource::GlobalAgents => "global-agents",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_scope_maps_local_to_project() {
        assert_eq!(skill_scope(true), ManagedSkillScope::Project);
        assert_eq!(skill_scope(false), ManagedSkillScope::Global);
    }

    #[test]
    fn scope_and_source_labels_match_existing_cli_output() {
        assert_eq!(skill_scope_label(ManagedSkillScope::Project), "project");
        assert_eq!(skill_scope_label(ManagedSkillScope::Global), "global");

        assert_eq!(
            managed_source_kind_label(ManagedSkillSourceKind::LocalPath),
            "local"
        );
        assert_eq!(
            managed_source_kind_label(ManagedSkillSourceKind::GitUrl),
            "git"
        );

        assert_eq!(skill_source_label(SkillSource::Explicit), "explicit");
        assert_eq!(skill_source_label(SkillSource::ProjectRoci), "project-roci");
        assert_eq!(
            skill_source_label(SkillSource::ProjectAgents),
            "project-agents"
        );
        assert_eq!(skill_source_label(SkillSource::GlobalRoci), "global-roci");
        assert_eq!(
            skill_source_label(SkillSource::GlobalAgents),
            "global-agents"
        );
    }
}
