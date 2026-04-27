use std::collections::HashMap;
use std::io::{self, Write};
use std::process::{Command, Stdio};

use crate::kubectl::*;
use crate::types::ShellState;
use crate::config::save_bookmarks;

/// Parse command line with shell-like quoting
pub fn parse_command_line(input: &str) -> Result<Vec<String>, String> {
    shlex::split(input).ok_or_else(|| "Unable to parse command line. Check quotes.".to_string())
}

/// Check if args have explicit output format specified
pub fn has_explicit_output(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "-o"
            || arg == "--output"
            || arg.starts_with("--output=")
            || arg.starts_with("-o=")
    })
}

/// Check if args have dry-run flag
pub fn has_dry_run_flag(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--dry-run" || arg.starts_with("--dry-run="))
}

/// Apply output profile to kubectl args
pub fn apply_output_profile(args: &mut Vec<String>, profile: crate::types::OutputProfile) {
    if has_explicit_output(args) {
        return;
    }

    if args.first().map(String::as_str) != Some("get") {
        return;
    }

    if let Some(value) = profile.as_output_value() {
        args.push("-o".to_string());
        args.push(value.to_string());
    }
}

/// Apply dry-run mode to kubectl args
pub fn apply_dry_run(args: &mut Vec<String>, enabled: bool) {
    if !enabled || args.is_empty() || has_dry_run_flag(args) {
        return;
    }

    let applies = matches!(
        args[0].as_str(),
        "apply" | "create" | "replace" | "patch" | "delete"
    );
    if !applies {
        return;
    }

    args.push("--dry-run=client".to_string());

    if matches!(args[0].as_str(), "apply" | "create" | "replace") && !has_explicit_output(args) {
        args.push("-o".to_string());
        args.push("yaml".to_string());
    }
}

/// Expand aliases in command args
pub fn expand_aliases(args: Vec<String>, aliases: &HashMap<String, String>) -> Result<Vec<String>, String> {
    if args.is_empty() {
        return Ok(args);
    }

    let Some(expansion) = aliases.get(&args[0]) else {
        return Ok(args);
    };

    expand_alias_template(expansion, &args[1..])
}

/// Parse placeholder index like {1}, {2}, etc.
pub fn parse_placeholder_index(token: &str) -> Option<usize> {
    if token.len() < 3 || !token.starts_with('{') || !token.ends_with('}') {
        return None;
    }

    let inner = &token[1..token.len() - 1];
    let idx = inner.parse::<usize>().ok()?;
    if idx == 0 {
        return None;
    }
    Some(idx)
}

/// Expand alias template with parameters
pub fn expand_alias_template(expansion: &str, params: &[String]) -> Result<Vec<String>, String> {
    let template_tokens = parse_command_line(expansion)?;
    let mut used_positions = std::collections::HashSet::new();
    let mut expanded: Vec<String> = Vec::new();
    let mut used_all = false;

    for token in template_tokens {
        if token == "{all}" {
            expanded.extend(params.iter().cloned());
            used_all = true;
            continue;
        }

        if let Some(idx) = parse_placeholder_index(&token) {
            let Some(value) = params.get(idx - 1) else {
                return Err(format!(
                    "Alias requires parameter {{{}}} but only {} argument(s) were supplied",
                    idx,
                    params.len()
                ));
            };

            expanded.push(value.clone());
            used_positions.insert(idx - 1);
            continue;
        }

        let mut rendered = token.clone();
        for idx in 1..=9 {
            let placeholder = format!("{{{idx}}}");
            if rendered.contains(&placeholder) {
                let Some(value) = params.get(idx - 1) else {
                    return Err(format!(
                        "Alias requires parameter {{{}}} but only {} argument(s) were supplied",
                        idx,
                        params.len()
                    ));
                };
                rendered = rendered.replace(&placeholder, value);
                used_positions.insert(idx - 1);
            }
        }

        expanded.push(rendered);
    }

    if !used_all {
        expanded.extend(
            params
                .iter()
                .enumerate()
                .filter(|(idx, _)| !used_positions.contains(idx))
                .map(|(_, value)| value.clone()),
        );
    }

    Ok(expanded)
}

