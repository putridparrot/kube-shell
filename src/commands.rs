use std::collections::HashMap;
use std::io::{self, Write};
use std::process::{Command, Stdio};
use serde::Serialize;

use crate::kubectl::*;
use crate::types::ShellState;
use crate::interrupt::ForegroundCommandGuard;

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
    let current = state.current_context.clone();
    if current == target {
        return Ok(());
    }

    set_context(target, state.show_commands)?;
    state.previous_context = Some(current);
    state.current_context = target.to_string();
    state.current_namespace = current_namespace();
    Ok(())
}

/// Switch to a namespace with history
pub fn switch_namespace_with_history(target: &str, state: &mut ShellState) -> Result<(), String> {
    let current = effective_namespace(state);
    if current == target {
        return Ok(());
    }

    if state.session_namespace_mode {
        state.session_namespace = Some(target.to_string());
    } else {
        set_namespace(target, state.show_commands)?;
        state.current_namespace = target.to_string();
    }
    state.previous_namespace = Some(current);
    Ok(())
}

/// Resolve the context this shell should use for commands and prompt.
pub fn effective_context(state: &ShellState) -> String {
    state.current_context.clone()
}

/// Resolve the namespace this shell should use for commands and prompt.
pub fn effective_namespace(state: &ShellState) -> String {
    if state.session_namespace_mode {
        state
            .session_namespace
            .clone()
            .unwrap_or_else(|| state.current_namespace.clone())
    } else {
        state.current_namespace.clone()
    }
}

fn has_all_namespaces_flag(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "-A" || arg == "--all-namespaces")
}

fn command_supports_default_namespace(command: &str) -> bool {
    matches!(
        command,
        "get"
            | "describe"
            | "logs"
            | "exec"
            | "edit"
            | "delete"
            | "apply"
            | "create"
            | "replace"
            | "patch"
            | "rollout"
            | "scale"
            | "annotate"
            | "label"
            | "wait"
            | "top"
            | "port-forward"
    )
}

