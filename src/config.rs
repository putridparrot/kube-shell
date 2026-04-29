use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::PathBuf;

use crate::constants::*;
use crate::types::{OutputProfile, ShellState};

/// Get the state file path
pub fn state_file() -> PathBuf {
    if let Ok(cwd) = env::current_dir() {
        return cwd.join(STATE_FILE_NAME);
    }

    if let Ok(home) = env::var("USERPROFILE") {
        return PathBuf::from(home).join(STATE_FILE_NAME);
    }

    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home).join(STATE_FILE_NAME);
    }

    PathBuf::from(STATE_FILE_NAME)
}

/// Get the history file path
pub fn history_file() -> PathBuf {
    if let Ok(home) = env::var("USERPROFILE") {
        return PathBuf::from(home).join(".kube-shell-history");
    }

    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home).join(".kube-shell-history");
    }

    PathBuf::from(".kube-shell-history")
}

/// Get the home config file path
pub fn home_config_file() -> PathBuf {
    if let Ok(home) = env::var("USERPROFILE") {
        return PathBuf::from(home).join(CONFIG_FILE_NAME);
    }

    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home).join(CONFIG_FILE_NAME);
    }

    PathBuf::from(CONFIG_FILE_NAME)
}

/// Get the runtime aliases file path
pub fn aliases_file() -> PathBuf {
    if let Ok(cwd) = env::current_dir() {
        return cwd.join(ALIASES_FILE_NAME);
    }

    if let Ok(home) = env::var("USERPROFILE") {
        return PathBuf::from(home).join(ALIASES_FILE_NAME);
    }

    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home).join(ALIASES_FILE_NAME);
    }

    PathBuf::from(ALIASES_FILE_NAME)
}

/// Resolve the config file (workspace or home)
pub fn resolve_config_file() -> PathBuf {
    if let Ok(cwd) = env::current_dir() {
        let workspace_config = cwd.join(CONFIG_FILE_NAME);
        if workspace_config.exists() {
            return workspace_config;
        }
    }

    home_config_file()
}

/// Save shell state to file
pub fn save_shell_state(state: &ShellState) -> Result<(), String> {
    let content = [
        format!("output_profile={}", state.output_profile.label()),
        format!("dry_run={}", state.dry_run),
        format!("show_commands={}", state.show_commands),
        format!(
            "previous_context={}",
            state.previous_context.clone().unwrap_or_default()
        ),
        format!(
            "previous_namespace={}",
            state.previous_namespace.clone().unwrap_or_default()
        ),
    ]
    .join("\n");

    fs::write(&state.state_file, format!("{content}\n"))
        .map_err(|err| format!("Failed to save shell state: {err}"))
}

/// Load shell state from file
pub fn load_shell_state(
    path: &PathBuf,
) -> (
    Option<OutputProfile>,
    Option<bool>,
    Option<bool>,
    Option<String>,
    Option<String>,
) {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return (None, None, None, None, None),
    };

    let mut output_profile = None;
    let mut dry_run = None;
    let mut show_commands = None;
    let mut previous_context = None;
    let mut previous_namespace = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(v) = line.strip_prefix("output_profile=") {
            output_profile = OutputProfile::from_name(v.trim());
            continue;
        }

        if let Some(v) = line.strip_prefix("dry_run=") {
            let v = v.trim().to_ascii_lowercase();
            if matches!(v.as_str(), "true" | "1" | "on" | "yes") {
                dry_run = Some(true);
            } else if matches!(v.as_str(), "false" | "0" | "off" | "no") {
                dry_run = Some(false);
            }
            continue;
        }

        if let Some(v) = line.strip_prefix("show_commands=") {
            let v = v.trim().to_ascii_lowercase();
            if matches!(v.as_str(), "true" | "1" | "on" | "yes") {
                show_commands = Some(true);
            } else if matches!(v.as_str(), "false" | "0" | "off" | "no") {
                show_commands = Some(false);
            }
            continue;
        }

        if let Some(v) = line.strip_prefix("previous_context=") {
            let v = v.trim();
            if !v.is_empty() {
                previous_context = Some(v.to_string());
            }
            continue;
        }

        if let Some(v) = line.strip_prefix("previous_namespace=") {
            let v = v.trim();
            if !v.is_empty() {
                previous_namespace = Some(v.to_string());
            }
        }
    }

    (
        output_profile,
        dry_run,
        show_commands,
        previous_context,
        previous_namespace,
    )
}