/// Confirm delete with user
pub fn confirm_delete(args: &[String]) -> Result<bool, String> {
    let target = if args.len() > 1 {
        args[1..].join(" ")
    } else {
        "requested resource".to_string()
    };

    print!("Confirm delete '{}'? [y/N]: ", target);
    io::stdout()
        .flush()
        .map_err(|err| format!("Failed to flush stdout: {err}"))?;

    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|err| format!("Failed to read confirmation: {err}"))?;

    let answer = answer.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

/// Switch to a context with history
pub fn switch_context_with_history(target: &str, state: &mut ShellState) -> Result<(), String> {
    let current = current_context();
    if current == target {
        return Ok(());
    }

    set_context(target, state.show_commands)?;
    state.previous_context = Some(current);
    Ok(())
}

/// Switch to a namespace with history
pub fn switch_namespace_with_history(target: &str, state: &mut ShellState) -> Result<(), String> {
    let current = current_namespace();
    if current == target {
        return Ok(());
    }

    set_namespace(target, state.show_commands)?;
    state.previous_namespace = Some(current);
    Ok(())
}

/// Should confirm in risky context
pub fn should_confirm_in_risky_context(args: &[String]) -> bool {
    args.first().is_some_and(|cmd| {
        matches!(
            cmd.as_str(),
            "delete" | "apply" | "replace" | "patch" | "rollout" | "restart"
        )
    })
}

/// Confirm operation in risky context
pub fn confirm_risky_context(args: &[String], context: &str) -> Result<bool, String> {
    print!(
        "Risky context '{}' detected for command '{}'. Continue? [y/N]: ",
        context,
        args.join(" ")
    );
    io::stdout()
        .flush()
        .map_err(|err| format!("Failed to flush stdout: {err}"))?;

    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|err| format!("Failed to read confirmation: {err}"))?;

    let answer = answer.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

/// Select from list with filtering
pub fn select_from_list(prompt: &str, items: &[String]) -> Result<Option<String>, String> {
    if items.is_empty() {
        return Ok(None);
    }

    let mut query = String::new();

    loop {
        let visible: Vec<&String> = if query.is_empty() {
            items.iter().collect()
        } else {
            let query_lower = query.to_ascii_lowercase();
            items
                .iter()
                .filter(|item| item.to_ascii_lowercase().contains(&query_lower))
                .collect()
        };

        if visible.is_empty() {
            println!("No matches for '{}'.", query);
        } else {
            println!("{} matches{}:", visible.len(), if query.is_empty() { "" } else { " (filtered)" });
            for (idx, item) in visible.iter().take(25).enumerate() {
                println!("{:>2}. {}", idx + 1, item);
            }

            if visible.len() > 25 {
                println!("... {} more", visible.len() - 25);
            }
        }

        print!(
            "{} (number to select, text to filter, /clear to reset, empty to cancel): ",
            prompt
        );
        io::stdout()
            .flush()
            .map_err(|err| format!("Failed to flush stdout: {err}"))?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|err| format!("Failed to read selection: {err}"))?;

        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }

        if trimmed == "/clear" {
            query.clear();
            continue;
        }

        if let Ok(idx) = trimmed.parse::<usize>() {
            if idx == 0 || idx > visible.len().min(25) {
                return Err("Selection out of range".to_string());
            }
            return Ok(Some(visible[idx - 1].clone()));
        }

        query = trimmed.to_string();
    }
}

/// Pipe text to a command
pub fn pipe_text_to_command(cmd: &str, args: &[&str], text: &str) -> Result<(), String> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("Failed to start clipboard command '{}': {err}", cmd))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|err| format!("Failed to write clipboard content: {err}"))?;
    }

    let status = child
        .wait()
        .map_err(|err| format!("Failed to wait for clipboard command '{}': {err}", cmd))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("Clipboard command '{}' failed", cmd))
    }
}

