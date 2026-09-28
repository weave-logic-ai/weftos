//! Codex: `.codex/agents/<id>.toml` (agent role file: `name`, `description`,
//! `developer_instructions`, as in the tracked `.codex/agents/world-builder.toml`)
//! and skills under `.agents/skills/<skill>/` (`~/.codex/skills` with `--global`).
//! The `AGENTS.md` pointer block is rendered in [`super::render`].

use serde::Serialize;

use super::package::Package;
use super::render::{Host, Layout, Merge, Rendered, one_line, skill_files};

#[derive(Serialize)]
struct RoleFile<'a> {
    name: &'a str,
    description: String,
    developer_instructions: String,
}

pub fn render(p: &Package, layout: Layout) -> Vec<Rendered> {
    let skills_dir = layout.codex_skills_dir();
    let mut out = skill_files(p, Host::Codex, skills_dir);
    if let Some(persona) = &p.persona {
        let mut instructions = String::new();
        if !p.skills.is_empty() {
            let names: Vec<String> = p.skills.iter().map(|s| format!("`{}`", s.name)).collect();
            instructions.push_str(&format!(
                "> Codex host note: this agent's procedures are the {} skill(s) in `{skills_dir}/`; read the SKILL.md before acting.\n\n",
                names.join(", ")
            ));
        }
        instructions.push_str(&persona.body);
        let role = RoleFile {
            name: &p.id,
            description: one_line(&persona.description),
            developer_instructions: instructions,
        };
        let text = toml::to_string(&role).expect("role file serializes");
        out.push(Rendered {
            path: format!(".codex/agents/{}.toml", p.id),
            bytes: text.into_bytes(),
            host: Host::Codex,
            owner: p.id.clone(),
            merge: Merge::File,
        });
    }
    out
}