/// Get default exec commands
pub fn default_exec_commands() -> Vec<String> {
    EXEC_INNER_COMMANDS.iter().map(|s| (*s).to_string()).collect()
}

/// Get default hint color prefix
pub fn default_hint_color_prefix() -> String {
    "\x1b[90m".to_string()
}

/// Parse hint color prefix from config value
pub fn parse_hint_color_prefix(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.eq_ignore_ascii_case("light-gray")
        || trimmed.eq_ignore_ascii_case("light_gray")
        || trimmed.eq_ignore_ascii_case("light_grey")
        || trimmed.eq_ignore_ascii_case("lightgray")
        || trimmed.eq_ignore_ascii_case("gray")
        || trimmed.eq_ignore_ascii_case("grey")
    {
        return Some("\x1b[90m".to_string());
    }

    if trimmed.starts_with("\x1b[") && trimmed.ends_with('m') {
        return Some(trimmed.to_string());
    }

    if trimmed.chars().all(|c| c.is_ascii_digit() || c == ';') {
        return Some(format!("\x1b[{trimmed}m"));
    }

    None
}

/// Load exec commands from config file
pub fn load_exec_commands(path: &PathBuf) -> Vec<String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return default_exec_commands(),
    };

    let mut commands: Vec<String> = Vec::new();

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(value) = line.strip_prefix("exec_inner_command=") {
            let cmd = value.trim();
            if !cmd.is_empty() {
                commands.push(cmd.to_string());
            }
            continue;
        }

        if let Some(value) = line.strip_prefix("exec_inner_commands=") {
            commands.extend(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(ToOwned::to_owned),
            );
        }
    }

    if commands.is_empty() {
        return default_exec_commands();
    }

    commands.sort();
    commands.dedup();
    commands
}

/// Load aliases from config file
pub fn load_aliases(path: &PathBuf) -> HashMap<String, String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return HashMap::new(),
    };

    let mut aliases = HashMap::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(rest) = line.strip_prefix("alias ") {
            if let Some((name, expansion)) = rest.split_once('=') {
                let name = name.trim();
                let expansion = expansion.trim();
                if !name.is_empty() && !expansion.is_empty() {
                    aliases.insert(name.to_string(), expansion.to_string());
                }
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("alias.")
            && let Some((name, expansion)) = rest.split_once('=')
        {
            let name = name.trim();
            let expansion = expansion.trim();
            if !name.is_empty() && !expansion.is_empty() {
                aliases.insert(name.to_string(), expansion.to_string());
            }
        }
    }

    aliases
}

/// Load persisted runtime aliases from file
pub fn load_runtime_aliases(path: &PathBuf) -> HashMap<String, String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return HashMap::new(),
    };

    let mut aliases = HashMap::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some((name, expansion)) = line.split_once('=') {
            let name = name.trim();
            let expansion = expansion.trim();
            if !name.is_empty() && !expansion.is_empty() {
                aliases.insert(name.to_string(), expansion.to_string());
            }
        }
    }

    aliases
}

/// Load safe delete setting from config
pub fn load_safe_delete(path: &PathBuf) -> bool {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return true,
    };

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(value) = line.strip_prefix("safe_delete=") {
            let value = value.trim().to_ascii_lowercase();
            if matches!(value.as_str(), "false" | "0" | "off" | "no") {
                return false;
            }
            if matches!(value.as_str(), "true" | "1" | "on" | "yes") {
                return true;
            }
        }
    }

    true
}

/// Load dry run setting from config
pub fn load_dry_run(path: &PathBuf) -> bool {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return false,
    };

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(value) = line.strip_prefix("dry_run=") {
            let value = value.trim().to_ascii_lowercase();
            if matches!(value.as_str(), "true" | "1" | "on" | "yes") {
                return true;
            }
            if matches!(value.as_str(), "false" | "0" | "off" | "no") {
                return false;
            }
        }
    }

    false
}

/// Load show commands setting from config
pub fn load_show_commands(path: &PathBuf) -> bool {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return false,
    };

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(value) = line.strip_prefix("show_commands=") {
            let value = value.trim().to_ascii_lowercase();
            if matches!(value.as_str(), "true" | "1" | "on" | "yes") {
                return true;
            }
            if matches!(value.as_str(), "false" | "0" | "off" | "no") {
                return false;
            }
        }
    }

    false
}