/// Copy text to clipboard
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    if cfg!(target_os = "windows") {
        return pipe_text_to_command("cmd", &["/C", "clip"], text).map_err(|err| {
            format!(
                "{err}. Ensure 'clip' is available in PATH (normally provided by Windows)."
            )
        });
    }

    if cfg!(target_os = "macos") {
        return pipe_text_to_command("pbcopy", &[], text)
            .map_err(|err| format!("{err}. Ensure 'pbcopy' is available on macOS."));
    }

    if pipe_text_to_command("wl-copy", &[], text).is_ok() {
        return Ok(());
    }

    if pipe_text_to_command("xclip", &["-selection", "clipboard"], text).is_ok() {
        return Ok(());
    }

    if pipe_text_to_command("xsel", &["--clipboard", "--input"], text).is_ok() {
        return Ok(());
    }

    Err(
        "No supported clipboard tool found (tried wl-copy, xclip, xsel). Install wl-clipboard or xclip/xsel."
            .to_string(),
    )
}

// ============================================================================
// Built-in command handlers
// ============================================================================

/// Execute alias command
pub fn execute_alias_command(args: &[String], state: &ShellState) -> Result<(), String> {
    if args.len() == 1 || (args.len() == 2 && args[1] == "list") {
        if state.aliases.is_empty() {
            println!("No aliases configured.");
            return Ok(());
        }

        let mut entries: Vec<(&String, &String)> = state.aliases.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (name, expansion) in entries {
            println!("{} = {}", name, expansion);
        }
        return Ok(());
    }

    if args.len() >= 3 && args[1] == "test" {
        let Some(expansion) = state.aliases.get(&args[2]) else {
            return Err(format!("Alias '{}' not found", args[2]));
        };

        let rendered = expand_alias_template(expansion, &args[3..])?;
        if rendered.is_empty() {
            println!("(empty)");
        } else {
            println!("{}", rendered.join(" "));
        }
        return Ok(());
    }

    Err("Usage: alias [list] | alias test <name> [args...]".to_string())
}

/// Execute dryrun command
pub fn execute_dryrun_command(args: &[String], state: &mut ShellState) -> Result<(), String> {
    if args.len() == 1 || (args.len() == 2 && args[1] == "status") {
        println!("Dry-run mode: {}", if state.dry_run { "on" } else { "off" });
        return Ok(());
    }

    if args.len() != 2 {
        return Err("Usage: dryrun [on|off|status]".to_string());
    }

    match args[1].as_str() {
        "on" => {
            state.dry_run = true;
            println!("Dry-run mode enabled");
            Ok(())
        }
        "off" => {
            state.dry_run = false;
            println!("Dry-run mode disabled");
            Ok(())
        }
        "status" => {
            println!("Dry-run mode: {}", if state.dry_run { "on" } else { "off" });
            Ok(())
        }
        _ => Err("Usage: dryrun [on|off|status]".to_string()),
    }
}

/// Execute showcmd command
pub fn execute_showcmd_command(args: &[String], state: &mut ShellState) -> Result<(), String> {
    if args.len() == 1 || (args.len() == 2 && args[1] == "status") {
        println!(
            "Command display: {}",
            if state.show_commands { "on" } else { "off" }
        );
        return Ok(());
    }

    if args.len() != 2 {
        return Err("Usage: showcmd [on|off|status]".to_string());
    }

    match args[1].as_str() {
        "on" => {
            state.show_commands = true;
            println!("Command display enabled");
            Ok(())
        }
        "off" => {
            state.show_commands = false;
            println!("Command display disabled");
            Ok(())
        }
        "status" => {
            println!(
                "Command display: {}",
                if state.show_commands { "on" } else { "off" }
            );
            Ok(())
        }
        _ => Err("Usage: showcmd [on|off|status]".to_string()),
    }
}

