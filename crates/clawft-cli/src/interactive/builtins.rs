//! Built-in slash commands for the `weft agent` interactive REPL.
//!
//! Provides the standard set of commands:
//!
//! - `/help [topic]` -- show available commands or topic help
//! - `/skills` -- list available skills
//! - `/use <skill>` -- activate a skill
//! - `/agent <name>` -- switch agent
//! - `/clear` -- clear context
//! - `/status` -- show current agent, model, skills
//! - `/quit` -- exit

use tracing::warn;

use super::registry::{InteractiveContext, SlashCommand, SlashCommandRegistry};

/// Register all built-in slash commands into the given registry.
pub fn register_builtins(registry: &mut SlashCommandRegistry) {
    registry.register(Box::new(HelpCommand));
    registry.register(Box::new(SkillsCommand));
    registry.register(Box::new(UseCommand));
    registry.register(Box::new(AgentCommand));
    registry.register(Box::new(ClearCommand));
    registry.register(Box::new(StatusCommand));
    registry.register(Box::new(QuitCommand));
    registry.register(Box::new(ToolsCommand));
}

/// Register skill-contributed commands into the registry.
///
/// Skills with `user_invocable: true` register as `/skill_name` commands.
/// If a skill command conflicts with a built-in command, the collision is
/// logged as a warning and the skill command is skipped.
///
/// Returns the number of skill commands successfully registered.
pub fn register_skill_commands(
    registry: &mut SlashCommandRegistry,
    skills: &[(String, String)],
) -> usize {
    let mut count = 0;
    for (name, description) in skills {
        let cmd = Box::new(SkillInvokeCommand {
            skill_name: name.clone(),
            skill_description: description.clone(),
        });
        match registry.register_checked(cmd) {
            Ok(()) => {
                count += 1;
            }
            Err(collision) => {
                warn!(
                    skill = %name,
                    collision = %collision,
                    "skill command '/{name}' conflicts with existing command, skipping"
                );
            }
        }
    }
    count
}

/// A dynamically registered slash command that activates a skill.
///
/// When a user types `/<skill_name>`, this command activates the skill
/// in the interactive context.
struct SkillInvokeCommand {
    skill_name: String,
    skill_description: String,
}

impl SlashCommand for SkillInvokeCommand {
    fn name(&self) -> &str {
        &self.skill_name
    }

    fn description(&self) -> &str {
        &self.skill_description
    }

    fn execute(&self, args: &str, ctx: &mut InteractiveContext) -> anyhow::Result<String> {
        ctx.active_skill = self.skill_name.clone();
        if args.is_empty() {
            Ok(format!("Activated skill: {}", self.skill_name))
        } else {
            Ok(format!(
                "Activated skill: {} (with args: {})",
                self.skill_name, args
            ))
        }
    }
}

// ── /help ─────────────────────────────────────────────────────────────────

/// `/help [topic]` -- show available commands or topic-specific help.
struct HelpCommand;

impl SlashCommand for HelpCommand {
    fn name(&self) -> &str {
        "help"
    }

    fn description(&self) -> &str {
        "Show available commands or topic help"
    }

    fn execute(&self, args: &str, _ctx: &mut InteractiveContext) -> anyhow::Result<String> {
        let topic = args.trim();

        if topic.is_empty() {
            return Ok(format_general_help());
        }

        match topic {
            "skills" => Ok("Skills are reusable LLM instruction bundles.\n\
                 Use /skills to list available skills.\n\
                 Use /use <name> to activate a skill for the current session."
                .into()),
            "agents" => Ok(
                "Agents are custom personas with their own system prompts and skills.\n\
                 Use /agent <name> to switch to a different agent.\n\
                 Use /status to see the current agent."
                    .into(),
            ),
            "tools" => Ok(
                "Tools are functions the agent can call during conversation.\n\
                 Use /tools to list all registered tools."
                    .into(),
            ),
            _ => Ok(format!("No help available for topic: {topic}")),
        }
    }
}