fn apply_default_namespace(args: &mut Vec<String>, state: &ShellState) {
    if !state.session_namespace_mode || args.is_empty() {
        return;
    }

    if explicit_namespace_from_args(args).is_some() || has_all_namespaces_flag(args) {
        return;
    }

    if !command_supports_default_namespace(args[0].as_str()) {
        return;
    }

    let namespace = effective_namespace(state);
    args.push("-n".to_string());
    args.push(namespace);
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

// ============================================================================
// Built-in command handlers
// ============================================================================

/// Execute alias command
pub fn execute_alias_command(args: &[String], state: &mut ShellState) -> Result<(), String> {
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

    if args.len() >= 2 && args[1] == "remove" {
        if args.len() != 3 {
            return Err("Usage: alias remove <name>".to_string());
        }

        if state.aliases.remove(&args[2]).is_some() {
            println!("Alias '{}' removed.", args[2]);
            return Ok(());
        }

        return Err(format!("Alias '{}' not found", args[2]));
    }

    if args.len() >= 2 && args[1] == "add" {
        if args.len() < 4 {
            return Err("Usage: alias add <name> <expansion...>".to_string());
        }

        let name = args[2].trim().to_string();
        let expansion = args[3..].join(" ").trim().to_string();

        if name.is_empty() || expansion.is_empty() {
            return Err("Usage: alias add <name> <expansion...>".to_string());
        }

        state.aliases.insert(name.clone(), expansion.clone());
        println!("Alias '{}' = '{}' saved.", name, expansion);
        return Ok(());
    }

    if args.len() >= 2 {
        let mut split = args[1].splitn(2, '=');
        if let (Some(name_raw), Some(first_expansion)) = (split.next(), split.next()) {
            let name = name_raw.trim().to_string();
            let mut expansion_parts: Vec<String> = Vec::new();

            if !first_expansion.trim().is_empty() {
                expansion_parts.push(first_expansion.trim().to_string());
            }

            if args.len() > 2 {
                expansion_parts.extend(args[2..].iter().cloned());
            }

            let expansion = expansion_parts.join(" ").trim().to_string();

            if name.is_empty() || expansion.is_empty() {
                return Err("Usage: alias <name>=<expansion...>".to_string());
            }

            state.aliases.insert(name.clone(), expansion.clone());
            println!("Alias '{}' = '{}' saved.", name, expansion);
            return Ok(());
        }
    }

    Err(
        "Usage: alias [list] | alias <name>=<expansion...> | alias add <name> <expansion...> | alias remove <name> | alias test <name> [args...]".to_string(),
    )
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

/// Execute trace command
pub fn execute_trace_command(args: &[String], state: &mut ShellState) -> Result<(), String> {
    if args.len() == 1 || (args.len() == 2 && args[1] == "status") {
        println!(
            "Command display: {}",
            if state.show_commands { "on" } else { "off" }
        );
        return Ok(());
    }

    if args.len() != 2 {
        return Err("Usage: trace [on|off|status]".to_string());
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
        _ => Err("Usage: trace [on|off|status]".to_string()),
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
        println!("  session_namespace_mode=true Keep ns changes local to this shell session");
        println!("  alias <name>=<expansion>    Add/update an alias");
        println!("  alias add <name> <exp...>   Add/update an alias");
        println!("  alias remove <name>         Remove an alias");
        println!("  alias [list|test]           Inspect and test aliases");
        println!("  view [profile]              Set output profile for get commands");
        println!("  dryrun [on|off|status]      Toggle automatic dry-run mode");
        println!("  trace [on|off|status]       Show full kubectl command before execution");
        println!("  restart ...                 Rollout restart + status helper");
        println!("  restart-reason ...          Show probable pod restart causes");
        println!("  port-forward ... --browse   Open browser for forwarded localhost port");
        println!("  tail ...                    Follow pod/deployment logs");
        println!("  logs --multi|--pick         Multi-pod fuzzy log streaming with per-pod colors");
        println!("  jobs                        List background jobs");
        println!("  fg <id>                     Bring background job to foreground");
        println!("  job kill <id>               Kill a background job");
        println!("  job clean                   Remove finished jobs from list");
        println!("  <command> &                 Run any kubectl command in the background");
        println!("  ask <question>              Ask the AI a Kubernetes question");
        println!("  ai status                   Show AI configuration and connectivity");
        println!("  ai model <name>             Switch the active AI model at runtime");
        println!("  help ai                     Show AI prompt/template customization help");
        println!("  ai explain <args...>        Run kubectl <args> and explain the output");
        println!("  <kubectl cmd> | explain     Pipe kubectl output through AI for explanation");
        println!("  exit, quit                  Leave kube-shell");
        return Ok(());
    }

    if args.len() != 2 {
        return Err("Usage: help [topic]".to_string());
    }

    match args[1].as_str() {
        "alias" => {
            println!("alias usage:");
            println!("  alias list");
            println!("  alias <name>=<expansion>");
            println!("  alias add <name> <expansion...>");
            println!("  alias remove <name>");
            println!("  alias test <name> [args...]");
            println!("  aliases are persisted in .kube-shell-aliases");
            println!("  alias ops=use prod-cluster/kube-system");
            println!("  config placeholders: {{1}}, {{2}}, ..., {{all}}");
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
            println!("  time range flags: --from <time>, --to <time> (disables follow)");
            println!("  time formats: HH:MM, YYYY-MM-DD, YYYY-MM-DD HH:MM, RFC3339");
            println!("  formatting flags: --no-ts, --no-align");
            println!("  filtering flags: --include <pattern>, --exclude <pattern>");
            println!("  context flags: --before <N>, --after <N>");
            println!("  matching flags: --ignore-case, --regex");
        }
        "restart-reason" => {
            println!("restart-reason usage:");
            println!("  restart-reason <pod-name>");
            println!("  restart-reason pod/<name>");
            println!("  restart-reason <resource>/<name>");
            println!("  restart-reason <resource> <name>");
            println!("  options: -n|--namespace <ns>, --all");
            println!("           --logs [--tail N] [--since DURATION]");
            println!("           -o|--output table|json|markdown");
            println!("  resources: deployment, statefulset, daemonset, replicaset, pod");
            println!("  notes: exit code hints include common causes such as OOMKill (137)");
        }
        "trace" => {
            println!("  trace [on|off|status]");
        }
        "port-forward" => {
            println!("port-forward helper option:");
            println!("  port-forward <target> <local:remote> --browse");
            println!("  port-forward <target> <local:remote> --browse-scheme https");
            println!("  kubectl port-forward <target> <local:remote> --browse");
            println!("  --browse-scheme accepts http|https (defaults to http)");
            println!("  opens <scheme>://localhost:<localPort>");
            println!("  Tip: append & to run in the background:");
            println!("    port-forward svc/my-service 8080:80 &");
        }
        "jobs" => {
            println!("Background job management:");
            println!("  jobs              List all background jobs");
            println!("  fg <id>           Stream output of job <id>; Ctrl+C returns to shell");
            println!("  job kill <id>     Terminate job <id>");
            println!("  job clean         Remove finished jobs from the list");
            println!("  <command> &       Run any kubectl command in the background");
        }
        "ai" => {
            println!("AI usage:");
            println!("  ask <question>");
            println!("  ai status");
            println!("  ai model <name>");
            println!("  ai explain <kubectl args...>");
            println!("  <kubectl cmd> | explain");
            println!("Config keys in .kube-shellrc:");
            println!("  ai_url=<ollama-url>");
            println!("  ai_model=<model-name>");
            println!("  ai_ask_prompt_template=<single-line template>");
            println!("  ai_explain_prompt_template=<single-line template>");
            println!("  session_namespace_mode=<true|false>");
            println!("Template placeholders:");
            println!("  ask: {{question}}, {{context}}, {{namespace}}");
            println!("  explain: {{output}}, {{command}}, {{context}}, {{namespace}}");
            println!("Use escaped newlines for readability, e.g. \\n in config values.");
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

fn resolve_pod_name(namespace: &str, input: &str) -> Result<String, String> {
    if input.is_empty() {
        return Err("Pod name cannot be empty".to_string());
    }

    let pods = kubectl_object_names("pods", Some(namespace));
    if pods.is_empty() {
        return Err(format!("No pods found in namespace '{}'", namespace));
    }

    if let Some(exact) = pods.iter().find(|pod| pod.as_str() == input) {
        return Ok(exact.clone());
    }

    let input_lower = input.to_ascii_lowercase();
    let matches: Vec<String> = pods
        .into_iter()
        .filter(|pod| pod.to_ascii_lowercase().contains(&input_lower))
        .collect();

    match matches.len() {
        0 => Err(format!(
            "No pods matched '{}' in namespace '{}'",
            input, namespace
        )),
        1 => Ok(matches[0].clone()),
        _ => select_from_list(
            &format!("Multiple pods matched '{}', select pod", input),
            &matches,
        )?
        .ok_or_else(|| "Tail cancelled".to_string()),
    }
}

fn split_pipeline(args: &[String]) -> Result<Vec<Vec<String>>, String> {
    let mut stages: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();

    for token in args {
        if token == "|" {
            if current.is_empty() {
                return Err("Missing command between pipe operators".to_string());
            }
            stages.push(current);
            current = Vec::new();
        } else {
            current.push(token.clone());
        }
    }

    if current.is_empty() {
        return Err("Pipe command is missing after '|'".to_string());
    }

    stages.push(current);
    Ok(stages)
}

fn run_pipeline_capture(stages: &[Vec<String>], show_commands: bool) -> Result<String, String> {
    if stages.is_empty() {
        return Err("No command to execute".to_string());
    }

    if stages.iter().any(|s| s.is_empty()) {
        return Err("Invalid empty command in pipeline".to_string());
    }

    if show_commands {
        let rendered = stages
            .iter()
            .map(|s| {
                s.iter()
                    .map(|a| format!("{:?}", a))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join(" | ");
        println!("+ {rendered}");
    }

    let _guard = ForegroundCommandGuard::new();

    let mut previous_stdout: Option<std::process::ChildStdout> = None;
    let mut all_children: Vec<std::process::Child> = Vec::new();

    for (idx, stage) in stages.iter().enumerate() {
        let mut command = Command::new(&stage[0]);
        command.args(stage[1..].iter().map(String::as_str));
        command.stderr(Stdio::inherit());

        if idx == 0 {
            command.stdin(Stdio::inherit());
        } else {
            let stdin = previous_stdout
                .take()
                .ok_or_else(|| "Failed to connect pipeline stdin".to_string())?;
            command.stdin(Stdio::from(stdin));
        }

        command.stdout(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|err| format!("Failed to spawn '{}': {err}", stage[0]))?;
        previous_stdout = child.stdout.take();
        all_children.push(child);
    }

    let last_stdout = previous_stdout
        .ok_or_else(|| "No output stream available".to_string())?;

    use std::io::Read;
    let mut output = String::new();
    std::io::BufReader::new(last_stdout)
        .read_to_string(&mut output)
        .map_err(|e| format!("Failed to read command output: {e}"))?;

    for mut child in all_children {
        child
            .wait()
            .map_err(|e| format!("Failed waiting for command: {e}"))?;
    }

    Ok(output)
}

fn run_command_pipeline(stages: &[Vec<String>], show_commands: bool) -> Result<(), String> {
    if stages.is_empty() {
        return Err("No command to execute".to_string());
    }

    if stages.iter().any(|stage| stage.is_empty()) {
        return Err("Invalid empty command in pipeline".to_string());
    }

    if show_commands {
        let rendered = stages
            .iter()
            .map(|stage| {
                stage
                    .iter()
                    .map(|arg| format!("{:?}", arg))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join(" | ");
        println!("+ {rendered}");
    }

    let mut children: Vec<std::process::Child> = Vec::new();
    let mut previous_stdout: Option<std::process::ChildStdout> = None;

    let _guard = ForegroundCommandGuard::new();


    for (idx, stage) in stages.iter().enumerate() {
        let mut command = Command::new(&stage[0]);
        command.args(stage[1..].iter().map(String::as_str));
        command.stderr(Stdio::inherit());

        if idx == 0 {
            command.stdin(Stdio::inherit());
        } else {
            let stdin = previous_stdout
                .take()
                .ok_or_else(|| "Failed to connect pipeline stdin".to_string())?;
            command.stdin(Stdio::from(stdin));
        }

        if idx + 1 < stages.len() {
            command.stdout(Stdio::piped());
        } else {
            command.stdout(Stdio::inherit());
        }

        let mut child = command
            .spawn()
            .map_err(|err| format!("Failed to execute command '{}': {err}", stage[0]))?;

        if idx + 1 < stages.len() {
            previous_stdout = child.stdout.take();
        }

        children.push(child);
    }

    let mut statuses = Vec::with_capacity(children.len());
    for mut child in children {
        statuses.push(
            child
                .wait()
                .map_err(|err| format!("Failed waiting for command: {err}"))?,
        );
    }

    for (idx, status) in statuses.iter().enumerate() {
        if !status.success() {
            return Err(format!(
                "Command '{}' exited with status: {}",
                stages[idx][0], status
            ));
        }
    }

    Ok(())
}

fn execute_prefixed_kubectl_command(args: &[String], show_commands: bool) -> Result<(), String> {
    if args.is_empty() {
        return Err("Usage: kubectl <args...>".to_string());
    }

    let stages = split_pipeline(args)?;

    if stages.len() == 1
        && stages[0]
            .first()
            .is_some_and(|cmd| cmd == "port-forward")
    {
        return execute_port_forward_with_optional_browse(&stages[0], show_commands);
    }

    let mut pipeline: Vec<Vec<String>> = Vec::with_capacity(stages.len());

    let mut first = vec!["kubectl".to_string()];
    first.extend(stages[0].clone());
    pipeline.push(first);

    for stage in stages.into_iter().skip(1) {
        pipeline.push(stage);
    }

    run_command_pipeline(&pipeline, show_commands)
}

fn port_forward_flag_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "-n"
            | "--namespace"
            | "--context"
            | "--kubeconfig"
            | "--address"
            | "--pod-running-timeout"
    )
}

fn normalize_browse_scheme(value: &str) -> Result<String, String> {
    match value {
        "http" | "https" => Ok(value.to_string()),
        _ => Err("--browse-scheme must be 'http' or 'https'".to_string()),
    }
}

fn strip_browse_flag(args: &[String]) -> Result<(Vec<String>, bool, String), String> {
    let mut stripped: Vec<String> = Vec::with_capacity(args.len());
    let mut browse = false;
    let mut scheme = "http".to_string();
    let mut idx = 0;

    while idx < args.len() {
        let arg = args[idx].as_str();

        if arg == "--browse" {
            browse = true;
            idx += 1;
            continue;
        }

        if arg == "--browse-scheme" {
            if idx + 1 >= args.len() {
                return Err("--browse-scheme requires a value: http or https".to_string());
            }

            scheme = normalize_browse_scheme(&args[idx + 1])?;
            browse = true;
            idx += 2;
            continue;
        }

        if let Some(value) = arg.strip_prefix("--browse-scheme=") {
            scheme = normalize_browse_scheme(value)?;
            browse = true;
            idx += 1;
            continue;
        }

        stripped.push(args[idx].clone());
        idx += 1;
    }

    Ok((stripped, browse, scheme))
}

fn port_forward_local_port(args: &[String]) -> Option<u16> {
    if args.first().map(String::as_str) != Some("port-forward") {
        return None;
    }

    let mut i = 1;
    let mut skip_next = false;
    let mut positional_count = 0;

    while i < args.len() {
        let token = args[i].as_str();

        if skip_next {
            skip_next = false;
            i += 1;
            continue;
        }

        if token.starts_with('-') {
            if port_forward_flag_takes_value(token) {
                skip_next = true;
            }
            i += 1;
            continue;
        }

        positional_count += 1;
        if positional_count == 2 {
            let local = token.split(':').next().unwrap_or(token).trim();
            return local.parse::<u16>().ok();
        }

        i += 1;
    }

    None
}

fn execute_port_forward_with_optional_browse(args: &[String], show_commands: bool) -> Result<(), String> {
    let (sanitized_args, browse, scheme) = strip_browse_flag(args)?;

    if !browse {
        return run_kubectl_args(&sanitized_args, show_commands);
    }

    let Some(port) = port_forward_local_port(&sanitized_args) else {
        return Err(
            "--browse requires an explicit local port mapping, e.g. port-forward svc/api 8080:80 --browse"
                .to_string(),
        );
    };

    if show_commands {
        print_kubectl_command(&sanitized_args);
    }

    let _guard = ForegroundCommandGuard::new();
    let mut child = Command::new("kubectl")
        .args(sanitized_args.iter().map(String::as_str))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|err| format!("Failed to execute kubectl: {err}"))?;

    let url = format!("{scheme}://localhost:{port}");
    match webbrowser::open(&url) {
        Ok(_) => println!("Opened {url}"),
        Err(err) => eprintln!("Unable to open browser for {url}: {err}"),
    }

    let status = child
        .wait()
        .map_err(|err| format!("Failed waiting for kubectl: {err}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("kubectl exited with status: {status}"))
    }
}

fn logs_flag_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "-n"
            | "--namespace"
            | "-c"
            | "--container"
            | "--context"
            | "--kubeconfig"
            | "--since"
            | "--since-time"
            | "--tail"
            | "--limit-bytes"
            | "--max-log-requests"
            | "-l"
            | "--selector"
    )
}

fn explicit_namespace_from_args(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-n" || args[i] == "--namespace" {
            if i + 1 < args.len() {
                return Some(args[i + 1].clone());
            }
            return None;
        }

        if let Some(ns) = args[i].strip_prefix("--namespace=")
            && !ns.is_empty()
        {
            return Some(ns.to_string());
        }

        if let Some(ns) = args[i].strip_prefix("-n=")
            && !ns.is_empty()
        {
            return Some(ns.to_string());
        }

        i += 1;
    }

    None
}

fn first_logs_target_index(args: &[String]) -> Option<usize> {
    let mut i = 1;
    let mut skip_next = false;

    while i < args.len() {
        let token = args[i].as_str();

        if token == "--" {
            return if i + 1 < args.len() { Some(i + 1) } else { None };
        }

        if skip_next {
            skip_next = false;
            i += 1;
            continue;
        }

        if logs_flag_takes_value(token) {
            skip_next = true;
            i += 1;
            continue;
        }

        if token.starts_with('-') {
            i += 1;
            continue;
        }

        return Some(i);
    }

    None
}

fn matching_pods_for_input(namespace: &str, input: &str) -> Result<Vec<String>, String> {
    if input.is_empty() {
        return Err("Pod name cannot be empty".to_string());
    }

    let pods = kubectl_object_names("pods", Some(namespace));
    if pods.is_empty() {
        return Err(format!("No pods found in namespace '{}'", namespace));
    }

    let input_lower = input.to_ascii_lowercase();
    let matches: Vec<String> = pods
        .into_iter()
        .filter(|pod| pod.to_ascii_lowercase().contains(&input_lower))
        .collect();

    if matches.is_empty() {
        Err(format!(
            "No pods matched '{}' in namespace '{}'",
            input, namespace
        ))
    } else {
        Ok(matches)
    }
}

fn logs_has_time_range(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "--from"
            || arg == "--to"
            || arg.starts_with("--from=")
            || arg.starts_with("--to=")
    })
}

fn logs_has_follow(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "-f" || arg == "--follow")
}