/// Execute help command
pub fn execute_help_command(args: &[String]) -> Result<(), String> {
    if args.len() == 1 {
        println!("kube-shell built-ins:");
        println!("  !!                          Re-run previous command");
        println!("  help [topic]                Show built-in help");
        println!("  ns <namespace>|-            Switch namespace or previous namespace");
        println!("  ctx <context>|-             Switch context or previous context");
        println!("  use <ctx>/<ns>              Switch context/namespace in one command");
        println!("  bookmark|b ...              Manage/use bookmarks");
        println!("  alias [list|test]           Inspect and test aliases");
        println!("  view [profile]              Set output profile for get commands");
        println!("  dryrun [on|off|status]      Toggle automatic dry-run mode");
        println!("  showcmd [on|off|status]     Show full kubectl command before execution");
        println!("  trace [on|off|status]       Alias for showcmd");
        println!("  pick ...                    Select resource names (supports --run/--copy)");
        println!("  restart ...                 Rollout restart + status helper");
        println!("  tail ...                    Follow pod/deployment logs");
        println!("  logs --multi|--pick         Multi-pod fuzzy log streaming with per-pod colors");
        println!("  exit, quit                  Leave kube-shell");
        return Ok(());
    }

    if args.len() != 2 {
        return Err("Usage: help [topic]".to_string());
    }

    match args[1].as_str() {
        "pick" => {
            println!("pick usage:");
            println!("  pick <resource> [namespace] [--run <template>] [--copy]");
            println!("  template placeholders: {{1}}/{{name}}, {{ns}}");
        }
        "alias" => {
            println!("alias usage:");
            println!("  alias list");
            println!("  alias test <name> [args...]");
            println!("  config placeholders: {{1}}, {{2}}, ..., {{all}}");
        }
        "bookmark" | "b" => {
            println!("bookmark usage:");
            println!("  bookmark add <name> <ctx>/<ns>");
            println!("  bookmark use <name>");
            println!("  bookmark list");
            println!("  bookmark remove <name>");
            println!("  b <name> is shorthand for bookmark use <name>");
        }
        "dryrun" => {
            println!("dryrun usage:");
            println!("  dryrun [on|off|status]");
        }
        "logs" => {
            println!("logs multi-stream usage:");
            println!("  logs --multi");
            println!("  logs --pick");
            println!("  optional flags still apply, e.g. -n/--namespace, -c/--container, --tail=100");
            println!("  formatting flags: --no-ts, --no-align");
            println!("  filtering flags: --include <pattern>, --exclude <pattern>");
            println!("  context flags: --before <N>, --after <N>");
            println!("  matching flags: --ignore-case, --regex");
        }
        "showcmd" | "trace" => {
            println!("showcmd usage:");
            println!("  showcmd [on|off|status]");
            println!("  trace [on|off|status]");
        }
        _ => {
            println!("No detailed help for '{}'. Try 'help' for command list.", args[1]);
        }
    }

    Ok(())
}

/// Render prompt template
pub fn render_prompt(template: &str, risk: &str, context: &str, namespace: &str) -> String {
    template
        .replace("{risk}", risk)
        .replace("{context}", context)
        .replace("{namespace}", namespace)
}

/// Parse use target (context/namespace format)
pub fn parse_use_target(value: &str) -> Result<(Option<String>, Option<String>), String> {
    if let Some((ctx, ns)) = value.split_once('/') {
        let context = if ctx.trim().is_empty() {
            None
        } else {
            Some(ctx.trim().to_string())
        };
        let namespace = if ns.trim().is_empty() {
            None
        } else {
            Some(ns.trim().to_string())
        };

        if context.is_none() && namespace.is_none() {
            return Err("Usage: use <context> [namespace] | use <context>/<namespace> | use /<namespace>".to_string());
        }

        return Ok((context, namespace));
    }

    if value.trim().is_empty() {
        return Err("Usage: use <context> [namespace] | use <context>/<namespace> | use /<namespace>".to_string());
    }

    Ok((Some(value.trim().to_string()), None))
}

/// Execute use command to switch context/namespace
pub fn execute_use_command_with_state(args: &[String], state: &mut ShellState) -> Result<(), String> {
    let usage = "Usage: use <context> [namespace] | use <context>/<namespace> | use /<namespace>";

    if args.len() < 2 {
        return Err(usage.to_string());
    }

    let (context, namespace) = if args.len() == 2 {
        parse_use_target(&args[1])?
    } else if args.len() == 3 {
        let context = if args[1].trim().is_empty() {
            None
        } else {
            Some(args[1].trim().to_string())
        };
        let namespace = if args[2].trim().is_empty() {
            None
        } else {
            Some(args[2].trim().to_string())
        };
        (context, namespace)
    } else {
        return Err(usage.to_string());
    };

    if let Some(ctx) = context.as_deref() {
        switch_context_with_history(ctx, state)?;
    }

    if let Some(ns) = namespace.as_deref() {
        switch_namespace_with_history(ns, state)?;
    }

    if context.is_none() && namespace.is_none() {
        return Err(usage.to_string());
    }

    Ok(())
}

