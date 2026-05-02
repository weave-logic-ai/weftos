//! Shared help text for the `weft help` CLI subcommand and the `/help`
//! interactive slash command.
//!
//! Centralising the topic prose here ensures both surfaces stay in sync.

/// Return the general help overview (available commands and topics).
pub fn general_help() -> String {
    let mut output = String::from("weft -- clawft AI assistant CLI\n\n");
    output.push_str("Subcommands:\n");
    output.push_str("  agent          Start an interactive agent session or send a message\n");
    output.push_str("  gateway        Start channel gateway (Telegram, Slack, etc.)\n");
    output.push_str("  mcp-server     Run as an MCP tool server over stdio\n");
    output.push_str("  status         Show configuration status and diagnostics\n");
    output.push_str("  channels       Inspect channel configuration\n");
    output.push_str("  cron           Manage scheduled (cron) jobs\n");
    output.push_str("  sessions       Manage agent sessions\n");
    output.push_str("  memory         Read and search agent memory\n");
    output.push_str("  config         Show resolved configuration\n");
    output.push_str("  skills         Manage skills (list, show, install)\n");
    output.push_str("  tools          Manage tools (list, show, search, deny/allow)\n");
    output.push_str("  agents         Manage agents (list, show, use)\n");
    output.push_str("  workspace      Manage workspaces\n");
    output.push_str("  onboard        Initialize clawft config and workspace\n");
    output.push_str("  ui             Start the web dashboard\n");
    output.push_str("  kernel         WeftOS kernel management (status, ps, boot)\n");
    output.push_str("  help           Show help for a topic\n");
    output.push_str("  completions    Generate shell completions\n");
    output.push_str("\nHelp topics: skills, agents, tools, commands, config, ui, kernel\n");
    output.push_str("  Run 'weft help <topic>' for more information on a topic.");
    output
}

