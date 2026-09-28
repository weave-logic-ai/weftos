//! Grok Build: `.grok/agents/<id>.md` and `.grok/skills/<skill>/`.
//!
//! Grok agent frontmatter differs from Claude's: it takes `prompt_mode` and
//! `agents_md` (matching the tracked `.grok/agents/*.md`) and no `tools`
//! list, since Grok's tool names are not Claude's.

use super::package::Package;
use super::render::{Host, Merge, Rendered, one_line, skill_files, yaml_str};

pub fn render(p: &Package) -> Vec<Rendered> {
    let mut out = skill_files(p, Host::Grok, ".grok/skills");
    if let Some(persona) = &p.persona {
        let mut text = format!(
            "---\nname: {}\ndescription: {}\nprompt_mode: full\nagents_md: true\n---\n\n",
            p.id,
            yaml_str(&one_line(&persona.description))
        );
        if !p.skills.is_empty() {
            let names: Vec<String> = p.skills.iter().map(|s| format!("`{}`", s.name)).collect();
            text.push_str(&format!(
                "> Grok host note: this agent's procedures are the {} skill(s) in `.grok/skills/`; load them before acting.\n\n",
                names.join(", ")
            ));
        }
        text.push_str(&persona.body);
        out.push(Rendered {
            path: format!(".grok/agents/{}.md", p.id),
            bytes: text.into_bytes(),
            host: Host::Grok,
            owner: p.id.clone(),
            merge: Merge::File,
        });
    }
    out
}
