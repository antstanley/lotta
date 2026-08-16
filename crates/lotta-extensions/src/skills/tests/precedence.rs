use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "lotta-skills-{}-{}",
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

fn roots(temp: &Temp) -> SkillRoots {
    SkillRoots {
        project_working_root: temp.0.join("project"),
        agent_skills_directory: Some(temp.0.join("agent")),
        memory_root: Some(temp.0.join("memory")),
        global_skills_directory: temp.0.join("global"),
        bundled_skills_directory: temp.0.join("bundled"),
    }
}
fn write(root: &Path, id: &str, description: &str) {
    let directory = root.join(id);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("SKILL.md"),
        format!("---\nid: {id}\ndescription: {description}\n---\nbody"),
    )
    .unwrap();
}
fn all() -> SkillSources {
    SkillSources::ALL
}

#[test]
fn project_wins_over_agent() {
    let temp = Temp::new();
    let roots = roots(&temp);
    write(
        &roots.project_working_root.join(".agents/skills"),
        "same",
        "project",
    );
    write(
        roots.agent_skills_directory.as_ref().unwrap(),
        "same",
        "agent",
    );
    let catalog = SkillDiscovery::discover(&roots, all()).unwrap();
    assert_eq!(catalog[0].source, SkillSource::Project);
}
#[test]
fn agent_wins_over_global() {
    let temp = Temp::new();
    let roots = roots(&temp);
    write(
        roots.agent_skills_directory.as_ref().unwrap(),
        "same",
        "agent",
    );
    write(&roots.global_skills_directory, "same", "global");
    assert_eq!(
        SkillDiscovery::discover(&roots, all()).unwrap()[0].source,
        SkillSource::Agent
    );
}
#[test]
fn global_wins_over_bundled() {
    let temp = Temp::new();
    let roots = roots(&temp);
    write(&roots.global_skills_directory, "same", "global");
    write(&roots.bundled_skills_directory, "same", "bundled");
    assert_eq!(
        SkillDiscovery::discover(&roots, all()).unwrap()[0].source,
        SkillSource::Global
    );
}
#[test]
fn legacy_dot_skills_fallback() {
    let temp = Temp::new();
    let roots = roots(&temp);
    write(
        &roots.project_working_root.join(".skills"),
        "legacy",
        "legacy",
    );
    assert_eq!(
        SkillDiscovery::discover(&roots, all()).unwrap()[0].id,
        "legacy"
    );
}
#[test]
fn memory_dir_fallback() {
    let temp = Temp::new();
    let roots = roots(&temp);
    write(
        &roots.memory_root.as_ref().unwrap().join("skills"),
        "memory",
        "memory",
    );
    assert_eq!(
        SkillDiscovery::discover(&roots, all()).unwrap()[0].id,
        "memory"
    );
}
#[test]
fn canonical_directories_suppress_fallbacks() {
    let temp = Temp::new();
    let roots = roots(&temp);
    fs::create_dir_all(roots.project_working_root.join(".agents/skills")).unwrap();
    fs::create_dir_all(roots.agent_skills_directory.as_ref().unwrap()).unwrap();
    write(
        &roots.project_working_root.join(".skills"),
        "legacy",
        "legacy",
    );
    write(
        &roots.memory_root.as_ref().unwrap().join("skills"),
        "memory",
        "memory",
    );
    assert!(SkillDiscovery::discover(&roots, all()).unwrap().is_empty());
}
#[test]
fn nested_id_and_missing_roots_are_deterministic() {
    let temp = Temp::new();
    let roots = roots(&temp);
    let root = roots.project_working_root.join(".agents/skills");
    fs::create_dir_all(root.join("web/scraper")).unwrap();
    fs::write(root.join("web/scraper/SKILL.md"), "body").unwrap();
    let catalog = SkillDiscovery::discover(&roots, all()).unwrap();
    assert_eq!(catalog[0].id, "web/scraper");
    assert_eq!(catalog[0].name, "Scraper");
}
