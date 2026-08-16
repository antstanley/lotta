use super::input::{PromptSkill, normalize_path, validate_normalized_path, validate_tree_name};
use super::util::{BoundedString, invalid, reserve_one};
use lotta_runtime::RuntimeError;
use std::collections::{BTreeMap, BTreeSet};

const MEMORY_DIR_PLACEHOLDER: &str = "${MEMORY_DIR}";

struct SkillEntry<'a> {
    relative: String,
    description: &'a str,
}

pub(crate) fn render_skills(skills: &[PromptSkill]) -> Result<String, RuntimeError> {
    if skills.is_empty() {
        return Ok(String::new());
    }
    let mut seen = BTreeSet::new();
    let mut grouped: BTreeMap<String, Vec<SkillEntry<'_>>> = BTreeMap::new();
    for skill in skills {
        if !seen.insert(skill.name().as_str()) {
            continue;
        }
        let location = match skill.location() {
            Some(value) => value.as_str(),
            None => "",
        };
        let (root, relative) = skill_path(skill.name().as_str(), location)?;
        let description = match skill.description().as_str().lines().next() {
            Some(value) => value.trim(),
            None => "",
        };
        let entry = SkillEntry {
            relative,
            description,
        };
        let values = grouped.entry(root).or_default();
        reserve_one(values, "prompt skill entries")?;
        values.push(entry);
    }
    let mut out = BoundedString::default();
    out.push("<available_skills>\n")?;
    let roots = grouped.len();
    for (root_index, (root, mut entries)) in grouped.into_iter().enumerate() {
        out.push(&root)?;
        out.push("\n")?;
        entries.sort_by(|left, right| left.relative.cmp(&right.relative));
        let tree = skill_tree(&entries)?;
        render_skill_node(&tree, "", &mut out)?;
        if root_index + 1 != roots {
            out.push("\n")?;
        }
    }
    out.push("</available_skills>")?;
    Ok(out.finish())
}

#[derive(Default)]
struct SkillNode<'a> {
    children: BTreeMap<String, SkillNode<'a>>,
    description: Option<&'a str>,
}

fn skill_tree<'a>(entries: &'a [SkillEntry<'a>]) -> Result<SkillNode<'a>, RuntimeError> {
    let mut root = SkillNode::default();
    for entry in entries {
        let mut node = &mut root;
        for part in entry.relative.split('/') {
            validate_tree_name(part)?;
            node = node.children.entry(part.to_owned()).or_default();
        }
        node.description = Some(entry.description);
    }
    Ok(root)
}

fn render_skill_node(
    node: &SkillNode<'_>,
    prefix: &str,
    out: &mut BoundedString,
) -> Result<(), RuntimeError> {
    let entries = node.children.iter();
    let last = node.children.len().checked_sub(1);
    for (index, (name, child)) in entries.enumerate() {
        let final_entry = Some(index) == last;
        out.push(prefix)?;
        out.push(if final_entry {
            "└── "
        } else {
            "├── "
        })?;
        out.push(name)?;
        if child.description.is_none() {
            out.push("/")?;
        }
        if let Some(description) = child.description
            && !description.is_empty()
        {
            out.push(" (")?;
            out.push(description)?;
            out.push(")")?;
        }
        out.push("\n")?;
        if child.description.is_none() {
            let mut next = BoundedString::default();
            next.push(prefix)?;
            next.push(if final_entry { "    " } else { "│   " })?;
            render_skill_node(child, &next.finish(), out)?;
        }
    }
    Ok(())
}

fn skill_path(name: &str, location: &str) -> Result<(String, String), RuntimeError> {
    let normalized = if location.trim().is_empty() {
        let mut value = BoundedString::default();
        value.push(MEMORY_DIR_PLACEHOLDER)?;
        value.push("/skills/")?;
        value.push(name)?;
        value.push("/SKILL.md")?;
        value.finish()
    } else {
        normalize_path(location.trim())?
    };
    validate_normalized_path(&normalized)?;
    if normalized.ends_with("/SKILL.md") {
        let skill_dir = parent_path(&normalized);
        let leaf = name
            .rsplit('/')
            .next()
            .ok_or_else(|| invalid("skill name"))?;
        if basename(skill_dir) == leaf {
            return Ok((
                parent_path(skill_dir).to_owned(),
                relative_to_parent(&normalized, 2)?,
            ));
        }
    }
    Ok((
        parent_path(&normalized).to_owned(),
        basename(&normalized).to_owned(),
    ))
}

fn relative_to_parent(value: &str, levels: usize) -> Result<String, RuntimeError> {
    let mut start = value.len();
    for _ in 0..levels {
        let parent = &value[..start];
        start = parent
            .rfind('/')
            .ok_or_else(|| invalid("skill display location"))?;
    }
    Ok(value[start + 1..].to_owned())
}

fn parent_path(value: &str) -> &str {
    match value.rfind('/') {
        Some(index) => &value[..index],
        None => ".",
    }
}
fn basename(value: &str) -> &str {
    match value.rsplit('/').next() {
        Some(value) => value,
        None => value,
    }
}