/// Execute view command to set output profile
pub fn execute_view_command(args: &[String], state: &mut ShellState) -> Result<(), String> {
    use crate::types::OutputProfile;

    if args.len() == 1 {
        println!("Current output profile: {}", state.output_profile.label());
        return Ok(());
    }

    if args.len() != 2 {
        return Err("Usage: view [default|none|wide|yaml|json]".to_string());
    }

    let Some(profile) = OutputProfile::from_name(args[1].as_str()) else {
        return Err("Usage: view [default|none|wide|yaml|json]".to_string());
    };

    state.output_profile = profile;
    println!("Output profile set to {}", state.output_profile.label());
    Ok(())
}

// Bookmark and picker commands are in their own section below...
/// Execute bookmark command
pub fn execute_bookmark_command(args: &[String], state: &mut ShellState) -> Result<(), String> {
    if args.len() < 2 {
        return Err("Usage: bookmark <add|use|list|remove> ...".to_string());
    }

    match args[1].as_str() {
        "list" => {
            if state.bookmarks.is_empty() {
                println!("No bookmarks defined.");
                return Ok(());
            }

            let mut items: Vec<(&String, &String)> = state.bookmarks.iter().collect();
            items.sort_by(|a, b| a.0.cmp(b.0));
            for (name, target) in items {
                println!("{name} -> {target}");
            }
            Ok(())
        }
        "add" => {
            if args.len() < 4 {
                return Err(
                    "Usage: bookmark add <name> <context>/<namespace>|<context>|/<namespace>|<context> <namespace>"
                        .to_string(),
                );
            }

            let name = args[2].trim();
            if name.is_empty() {
                return Err("Bookmark name cannot be empty".to_string());
            }

            let target = if args.len() == 4 {
                args[3].trim().to_string()
            } else if args.len() == 5 {
                format!("{}/{}", args[3].trim(), args[4].trim())
            } else {
                return Err(
                    "Usage: bookmark add <name> <context>/<namespace>|<context>|/<namespace>|<context> <namespace>"
                        .to_string(),
                );
            };

            parse_use_target(&target)?;
            state.bookmarks.insert(name.to_string(), target.clone());
            save_bookmarks(&state.bookmarks_file, &state.bookmarks)?;
            println!("Bookmark '{name}' saved as {target}");
            Ok(())
        }
        "use" => {
            if args.len() != 3 {
                return Err("Usage: bookmark use <name>".to_string());
            }

            let Some(target) = state.bookmarks.get(&args[2]).cloned() else {
                return Err(format!("Bookmark '{}' not found", args[2]));
            };

            execute_use_command_with_state(&["use".to_string(), target.clone()], state)?;
            println!("Applied bookmark '{}' ({target})", args[2]);
            Ok(())
        }
        "remove" | "rm" | "delete" => {
            if args.len() != 3 {
                return Err("Usage: bookmark remove <name>".to_string());
            }

            if state.bookmarks.remove(&args[2]).is_some() {
                save_bookmarks(&state.bookmarks_file, &state.bookmarks)?;
                println!("Removed bookmark '{}'", args[2]);
            } else {
                return Err(format!("Bookmark '{}' not found", args[2]));
            }
            Ok(())
        }
        _ => Err("Usage: bookmark <add|use|list|remove> ...".to_string()),
    }
}

/// Execute bookmark shortcut (b command)
pub fn execute_bookmark_shortcut(args: &[String], state: &mut ShellState) -> Result<(), String> {
    if args.len() < 2 {
        return Err("Usage: b <bookmark-name> | b <add|use|list|remove> ...".to_string());
    }

    let bookmark_subcommands = ["add", "use", "list", "remove", "rm", "delete"];
    let mapped: Vec<String> = if bookmark_subcommands.contains(&args[1].as_str()) {
        let mut v = vec!["bookmark".to_string()];
        v.extend(args.iter().skip(1).cloned());
        v
    } else {
        if args.len() != 2 {
            return Err("Usage: b <bookmark-name> | b <add|use|list|remove> ...".to_string());
        }
        vec!["bookmark".to_string(), "use".to_string(), args[1].clone()]
    };

    execute_bookmark_command(&mapped, state)
}