/// Load session namespace mode setting from config
/// When true, `ns` and `use .../<ns>` update only this kube-shell session.
pub fn load_session_namespace_mode(path: &PathBuf) -> bool {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return false,
    };

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(value) = line.strip_prefix("session_namespace_mode=") {
            let value = value.trim().to_ascii_lowercase();
            if matches!(value.as_str(), "true" | "1" | "on" | "yes") {
                return true;
            }
            if matches!(value.as_str(), "false" | "0" | "off" | "no") {
                return false;
            }
        }
    }

    false
}

/// Load risky contexts from config
pub fn load_risky_contexts(path: &PathBuf) -> HashSet<String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return HashSet::new(),
    };

    let mut contexts = HashSet::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(value) = line.strip_prefix("risky_context=") {
            let value = value.trim();
            if !value.is_empty() {
                contexts.insert(value.to_string());
            }
            continue;
        }

        if let Some(value) = line.strip_prefix("risky_contexts=") {
            for ctx in value.split(',').map(str::trim).filter(|v| !v.is_empty()) {
                contexts.insert(ctx.to_string());
            }
        }
    }

    contexts
}

/// Load hint color prefix from config
pub fn load_hint_color_prefix(path: &PathBuf) -> String {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return default_hint_color_prefix(),
    };

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(value) = line.strip_prefix("hint_color=") {
            if let Some(prefix) = parse_hint_color_prefix(value) {
                return prefix;
            }
        }
    }

    default_hint_color_prefix()
}

/// Load prompt template from config
pub fn load_prompt_template(path: &PathBuf) -> String {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return DEFAULT_PROMPT_TEMPLATE.to_string(),
    };

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(value) = line.strip_prefix("prompt_template=") {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }

    DEFAULT_PROMPT_TEMPLATE.to_string()
}

/// Save runtime aliases to file
pub fn save_runtime_aliases(path: &PathBuf, aliases: &HashMap<String, String>) -> Result<(), String> {
    let mut entries: Vec<(&String, &String)> = aliases.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let content = entries
        .into_iter()
        .map(|(name, expansion)| format!("{name}={expansion}"))
        .collect::<Vec<_>>()
        .join("\n");

    fs::write(path, format!("{content}\n"))
        .map_err(|err| format!("Failed to save runtime aliases: {err}"))
}

/// Load AI URL from config (default: http://localhost:11434)
pub fn load_ai_url(path: &PathBuf) -> String {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return "http://localhost:11434".to_string(),
    };

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(value) = line.strip_prefix("ai_url=") {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }

    "http://localhost:11434".to_string()
}

/// Load AI model from config (default: llama3.2)
pub fn load_ai_model(path: &PathBuf) -> String {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return "llama3.2".to_string(),
    };

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(value) = line.strip_prefix("ai_model=") {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }

    "llama3.2".to_string()
}

fn decode_escaped_config_value(value: &str) -> String {
    value
        .replace("\\n", "\n")
        .replace("\\t", "\t")
        .replace("\\r", "\r")
}

/// Load AI ask prompt template from config.
/// Placeholders: {question}, {context}, {namespace}
pub fn load_ai_ask_prompt_template(path: &PathBuf) -> String {
    const DEFAULT: &str = "You are a Kubernetes expert assistant. Current kubectl context: {context}. Current namespace: {namespace}. Answer the following question clearly and concisely. Use plain text without markdown formatting.\n\nQuestion:\n{question}";

    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return DEFAULT.to_string(),
    };

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(value) = line.strip_prefix("ai_ask_prompt_template=") {
            let value = value.trim();
            if !value.is_empty() {
                return decode_escaped_config_value(value);
            }
        }
    }

    DEFAULT.to_string()
}

/// Load AI explain prompt template from config.
/// Placeholders: {output}, {command}, {context}, {namespace}
pub fn load_ai_explain_prompt_template(path: &PathBuf) -> String {
    const DEFAULT: &str = "You are a Kubernetes expert. Current kubectl context: {context}. Current namespace: {namespace}. Explain the following kubectl output clearly and concisely, highlighting anything notable. Use plain text without markdown formatting.\n\nCommand:\n{command}\n\nOutput:\n{output}";

    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return DEFAULT.to_string(),
    };

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(value) = line.strip_prefix("ai_explain_prompt_template=") {
            let value = value.trim();
            if !value.is_empty() {
                return decode_escaped_config_value(value);
            }
        }
    }

    DEFAULT.to_string()
}