/// Execute tail command to follow logs
pub fn execute_tail_command(args: &[String], state: &ShellState) -> Result<(), String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("Usage: tail <pod> | tail deploy <name>".to_string());
    }

    let namespace = effective_namespace(state);

    let pod = if args.len() == 2 {
        resolve_pod_name(&namespace, &args[1])?
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

#[derive(Debug)]
struct RestartReasonRow {
    container: String,
    restart_count: u32,
    current_reason: Option<String>,
    last_reason: Option<String>,
    last_exit_code: Option<i32>,
    last_finished_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestartReasonOutput {
    Table,
    Json,
    Markdown,
}

#[derive(Debug)]
struct RestartReasonArgs {
    namespace: String,
    positional: Vec<String>,
    show_all: bool,
    include_logs: bool,
    tail: Option<u32>,
    since: Option<String>,
    output: RestartReasonOutput,
}

#[derive(Debug, Serialize)]
struct RestartReasonItem {
    container: String,
    restart_count: u32,
    current_reason: Option<String>,
    last_reason: Option<String>,
    last_exit_code: Option<i32>,
    last_exit_hint: Option<String>,
    last_finished_at: Option<String>,
    logs: Option<String>,
}

#[derive(Debug, Serialize)]
struct RestartReasonReport {
    namespace: String,
    pod: String,
    items: Vec<RestartReasonItem>,
}

fn is_pod_resource(resource: &str) -> bool {
    matches!(resource, "pod" | "pods" | "po")
}

fn workload_selector(resource: &str, name: &str, namespace: &str) -> Result<String, String> {
    run_kubectl_capture(&[
        "get",
        resource,
        name,
        "-n",
        namespace,
        "-o",
        "jsonpath={range $k,$v := .spec.selector.matchLabels}{$k}={$v},{end}",
    ])
    .map(|value| value.trim_end_matches(',').to_string())
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn usage_restart_reason() -> &'static str {
    "Usage: restart-reason <pod|pod/name|resource/name|resource name> [-n namespace] [--all] [--logs] [--tail N] [--since DURATION] [--output table|json|markdown]"
}

fn parse_restart_reason_output(value: &str) -> Result<RestartReasonOutput, String> {
    match value {
        "table" => Ok(RestartReasonOutput::Table),
        "json" => Ok(RestartReasonOutput::Json),
        "markdown" | "md" => Ok(RestartReasonOutput::Markdown),
        _ => Err(format!(
            "Invalid output '{value}'. Expected one of: table, json, markdown"
        )),
    }
}

fn parse_u32_flag(name: &str, value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("{name} requires an integer value"))
}

fn exit_code_hint(code: i32) -> Option<&'static str> {
    match code {
        0 => Some("clean exit"),
        1 => Some("general error"),
        126 => Some("command invoked cannot execute"),
        127 => Some("command not found"),
        128 => Some("invalid exit argument"),
        130 => Some("terminated by SIGINT (Ctrl+C)"),
        137 => Some("terminated by SIGKILL (often OOMKill)") ,
        139 => Some("segmentation fault (SIGSEGV)"),
        143 => Some("terminated by SIGTERM"),
        _ => None,
    }
}