/// Format the general help text listing all built-in commands.
fn format_general_help() -> String {
    let mut output = String::from("Commands:\n");
    output.push_str("  /help [topic]     -- Show this help or topic-specific help\n");
    output.push_str("  /skills           -- List available skills\n");
    output.push_str("  /use <skill>      -- Activate a skill\n");
    output.push_str("  /agent <name>     -- Switch agent\n");
    output.push_str("  /tools            -- List available tools\n");
    output.push_str("  /clear            -- Clear context\n");
    output.push_str("  /status           -- Show current agent, model, skills\n");
    output.push_str("  /quit             -- Exit the session\n");
    output.push_str("\nTopics: skills, agents, tools");
    output
}

// ── /skills ───────────────────────────────────────────────────────────────

/// `/skills` -- list available skills from the context.
struct SkillsCommand;

impl SlashCommand for SkillsCommand {
    fn name(&self) -> &str {
        "skills"
    }

    fn description(&self) -> &str {
        "List available skills"
    }

    fn execute(&self, _args: &str, ctx: &mut InteractiveContext) -> anyhow::Result<String> {
        if ctx.skill_names.is_empty() {
            return Ok("No skills available.".into());
        }

        let mut output = format!("Available skills ({}):\n", ctx.skill_names.len());
        for name in &ctx.skill_names {
            let marker = if name == &ctx.active_skill {
                " (active)"
            } else {
                ""
            };
            output.push_str(&format!("  - {name}{marker}\n"));
        }
        Ok(output)
    }
}

// ── /use ──────────────────────────────────────────────────────────────────

/// `/use <skill>` -- activate a skill for the current session.
struct UseCommand;

impl SlashCommand for UseCommand {
    fn name(&self) -> &str {
        "use"
    }

    fn description(&self) -> &str {
        "Activate a skill"
    }

    fn execute(&self, args: &str, ctx: &mut InteractiveContext) -> anyhow::Result<String> {
        let skill_name = args.trim();

        if skill_name.is_empty() {
            if ctx.active_skill.is_empty() {
                return Ok("No skill is currently active. Usage: /use <skill-name>".into());
            }
            ctx.active_skill.clear();
            return Ok("Skill deactivated.".into());
        }

        if !ctx.skill_names.contains(&skill_name.to_string()) {
            return Ok(format!(
                "Unknown skill: {skill_name}\nUse /skills to see available skills."
            ));
        }

        ctx.active_skill = skill_name.to_string();
        Ok(format!("Activated skill: {skill_name}"))
    }
}

// ── /agent ────────────────────────────────────────────────────────────────

/// `/agent <name>` -- switch to a different agent.
struct AgentCommand;

impl SlashCommand for AgentCommand {
    fn name(&self) -> &str {
        "agent"
    }

    fn description(&self) -> &str {
        "Switch agent"
    }

    fn execute(&self, args: &str, ctx: &mut InteractiveContext) -> anyhow::Result<String> {
        let agent_name = args.trim();

        if agent_name.is_empty() {
            if ctx.active_agent.is_empty() {
                return Ok("Using default agent. Usage: /agent <name>".into());
            }
            return Ok(format!("Current agent: {}", ctx.active_agent));
        }

        if !ctx.agent_names.contains(&agent_name.to_string()) {
            let mut msg = format!("Unknown agent: {agent_name}\n");
            if ctx.agent_names.is_empty() {
                msg.push_str("No agents are registered.");
            } else {
                msg.push_str("Available agents:\n");
                for name in &ctx.agent_names {
                    msg.push_str(&format!("  - {name}\n"));
                }
            }
            return Ok(msg);
        }

        ctx.active_agent = agent_name.to_string();
        Ok(format!("Switched to agent: {agent_name}"))
    }
}

// ── /tools ────────────────────────────────────────────────────────────────

/// `/tools` -- list registered tools.
struct ToolsCommand;

impl SlashCommand for ToolsCommand {
    fn name(&self) -> &str {
        "tools"
    }

    fn description(&self) -> &str {
        "List available tools"
    }

