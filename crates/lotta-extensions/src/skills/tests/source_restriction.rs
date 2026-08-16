use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "lotta-restrict-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(root: &Path, source: &str) {
    let path = root.join("same");
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("SKILL.md"),
        format!("---\nid: same\ndescription: {source}\n---\n"),
    )
    .unwrap();
}

#[test]
fn agent_global_excludes_project_bundled_and_preserves_precedence() {
    let temp = Temp::new();
    let project = temp.0.join("project");
    let agent = temp.0.join("agent");
    let global = temp.0.join("global");
    let bundled = temp.0.join("bundled");
    write(&project.join(".agents/skills"), "project");
    write(&agent, "agent");
    write(&global, "global");
    write(&bundled, "bundled");
    let roots = SkillRoots {
        project_working_root: project,
        agent_skills_directory: Some(agent),
        memory_root: None,
        global_skills_directory: global,
        bundled_skills_directory: bundled,
    };
    let selected = [SkillSource::Global, SkillSource::Agent, SkillSource::Global];
    let catalog = SkillDiscovery::discover(&roots, &selected).unwrap();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].source, SkillSource::Agent);
    assert_eq!(catalog[0].description, "agent");
    assert!(
        SkillDiscovery::discover(&roots, SkillSources::NONE)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn selected_catalog_item_maps_exactly_to_prompt_skill() {
    let temp = Temp::new();
    let project = temp.0.join("project");
    let root = project.join(".agents/skills");
    write(&root, "selected description");
    let roots = SkillRoots {
        project_working_root: project,
        agent_skills_directory: None,
        memory_root: None,
        global_skills_directory: temp.0.join("global"),
        bundled_skills_directory: temp.0.join("bundled"),
    };
    let catalog = SkillDiscovery::discover(&roots, &[SkillSource::Project]).unwrap();
    let selected = SkillDiscovery::select(&catalog, &["same".to_owned()]).unwrap();
    let selected = &selected[0];
    let prompt = lotta_memfs::PromptSkill::new(
        selected.id.clone(),
        selected.description.clone(),
        Some(selected.location.clone()),
    )
    .expect("prompt skill");
    assert_eq!(prompt.name().as_str(), selected.id);
    assert_eq!(prompt.description().as_str(), "selected description");
    assert_eq!(
        prompt.location().expect("location").as_str(),
        selected.location
    );
    assert_eq!(
        SkillDiscovery::select(&catalog, &["missing".to_owned()]),
        Err(SkillError::MissingSelection)
    );
    assert_eq!(
        SkillDiscovery::select(&catalog, &["same".to_owned(), "same".to_owned()]),
        Err(SkillError::DuplicateSelection)
    );
}