fn restart_row_has_evidence(row: &RestartReasonRow, show_all: bool) -> bool {
    show_all || row.restart_count > 0 || row.current_reason.is_some() || row.last_reason.is_some()
}

fn row_item_with_logs(
    namespace: &str,
    pod: &str,
    row: &RestartReasonRow,
    include_logs: bool,
    tail: Option<u32>,
    since: Option<&str>,
) -> RestartReasonItem {
    let logs = if include_logs {
        Some(fetch_container_logs(
            namespace,
            pod,
            row.container.strip_prefix("init:").unwrap_or(row.container.as_str()),
            tail.unwrap_or(50),
            since,
        ))
    } else {
        None
    };

    RestartReasonItem {
        container: row.container.clone(),
        restart_count: row.restart_count,
        current_reason: row.current_reason.clone(),
        last_reason: row.last_reason.clone(),
        last_exit_code: row.last_exit_code,
        last_exit_hint: row
            .last_exit_code
            .and_then(exit_code_hint)
            .map(str::to_string),
        last_finished_at: row.last_finished_at.clone(),
        logs,
    }
}

fn fetch_container_logs(
    namespace: &str,
    pod: &str,
    container: &str,
    tail: u32,
    since: Option<&str>,
) -> String {
    let mut args: Vec<String> = vec![
        "logs".to_string(),
        pod.to_string(),
        "-n".to_string(),
        namespace.to_string(),
        "-c".to_string(),
        container.to_string(),
        "--tail".to_string(),
        tail.to_string(),
    ];

    if let Some(value) = since {
        args.push("--since".to_string());
        args.push(value.to_string());
    }

    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_kubectl_capture(&refs).unwrap_or_else(|err| format!("<unable to fetch logs: {err}>"))
}