    fn execute(&self, _args: &str, ctx: &mut InteractiveContext) -> anyhow::Result<String> {
        if ctx.tool_names.is_empty() {
            return Ok("No tools registered.".into());
        }

        let mut output = format!("Registered tools ({}):\n", ctx.tool_names.len());
        for name in &ctx.tool_names {
            output.push_str(&format!("  - {name}\n"));
        }
        Ok(output)
    }
}

// ── /clear ────────────────────────────────────────────────────────────────

/// `/clear` -- clear session context.
struct ClearCommand;

impl SlashCommand for ClearCommand {
    fn name(&self) -> &str {
        "clear"
    }

    fn description(&self) -> &str {
        "Clear context"
    }

    fn execute(&self, _args: &str, ctx: &mut InteractiveContext) -> anyhow::Result<String> {
        ctx.active_skill.clear();
        Ok("[session cleared]".into())
    }
}

// ── /status ───────────────────────────────────────────────────────────────

/// `/status` -- show current agent, model, and active skills.
struct StatusCommand;

impl SlashCommand for StatusCommand {
    fn name(&self) -> &str {
        "status"
    }

    fn description(&self) -> &str {
        "Show current agent, model, skills"
    }

    fn execute(&self, _args: &str, ctx: &mut InteractiveContext) -> anyhow::Result<String> {
        let agent = if ctx.active_agent.is_empty() {
            "(default)"
        } else {
            &ctx.active_agent
        };

        let skill = if ctx.active_skill.is_empty() {
            "(none)"
        } else {
            &ctx.active_skill
        };

        Ok(format!(
            "Agent:  {agent}\n\
             Model:  {}\n\
             Skill:  {skill}\n\
             Tools:  {} registered\n\
             Skills: {} available\n\
             Agents: {} available",
            ctx.model,
            ctx.tool_names.len(),
            ctx.skill_names.len(),
            ctx.agent_names.len(),
        ))
    }
}

// ── /quit ─────────────────────────────────────────────────────────────────

/// `/quit` -- signal exit from the interactive session.
///
/// This command returns a special sentinel string that the REPL loop
/// should check to break out of the read-eval-print loop.
struct QuitCommand;

/// Sentinel value returned by `/quit` to signal session exit.
pub const QUIT_SENTINEL: &str = "__QUIT__";

impl SlashCommand for QuitCommand {
    fn name(&self) -> &str {
        "quit"
    }

    fn description(&self) -> &str {
        "Exit the session"
    }