/// Return help text for a known topic, or an error message for an unknown one.
pub fn topic_help(topic: &str) -> String {
    match topic {
        "skills" => "Skills are reusable LLM instruction bundles.\n\
             \n\
             A skill defines a system prompt, allowed tools, and variables that\n\
             customise agent behaviour for a specific task (e.g. research, coding,\n\
             review).\n\
             \n\
             CLI commands:\n\
             \n\
             weft skills list             List all skills (workspace, user, builtin)\n\
             weft skills show <name>      Show skill details\n\
             weft skills install <path>   Install a skill from a local path\n\
             \n\
             Interactive commands:\n\
             \n\
             /skills                      List available skills\n\
             /use <name>                  Activate a skill for the current session"
            .into(),
        "agents" => "Agents are custom personas with their own system prompts and skills.\n\
             \n\
             Each agent has a name, an optional model override, and a set of skills.\n\
             You can switch between agents during an interactive session.\n\
             \n\
             CLI commands:\n\
             \n\
             weft agents list             List all agents\n\
             weft agents show <name>      Show agent details\n\
             weft agents use <name>       Set the default agent\n\
             \n\
             Interactive commands:\n\
             \n\
             /agent <name>                Switch to a different agent\n\
             /status                      Show the current agent"
            .into(),
        "tools" => "Tools are functions the agent can call during conversation.\n\
             \n\
             They are registered via MCP servers or built-in tool providers.\n\
             The agent uses tools to interact with the filesystem, run commands,\n\
             search the web, and more.\n\
             \n\
             CLI commands:\n\
             \n\
             weft tools list              List all registered tools with source\n\
             weft tools show <name>       Show tool details and parameter schema\n\
             weft tools mcp               List MCP servers and tool counts\n\
             weft tools search <query>    Search tools by name or description\n\
             weft tools deny <pattern>    Add a glob pattern to the tool denylist\n\
             weft tools allow <pattern>   Remove a pattern from the tool denylist\n\
             \n\
             Interactive commands:\n\
             \n\
             /tools                       List all registered tools"
            .into(),
        "commands" => "Interactive slash commands (available inside `weft agent`):\n\
             \n\
             /help [topic]                Show help or topic-specific help\n\
             /skills                      List available skills\n\
             /use <skill>                 Activate a skill\n\
             /agent <name>                Switch agent\n\
             /tools                       List available tools\n\
             /clear                       Clear context\n\
             /status                      Show current agent, model, skills\n\
             /quit                        Exit the session"
            .into(),
        "ui" => "## weft ui\n\
             \n\
             Start the web dashboard.\n\
             \n\
             This command starts the gateway with the REST/WS API enabled\n\
             and optionally serves a built frontend from a local directory.\n\
             \n\
             ### Usage\n\
             \n\
               weft ui [OPTIONS]\n\
             \n\
             ### Options\n\
             \n\
               -c, --config <PATH>    Config file override\n\
               -p, --port <PORT>      API port (default: 18789)\n\
               --no-open              Don't open browser\n\
               --ui-dir <DIR>         Serve built UI from this directory\n\
             \n\
             ### Examples\n\
             \n\
               weft ui                        # Start with defaults\n\
               weft ui --port 9000            # Custom port\n\
               weft ui --ui-dir ./clawft-ui/dist     # Serve built frontend\n\
               weft ui --no-open              # Skip browser auto-open"
            .into(),
        "config" => "Configuration is loaded from (in priority order):\n\
             \n\
             1. $CLAWFT_CONFIG environment variable\n\
             2. ~/.clawft/config.json\n\
             3. ~/.nanobot/config.json\n\
             \n\
             CLI commands:\n\
             \n\
             weft config show             Show the full resolved configuration\n\
             weft config section <name>   Show a specific section\n\
             weft status                  Show configuration status and diagnostics"
            .into(),
        "kernel" => "## WeftOS Kernel\n\
             \n\
             The WeftOS kernel provides process management, service lifecycle,\n\
             IPC, and health monitoring for the clawft agent framework.\n\
             \n\
             CLI commands:\n\
             \n\
             weft kernel status         Show kernel state, uptime, counts\n\
             weft kernel services       List registered services with health\n\
             weft kernel ps             List process table entries\n\
             weft kernel boot           Boot kernel (non-interactive)\n\
             weft kernel boot --fg      Boot in foreground with log output"
            .into(),
        other => format!(
            "No help available for topic: {other}\n\
             \n\
             Available topics: skills, agents, tools, commands, config, ui, kernel"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_help_contains_subcommands() {
        let text = general_help();
        assert!(text.contains("agent"));
        assert!(text.contains("gateway"));
        assert!(text.contains("skills"));
        assert!(text.contains("tools"));
        assert!(text.contains("help"));
    }

    #[test]
    fn general_help_lists_topics() {
        let text = general_help();
        assert!(text.contains("Help topics:"));
        assert!(text.contains("skills"));
        assert!(text.contains("agents"));
        assert!(text.contains("tools"));
    }

    #[test]
    fn topic_skills() {
        let text = topic_help("skills");
        assert!(text.contains("instruction bundles"));
        assert!(text.contains("weft skills list"));
    }

    #[test]
    fn topic_agents() {
        let text = topic_help("agents");
        assert!(text.contains("personas"));
        assert!(text.contains("weft agents list"));
    }

    #[test]
    fn topic_tools() {
        let text = topic_help("tools");
        assert!(text.contains("functions"));
        assert!(text.contains("weft tools list"));
        assert!(text.contains("weft tools deny"));
    }

    #[test]
    fn topic_commands() {
        let text = topic_help("commands");
        assert!(text.contains("/help"));
        assert!(text.contains("/quit"));
    }

    #[test]
    fn topic_config() {
        let text = topic_help("config");
        assert!(text.contains("CLAWFT_CONFIG"));
        assert!(text.contains("weft config show"));
    }

    #[test]
    fn topic_unknown() {
        let text = topic_help("nonexistent");
        assert!(text.contains("No help available"));
        assert!(text.contains("Available topics"));
    }
}