fn print_restart_reason_table(reports: &[RestartReasonReport]) {
    for report in reports {
        println!("Pod: {} (namespace: {})", report.pod, report.namespace);
        if report.items.is_empty() {
            println!("  No restart evidence found.");
            println!();
            continue;
        }

        println!("  {0:<28} {1:<8} {2:<24} {3:<20} {4:<9} {5}", "container", "restarts", "current", "last", "exit", "finished");
        println!("  {0:-<28} {1:-<8} {2:-<24} {3:-<20} {4:-<9} {5:-<20}", "", "", "", "", "", "");

        for item in &report.items {
            let exit_display = match (item.last_exit_code, item.last_exit_hint.as_deref()) {
                (Some(code), Some(hint)) => format!("{code} ({hint})"),
                (Some(code), None) => code.to_string(),
                _ => "".to_string(),
            };

            println!(
                "  {0:<28} {1:<8} {2:<24} {3:<20} {4:<9} {5}",
                item.container,
                item.restart_count,
                item.current_reason.as_deref().unwrap_or(""),
                item.last_reason.as_deref().unwrap_or(""),
                exit_display,
                item.last_finished_at.as_deref().unwrap_or(""),
            );

            if let Some(logs) = item.logs.as_deref() {
                println!("    logs:");
                for line in logs.lines() {
                    println!("      {line}");
                }
            }
        }
        println!();
    }
}