/// Execute pick command
pub fn execute_pick_command_with_state(args: &[String], state: &mut ShellState) -> Result<(), String> {
    if args.len() < 2 {
        return Err("Usage: pick <resource> [namespace] [--run <template>] [--copy]".to_string());
    }

    let resource = args[1].as_str();
    let mut namespace_arg: Option<&str> = None;
    let mut run_template: Option<&str> = None;
    let mut copy_selected = false;

    let mut idx = 2;
    while idx < args.len() {
        if args[idx] == "--run" {
            if idx + 1 >= args.len() {
                return Err("Usage: pick <resource> [namespace] [--run <template>] [--copy]".to_string());
            }
            run_template = Some(args[idx + 1].as_str());
            idx += 2;
            continue;
        }

        if args[idx] == "--copy" {
            copy_selected = true;
            idx += 1;
            continue;
        }

        if namespace_arg.is_none() {
            namespace_arg = Some(args[idx].as_str());
            idx += 1;
            continue;
        }

        return Err("Usage: pick <resource> [namespace] [--run <template>] [--copy]".to_string());
    }

    let fallback_namespace = if namespace_arg.is_some() {
        None
    } else {
        Some(current_namespace())
    };
    let namespace = namespace_arg.or_else(|| fallback_namespace.as_deref());

    let items = kubectl_object_names(resource, namespace);
    if items.is_empty() {
        println!("No {} found.", resource);
        return Ok(());
    }

    if let Some(selected) = select_from_list("Pick", &items)? {
        if copy_selected {
            copy_to_clipboard(&selected)?;
            println!("Copied '{}' to clipboard", selected);
        }

        if let Some(template) = run_template {
            let ns_value = namespace.unwrap_or("");
            let command = template
                .replace("{1}", &selected)
                .replace("{name}", &selected)
                .replace("{ns}", ns_value);
            execute_kubectl_command(&command, state)?;
        } else if !copy_selected {
            println!("{}", selected);
        }
    }

    Ok(())
}

/// Execute restart command
pub fn execute_restart_command(args: &[String], state: &ShellState) -> Result<(), String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("Usage: restart <deployment> | restart <resource> <name>".to_string());
    }

    let target = if args.len() == 2 {
        format!("deployment/{}", args[1])
    } else {
        format!("{}/{}", args[1], args[2])
    };

    run_kubectl_args(&[
        "rollout".to_string(),
        "restart".to_string(),
        target.clone(),
    ], state.show_commands)?;
    run_kubectl_args(&[
        "rollout".to_string(),
        "status".to_string(),
        target,
    ], state.show_commands)
}

/// Get selector for a deployment
pub fn deployment_selector(deploy_name: &str, namespace: &str) -> Result<String, String> {
    run_kubectl_capture(&[
        "get",
        "deployment",
        deploy_name,
        "-n",
        namespace,
        "-o",
        "jsonpath={range $k,$v := .spec.selector.matchLabels}{$k}={$v},{end}",
    ])
    .map(|value| value.trim_end_matches(',').to_string())
}

/// Get pods for a selector
pub fn pods_for_selector(namespace: &str, selector: &str) -> Vec<String> {
    if selector.is_empty() {
        return Vec::new();
    }

    let args: Vec<String> = vec![
        "get".to_string(),
        "pods".to_string(),
        "-n".to_string(),
        namespace.to_string(),
        "-l".to_string(),
        selector.to_string(),
        "-o".to_string(),
        "jsonpath={range .items[*]}{.metadata.name}{\"\\n\"}{end}".to_string(),
    ];
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    kubectl_lines(&refs)
}