    fn execute(&self, _args: &str, _ctx: &mut InteractiveContext) -> anyhow::Result<String> {
        Ok(QUIT_SENTINEL.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> InteractiveContext {
        let mut ctx = InteractiveContext::new("test-model/v1".into());
        ctx.tool_names = vec!["read_file".into(), "write_file".into()];
        ctx.skill_names = vec!["research".into(), "coding".into()];
        ctx.agent_names = vec!["researcher".into(), "coder".into()];
        ctx
    }

    // ── /help tests ────────────────────────────────────────────────────

    #[test]
    fn help_general() {
        let cmd = HelpCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("", &mut ctx).unwrap();
        assert!(output.contains("Commands:"));
        assert!(output.contains("/help"));
        assert!(output.contains("/skills"));
        assert!(output.contains("/quit"));
    }

    #[test]
    fn help_topic_skills() {
        let cmd = HelpCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("skills", &mut ctx).unwrap();
        assert!(output.contains("instruction bundles"));
    }

    #[test]
    fn help_topic_agents() {
        let cmd = HelpCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("agents", &mut ctx).unwrap();
        assert!(output.contains("personas"));
    }

    #[test]
    fn help_topic_tools() {
        let cmd = HelpCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("tools", &mut ctx).unwrap();
        assert!(output.contains("functions"));
    }

    #[test]
    fn help_unknown_topic() {
        let cmd = HelpCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("nonexistent", &mut ctx).unwrap();
        assert!(output.contains("No help available"));
    }

    // ── /skills tests ──────────────────────────────────────────────────

    #[test]
    fn skills_lists_available() {
        let cmd = SkillsCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("", &mut ctx).unwrap();
        assert!(output.contains("research"));
        assert!(output.contains("coding"));
        assert!(output.contains("2"));
    }

    #[test]
    fn skills_marks_active() {
        let cmd = SkillsCommand;
        let mut ctx = test_ctx();
        ctx.active_skill = "research".into();
        let output = cmd.execute("", &mut ctx).unwrap();
        assert!(output.contains("research (active)"));
    }

    #[test]
    fn skills_empty() {
        let cmd = SkillsCommand;
        let mut ctx = InteractiveContext::new("m".into());
        let output = cmd.execute("", &mut ctx).unwrap();
        assert!(output.contains("No skills available"));
    }

    // ── /use tests ─────────────────────────────────────────────────────

    #[test]
    fn use_activates_skill() {
        let cmd = UseCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("research", &mut ctx).unwrap();
        assert!(output.contains("Activated skill: research"));
        assert_eq!(ctx.active_skill, "research");
    }

    #[test]
    fn use_unknown_skill() {
        let cmd = UseCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("nonexistent", &mut ctx).unwrap();
        assert!(output.contains("Unknown skill"));
    }

    #[test]
    fn use_no_args_deactivates() {
        let cmd = UseCommand;
        let mut ctx = test_ctx();
        ctx.active_skill = "research".into();
        let output = cmd.execute("", &mut ctx).unwrap();
        assert!(output.contains("deactivated"));
        assert!(ctx.active_skill.is_empty());
    }

    #[test]
    fn use_no_args_no_active_shows_usage() {
        let cmd = UseCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("", &mut ctx).unwrap();
        assert!(output.contains("Usage"));
    }

    // ── /agent tests ───────────────────────────────────────────────────

    #[test]
    fn agent_switches() {
        let cmd = AgentCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("researcher", &mut ctx).unwrap();
        assert!(output.contains("Switched to agent: researcher"));
        assert_eq!(ctx.active_agent, "researcher");
    }

    #[test]
    fn agent_unknown() {
        let cmd = AgentCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("nonexistent", &mut ctx).unwrap();
        assert!(output.contains("Unknown agent"));
        assert!(output.contains("researcher"));
    }

    #[test]
    fn agent_no_args_shows_current() {
        let cmd = AgentCommand;
        let mut ctx = test_ctx();
        ctx.active_agent = "coder".into();
        let output = cmd.execute("", &mut ctx).unwrap();
        assert!(output.contains("Current agent: coder"));
    }

    #[test]
    fn agent_no_args_default() {
        let cmd = AgentCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("", &mut ctx).unwrap();
        assert!(output.contains("default"));
    }

    // ── /tools tests ───────────────────────────────────────────────────

    #[test]
    fn tools_lists_registered() {
        let cmd = ToolsCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("", &mut ctx).unwrap();
        assert!(output.contains("read_file"));
        assert!(output.contains("write_file"));
        assert!(output.contains("2"));
    }

    #[test]
    fn tools_empty() {
        let cmd = ToolsCommand;
        let mut ctx = InteractiveContext::new("m".into());
        let output = cmd.execute("", &mut ctx).unwrap();
        assert!(output.contains("No tools registered"));
    }

    // ── /clear tests ───────────────────────────────────────────────────

    #[test]
    fn clear_clears_skill() {
        let cmd = ClearCommand;
        let mut ctx = test_ctx();
        ctx.active_skill = "research".into();
        let output = cmd.execute("", &mut ctx).unwrap();
        assert!(output.contains("cleared"));
        assert!(ctx.active_skill.is_empty());
    }

    // ── /status tests ──────────────────────────────────────────────────

    #[test]
    fn status_shows_info() {
        let cmd = StatusCommand;
        let mut ctx = test_ctx();
        ctx.active_agent = "researcher".into();
        ctx.active_skill = "coding".into();
        let output = cmd.execute("", &mut ctx).unwrap();
        assert!(output.contains("researcher"));
        assert!(output.contains("test-model/v1"));
        assert!(output.contains("coding"));
        assert!(output.contains("2 registered")); // tools
        assert!(output.contains("2 available")); // skills
    }

    #[test]
    fn status_defaults() {
        let cmd = StatusCommand;
        let mut ctx = InteractiveContext::new("model".into());
        let output = cmd.execute("", &mut ctx).unwrap();
        assert!(output.contains("(default)"));
        assert!(output.contains("(none)"));
    }

    // ── /quit tests ────────────────────────────────────────────────────

    #[test]
    fn quit_returns_sentinel() {
        let cmd = QuitCommand;
        let mut ctx = test_ctx();
        let output = cmd.execute("", &mut ctx).unwrap();
        assert_eq!(output, QUIT_SENTINEL);
    }

    // ── register_builtins tests ────────────────────────────────────────

    #[test]
    fn register_builtins_registers_all() {
        let mut reg = SlashCommandRegistry::new();
        register_builtins(&mut reg);

        assert!(reg.has("help"));
        assert!(reg.has("skills"));
        assert!(reg.has("use"));
        assert!(reg.has("agent"));
        assert!(reg.has("tools"));
        assert!(reg.has("clear"));
        assert!(reg.has("status"));
        assert!(reg.has("quit"));
        assert_eq!(reg.len(), 8);
    }

    #[test]
    fn builtins_dispatch_through_registry() {
        let mut reg = SlashCommandRegistry::new();
        register_builtins(&mut reg);
        let mut ctx = test_ctx();

        let result = reg.dispatch("/help", &mut ctx);
        assert!(result.is_some());
        let output = result.unwrap().unwrap();
        assert!(output.contains("Commands:"));
    }

    // ── skill command registration tests ──────────────────────────────

    #[test]
    fn register_skill_commands_adds_commands() {
        let mut reg = SlashCommandRegistry::new();
        register_builtins(&mut reg);
        let initial_count = reg.len();

        let skills = vec![
            ("research".into(), "Deep research on a topic".into()),
            ("coding".into(), "Write code".into()),
        ];

        let added = register_skill_commands(&mut reg, &skills);
        assert_eq!(added, 2);
        assert_eq!(reg.len(), initial_count + 2);
        assert!(reg.has("research"));
        assert!(reg.has("coding"));
    }

    #[test]
    fn register_skill_commands_collision_detected() {
        let mut reg = SlashCommandRegistry::new();
        register_builtins(&mut reg);

        // "help" collides with the built-in /help command
        let skills = vec![
            ("help".into(), "Conflicting skill".into()),
            ("myskill".into(), "Non-conflicting skill".into()),
        ];

        let added = register_skill_commands(&mut reg, &skills);
        assert_eq!(added, 1);
        assert!(reg.has("myskill"));
        // /help should still be the built-in version
        let mut ctx = test_ctx();
        let result = reg.dispatch("/help", &mut ctx).unwrap().unwrap();
        assert!(result.contains("Commands:"));
    }

    #[test]
    fn skill_invoke_command_activates_skill() {
        let mut reg = SlashCommandRegistry::new();
        register_builtins(&mut reg);

        let skills = vec![("research".into(), "Research stuff".into())];
        register_skill_commands(&mut reg, &skills);

        let mut ctx = test_ctx();
        let result = reg.dispatch("/research", &mut ctx).unwrap().unwrap();
        assert!(result.contains("Activated skill: research"));
        assert_eq!(ctx.active_skill, "research");
    }

    #[test]
    fn skill_invoke_command_with_args() {
        let mut reg = SlashCommandRegistry::new();
        let skills = vec![("lookup".into(), "Lookup things".into())];
        register_skill_commands(&mut reg, &skills);

        let mut ctx = test_ctx();
        let result = reg
            .dispatch("/lookup some query", &mut ctx)
            .unwrap()
            .unwrap();
        assert!(result.contains("Activated skill: lookup"));
        assert!(result.contains("some query"));
    }

    #[test]
    fn register_checked_rejects_duplicate() {
        let mut reg = SlashCommandRegistry::new();
        register_builtins(&mut reg);

        let cmd = Box::new(SkillInvokeCommand {
            skill_name: "quit".into(),
            skill_description: "Conflicts with quit".into(),
        });
        let result = reg.register_checked(cmd);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "quit");
    }
}