fn print_restart_reason_markdown(reports: &[RestartReasonReport]) {
    for report in reports {
        println!("## Pod `{}` (namespace `{}`)", report.pod, report.namespace);
        if report.items.is_empty() {
            println!();
            println!("No restart evidence found.");
            println!();
            continue;
        }

        println!();
        println!("| Container | Restarts | Current Reason | Last Reason | Last Exit | Finished At |");
        println!("|---|---:|---|---|---|---|");

        for item in &report.items {
            let exit_display = match (item.last_exit_code, item.last_exit_hint.as_deref()) {
                (Some(code), Some(hint)) => format!("{} ({})", code, hint),
                (Some(code), None) => code.to_string(),
                _ => "".to_string(),
            };

            println!(
                "| {} | {} | {} | {} | {} | {} |",
                item.container.replace('|', "\\|"),
                item.restart_count,
                item.current_reason.as_deref().unwrap_or("").replace('|', "\\|"),
                item.last_reason.as_deref().unwrap_or("").replace('|', "\\|"),
                exit_display.replace('|', "\\|"),
                item.last_finished_at.as_deref().unwrap_or("").replace('|', "\\|"),
            );

            if let Some(logs) = item.logs.as_deref() {
                println!();
                println!("Logs for `{}`:", item.container);
                println!("```text");
                println!("{}", logs);
                println!("```");
            }
        }

        println!();
    }
}

fn pod_restart_rows(namespace: &str, pod: &str) -> Result<Vec<RestartReasonRow>, String> {
    let output = run_kubectl_capture(&[
        "get",
        "pod",
        pod,
        "-n",
        namespace,
        "-o",
        "jsonpath={range .status.initContainerStatuses[*]}init:{.name}{'\t'}{.restartCount}{'\t'}{.state.waiting.reason}{'\t'}{.state.terminated.reason}{'\t'}{.lastState.terminated.reason}{'\t'}{.lastState.terminated.exitCode}{'\t'}{.lastState.terminated.finishedAt}{'\n'}{end}{range .status.containerStatuses[*]}{.name}{'\t'}{.restartCount}{'\t'}{.state.waiting.reason}{'\t'}{.state.terminated.reason}{'\t'}{.lastState.terminated.reason}{'\t'}{.lastState.terminated.exitCode}{'\t'}{.lastState.terminated.finishedAt}{'\n'}{end}",
    ])?;

    let mut rows = Vec::new();
    for line in output.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let mut cols = line.split('\t').map(str::trim).collect::<Vec<_>>();
        while cols.len() < 7 {
            cols.push("");
        }

        rows.push(RestartReasonRow {
            container: cols[0].to_string(),
            restart_count: cols[1].parse::<u32>().unwrap_or(0),
            current_reason: non_empty(cols[2]).or_else(|| non_empty(cols[3])),
            last_reason: non_empty(cols[4]),
            last_exit_code: non_empty(cols[5]).and_then(|v| v.parse::<i32>().ok()),
            last_finished_at: non_empty(cols[6]),
        });
    }

    Ok(rows)
}

fn parse_restart_reason_args(args: &[String], default_namespace: &str) -> Result<RestartReasonArgs, String> {
    if args.len() < 2 {
        return Err(usage_restart_reason().to_string());
    }

    let mut namespace = default_namespace.to_string();
    let mut show_all = false;
    let mut include_logs = false;
    let mut tail: Option<u32> = None;
    let mut since: Option<String> = None;
    let mut output = RestartReasonOutput::Table;
    let mut positional: Vec<String> = Vec::new();

    let mut idx = 1;
    while idx < args.len() {
        let arg = args[idx].as_str();

        if arg == "--all" {
            show_all = true;
            idx += 1;
            continue;
        }

        if arg == "--logs" {
            include_logs = true;
            idx += 1;
            continue;
        }

        if arg == "-n" || arg == "--namespace" {
            if idx + 1 >= args.len() {
                return Err(usage_restart_reason().to_string());
            }
            namespace = args[idx + 1].clone();
            idx += 2;
            continue;
        }

        if let Some(value) = arg.strip_prefix("--namespace=") {
            if value.is_empty() {
                return Err(usage_restart_reason().to_string());
            }
            namespace = value.to_string();
            idx += 1;
            continue;
        }

        if arg == "--tail" {
            if idx + 1 >= args.len() {
                return Err(usage_restart_reason().to_string());
            }
            tail = Some(parse_u32_flag("--tail", &args[idx + 1])?);
            include_logs = true;
            idx += 2;
            continue;
        }

        if let Some(value) = arg.strip_prefix("--tail=") {
            tail = Some(parse_u32_flag("--tail", value)?);
            include_logs = true;
            idx += 1;
            continue;
        }

        if arg == "--since" {
            if idx + 1 >= args.len() {
                return Err(usage_restart_reason().to_string());
            }
            since = Some(args[idx + 1].clone());
            include_logs = true;
            idx += 2;
            continue;
        }

        if let Some(value) = arg.strip_prefix("--since=") {
            if value.is_empty() {
                return Err(usage_restart_reason().to_string());
            }
            since = Some(value.to_string());
            include_logs = true;
            idx += 1;
            continue;
        }

        if arg == "-o" || arg == "--output" {
            if idx + 1 >= args.len() {
                return Err(usage_restart_reason().to_string());
            }
            output = parse_restart_reason_output(&args[idx + 1])?;
            idx += 2;
            continue;
        }

        if let Some(value) = arg.strip_prefix("--output=") {
            output = parse_restart_reason_output(value)?;
            idx += 1;
            continue;
        }

        if let Some(value) = arg.strip_prefix("-o=") {
            output = parse_restart_reason_output(value)?;
            idx += 1;
            continue;
        }

        if arg.starts_with('-') {
            return Err(format!("Unknown option for restart-reason: {arg}"));
        }

        positional.push(args[idx].clone());
        idx += 1;
    }

    if positional.is_empty() || positional.len() > 2 {
        return Err(usage_restart_reason().to_string());
    }

    Ok(RestartReasonArgs {
        namespace,
        positional,
        show_all,
        include_logs,
        tail,
        since,
        output,
    })
}