/// Execute tail command to follow logs
pub fn execute_tail_command(args: &[String], state: &ShellState) -> Result<(), String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("Usage: tail <pod> | tail deploy <name>".to_string());
    }

    let namespace = current_namespace();

    let pod = if args.len() == 2 {
        args[1].clone()
    } else if matches!(args[1].as_str(), "deploy" | "deployment" | "deployments") {
        let selector = deployment_selector(&args[2], &namespace)?;
        let pods = pods_for_selector(&namespace, &selector);
        if pods.is_empty() {
            return Err(format!(
                "No pods found for deployment '{}' in namespace '{}'",
                args[2], namespace
            ));
        }
        if pods.len() == 1 {
            pods[0].clone()
        } else {
            select_from_list("Select pod for logs", &pods)?
                .ok_or_else(|| "Tail cancelled".to_string())?
        }
    } else {
        return Err("Usage: tail <pod> | tail deploy <name>".to_string());
    };

    run_kubectl_args(&[
        "logs".to_string(),
        "-f".to_string(),
        pod,
        "-n".to_string(),
        namespace,
    ], state.show_commands)
}

/// Main command execution router
pub fn execute_kubectl_command(input: &str, state: &mut ShellState) -> Result<(), String> {
    let mut args = parse_command_line(input)?;
    args = expand_aliases(args, &state.aliases)?;

    if args.is_empty() {
        return Ok(());
    }

    if args[0] == "kubectl" {
        args.remove(0);
    }

    if args.is_empty() {
        return Ok(());
    }

    // Handle built-in commands
    if args[0] == "ns" || args[0] == "namespace" {
        if args.len() < 2 {
            return Err("Usage: ns <namespace>".to_string());
        }

        if args[1] == "-" {
            let Some(previous) = state.previous_namespace.clone() else {
                return Err("No previous namespace to switch to".to_string());
            };
            return switch_namespace_with_history(&previous, state);
        }

        return switch_namespace_with_history(&args[1], state);
    }

    if args[0] == "ctx" || args[0] == "context" {
        if args.len() < 2 {
            return Err("Usage: ctx <context>".to_string());
        }

        if args[1] == "-" {
            let Some(previous) = state.previous_context.clone() else {
                return Err("No previous context to switch to".to_string());
            };
            return switch_context_with_history(&previous, state);
        }

        return switch_context_with_history(&args[1], state);
    }

    if args[0] == "use" || args[0] == "switch" {
        return execute_use_command_with_state(&args, state);
    }

    if args[0] == "help" {
        return execute_help_command(&args);
    }

    if args[0] == "view" {
        return execute_view_command(&args, state);
    }

    if args[0] == "alias" {
        return execute_alias_command(&args, state);
    }

    if args[0] == "dryrun" {
        return execute_dryrun_command(&args, state);
    }

    if args[0] == "showcmd" || args[0] == "trace" {
        return execute_showcmd_command(&args, state);
    }

    if args[0] == "bookmark" {
        return execute_bookmark_command(&args, state);
    }

    if args[0] == "b" {
        return execute_bookmark_shortcut(&args, state);
    }

    if args[0] == "pick" {
        return execute_pick_command_with_state(&args, state);
    }

    if args[0] == "restart" {
        return execute_restart_command(&args, state);
    }

    if args[0] == "tail" {
        return execute_tail_command(&args, state);
    }

    if args[0] == "logs" && args.iter().any(|arg| arg == "--multi" || arg == "--pick") {
        return crate::multi_logs::execute_multi_logs_command(&args, state.show_commands, current_namespace());
    }

    // Handle kubectl commands
    let context = current_context();
    if state.risky_contexts.contains(&context)
        && should_confirm_in_risky_context(&args)
        && !args.iter().any(|arg| arg == "--yes")
        && !confirm_risky_context(&args, &context)?
    {
        println!("Command cancelled.");
        return Ok(());
    }

    apply_output_profile(&mut args, state.output_profile);
    apply_dry_run(&mut args, state.dry_run);

    if state.safe_delete
        && args.first().map(String::as_str) == Some("delete")
        && !args.iter().any(|arg| arg == "--yes")
        && !confirm_delete(&args)?
    {
        println!("Delete cancelled.");
        return Ok(());
    }

    run_kubectl_args(&args, state.show_commands)
}