fn restart_reason_target_pods(namespace: &str, positional: &[String]) -> Result<Vec<String>, String> {
    if positional.len() == 2 {
        let resource = positional[0].as_str();
        let name = positional[1].as_str();

        if is_pod_resource(resource) {
            return Ok(vec![resolve_pod_name(namespace, name)?]);
        }

        let selector = workload_selector(resource, name, namespace)?;
        let pods = pods_for_selector(namespace, &selector);
        if pods.is_empty() {
            return Err(format!(
                "No pods found for {resource}/{name} in namespace '{namespace}'"
            ));
        }
        return Ok(pods);
    }

    let target = positional[0].as_str();
    if let Some((resource, name)) = target.split_once('/') {
        if is_pod_resource(resource) {
            return Ok(vec![resolve_pod_name(namespace, name)?]);
        }

        let selector = workload_selector(resource, name, namespace)?;
        let pods = pods_for_selector(namespace, &selector);
        if pods.is_empty() {
            return Err(format!(
                "No pods found for {resource}/{name} in namespace '{namespace}'"
            ));
        }
        return Ok(pods);
    }

    matching_pods_for_input(namespace, target)
}

pub fn execute_restart_reason_command(args: &[String], state: &ShellState) -> Result<(), String> {
    let parsed = parse_restart_reason_args(args, &effective_namespace(state))?;
    let pods = restart_reason_target_pods(&parsed.namespace, &parsed.positional)?;

    let mut reports: Vec<RestartReasonReport> = Vec::new();
    for pod in pods {
        let rows = pod_restart_rows(&parsed.namespace, &pod)?;
        let items: Vec<RestartReasonItem> = rows
            .iter()
            .filter(|row| restart_row_has_evidence(row, parsed.show_all))
            .map(|row| {
                row_item_with_logs(
                    &parsed.namespace,
                    &pod,
                    row,
                    parsed.include_logs,
                    parsed.tail,
                    parsed.since.as_deref(),
                )
            })
            .collect();

        reports.push(RestartReasonReport {
            namespace: parsed.namespace.clone(),
            pod,
            items,
        });
    }

    match parsed.output {
        RestartReasonOutput::Table => print_restart_reason_table(&reports),
        RestartReasonOutput::Markdown => print_restart_reason_markdown(&reports),
        RestartReasonOutput::Json => {
            let json = serde_json::to_string_pretty(&reports)
                .map_err(|err| format!("Failed to render JSON output: {err}"))?;
            println!("{json}");
        }
    }

    Ok(())
}

/// Main command execution router
pub fn execute_kubectl_command(input: &str, state: &mut ShellState) -> Result<(), String> {
    // Detect trailing `&` — run the command in the background.
    let trimmed = input.trim_end();
    let (run_bg, clean_input) = if trimmed.ends_with('&') {
        (true, trimmed.trim_end_matches('&').trim_end())
    } else {
        (false, input)
    };

    let mut args = parse_command_line(clean_input)?;

    if args.is_empty() {
        return Ok(());
    }

    if args[0] == "kubectl" {
        if run_bg {
            return state.job_manager.spawn(&args[1..], state.show_commands).map(|_| ());
        }
        return execute_prefixed_kubectl_command(&args[1..], state.show_commands);
    }

    args = expand_aliases(args, &state.aliases)?;

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

    if args[0] == "trace" {
        return execute_trace_command(&args, state);
    }

    if args[0] == "restart" {
        return execute_restart_command(&args, state);
    }

    if args[0] == "restart-reason" {
        return execute_restart_reason_command(&args, state);
    }

    if args[0] == "tail" {
        return execute_tail_command(&args, state);
    }

    if args[0] == "jobs" {
        state.job_manager.list();
        return Ok(());
    }

    if args[0] == "fg" {
        if args.len() < 2 {
            return Err("Usage: fg <job-id>".to_string());
        }
        let id = args[1]
            .parse::<usize>()
            .map_err(|_| format!("'{}' is not a valid job id", args[1]))?;
        return state.job_manager.foreground(id);
    }

    if args[0] == "job" {
        if args.len() < 2 {
            return Err("Usage: job kill <id> | job clean".to_string());
        }
        match args[1].as_str() {
            "kill" => {
                if args.len() < 3 {
                    return Err("Usage: job kill <id>".to_string());
                }
                let id = args[2]
                    .parse::<usize>()
                    .map_err(|_| format!("'{}' is not a valid job id", args[2]))?;
                return state.job_manager.kill_job(id);
            }
            "clean" => {
                state.job_manager.clean();
                return Ok(());
            }
            other => return Err(format!("Unknown job sub-command '{}'. Try: kill, clean", other)),
        }
    }

    if args[0] == "ask" {
        if args.len() < 2 {
            return Err("Usage: ask <question>".to_string());
        }
        let question = args[1..].join(" ");
        let context = effective_context(state);
        let namespace = effective_namespace(state);
        match state.ai_client.ask(&question, &context, &namespace) {
            Ok(response) => println!("{response}"),
            Err(e) => return Err(e),
        }
        return Ok(());
    }

    if args[0] == "ai" {
        match args.get(1).map(String::as_str) {
            Some("status") => return state.ai_client.status(),
            Some("model") => {
                if args.len() < 3 {
                    return Err("Usage: ai model <name>".to_string());
                }
                state.ai_client.model = args[2].clone();
                println!("AI model set to '{}'", args[2]);
                return Ok(());
            }
            Some("explain") => {
                if args.len() < 3 {
                    return Err("Usage: ai explain <kubectl args...>".to_string());
                }
                let context = effective_context(state);
                let namespace = effective_namespace(state);
                let mut kubectl_args = args[2..].to_vec();
                apply_default_namespace(&mut kubectl_args, state);
                let command_hint = format!("kubectl {}", kubectl_args.join(" "));
                let mut kubectl_stage = vec!["kubectl".to_string()];
                kubectl_stage.extend(kubectl_args);
                let output = run_pipeline_capture(&[kubectl_stage], state.show_commands)?;
                match state.ai_client.explain(
                    &output,
                    Some(&command_hint),
                    &context,
                    &namespace,
                ) {
                    Ok(response) => println!("{response}"),
                    Err(e) => return Err(e),
                }
                return Ok(());
            }
            _ => return Err("Usage: ai status | ai model <name> | ai explain <args...>".to_string()),
        }
    }

    if args.iter().any(|arg| arg == "|") {
        let stages = split_pipeline(&args)?;

        // Intercept `| explain` — last stage is "explain" (AI)
        if stages.last().map(|s| s.as_slice()) == Some(&["explain".to_string()]) {
            let preceding = &stages[..stages.len() - 1];
            let mut kubectl_args = preceding[0].clone();

            let context = effective_context(state);
            if state.risky_contexts.contains(&context)
                && should_confirm_in_risky_context(&kubectl_args)
                && !kubectl_args.iter().any(|arg| arg == "--yes")
                && !confirm_risky_context(&kubectl_args, &context)?
            {
                println!("Command cancelled.");
                return Ok(());
            }

            apply_output_profile(&mut kubectl_args, state.output_profile);
            apply_dry_run(&mut kubectl_args, state.dry_run);
            apply_default_namespace(&mut kubectl_args, state);

            let mut pipeline: Vec<Vec<String>> = Vec::with_capacity(preceding.len());
            let mut first_stage = vec!["kubectl".to_string()];
            first_stage.extend(kubectl_args);
            pipeline.push(first_stage);
            for stage in preceding[1..].iter() {
                pipeline.push(stage.clone());
            }

            let context = effective_context(state);
            let namespace = effective_namespace(state);
            let command_hint = pipeline
                .iter()
                .map(|stage| stage.join(" "))
                .collect::<Vec<_>>()
                .join(" | ");
            let output = run_pipeline_capture(&pipeline, state.show_commands)?;
            return state
                .ai_client
                .explain(&output, Some(&command_hint), &context, &namespace)
                .map(|response| println!("{response}"));
        }

        let mut stages = stages;
        let mut kubectl_args = stages.remove(0);

        let context = effective_context(state);
        if state.risky_contexts.contains(&context)
            && should_confirm_in_risky_context(&kubectl_args)
            && !kubectl_args.iter().any(|arg| arg == "--yes")
            && !confirm_risky_context(&kubectl_args, &context)?
        {
            println!("Command cancelled.");
            return Ok(());
        }

        apply_output_profile(&mut kubectl_args, state.output_profile);
        apply_dry_run(&mut kubectl_args, state.dry_run);
        apply_default_namespace(&mut kubectl_args, state);

        if state.safe_delete
            && kubectl_args.first().map(String::as_str) == Some("delete")
            && !kubectl_args.iter().any(|arg| arg == "--yes")
            && !confirm_delete(&kubectl_args)?
        {
            println!("Delete cancelled.");
            return Ok(());
        }

        let mut pipeline: Vec<Vec<String>> = Vec::with_capacity(stages.len() + 1);
        let mut first_stage = vec!["kubectl".to_string()];
        first_stage.extend(kubectl_args);
        pipeline.push(first_stage);
        pipeline.extend(stages);

        return run_command_pipeline(&pipeline, state.show_commands);
    }

    if args[0] == "logs" && args.iter().any(|arg| arg == "--multi" || arg == "--pick") {
        if logs_has_time_range(&args) && logs_has_follow(&args) {
            return Err("logs time ranges cannot be used with -f/--follow".to_string());
        }
        return crate::multi_logs::execute_multi_logs_command(&args, state.show_commands, effective_namespace(state));
    }

    if args[0] == "logs" {
        let time_ranged = logs_has_time_range(&args);

        if time_ranged && logs_has_follow(&args) {
            return Err("logs time ranges cannot be used with -f/--follow".to_string());
        }

        let Some(target_idx) = first_logs_target_index(&args) else {
            if time_ranged {
                return crate::multi_logs::execute_filtered_logs_command(&args, state.show_commands);
            }
            return run_kubectl_args(&args, state.show_commands);
        };

        let target = args[target_idx].clone();
        if !target.contains('/') {
            let namespace = explicit_namespace_from_args(&args).unwrap_or_else(|| effective_namespace(state));
            let matches = matching_pods_for_input(&namespace, &target)?;

            if matches.len() == 1 {
                args[target_idx] = matches[0].clone();
            } else {
                let mut logs_args = args.clone();
                logs_args.remove(target_idx);
                if time_ranged {
                    return crate::multi_logs::stream_logs_for_pod_list_with_filters(
                        &matches,
                        &namespace,
                        &logs_args,
                        state.show_commands,
                    );
                }
                return crate::multi_logs::stream_logs_for_pod_list(
                    &matches,
                    &namespace,
                    &logs_args,
                    state.show_commands,
                );
            }
        }

        if time_ranged {
            return crate::multi_logs::execute_filtered_logs_command(&args, state.show_commands);
        }
    }

    // Handle kubectl commands
    let context = effective_context(state);
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
    apply_default_namespace(&mut args, state);

    if state.safe_delete
        && args.first().map(String::as_str) == Some("delete")
        && !args.iter().any(|arg| arg == "--yes")
        && !confirm_delete(&args)?
    {
        println!("Delete cancelled.");
        return Ok(());
    }

    if args.first().map(String::as_str) == Some("port-forward") {
        if run_bg {
            return state.job_manager.spawn(&args, state.show_commands).map(|_| ());
        }
        return execute_port_forward_with_optional_browse(&args, state.show_commands);
    }

    if run_bg {
        return state.job_manager.spawn(&args, state.show_commands).map(|_| ());
    }

    run_kubectl_args(&args, state.show_commands)
}
