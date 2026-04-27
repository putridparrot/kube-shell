use std::collections::HashSet;
use rustyline::completion::Pair;
use crate::constants::*;
use crate::kubectl::current_namespace;
use crate::types::KubeShellHelper;

pub fn completion_start(line: &str, pos: usize) -> usize {
    line[..pos]
        .rfind(char::is_whitespace)
        .map(|i| i + 1)
        .unwrap_or(0)
}

pub fn command_candidates(previous_tokens: &[&str], current_token: &str) -> Vec<String> {
    let command_index = if previous_tokens.first() == Some(&"kubectl") {
        1
    } else {
        0
    };

    if previous_tokens.len() <= command_index {
        let mut roots = vec!["kubectl".to_string()];
        roots.extend(COMMANDS.iter().map(|s| (*s).to_string()));
        roots.extend(BUILT_INS.iter().map(|s| (*s).to_string()));
        return roots;
    }

    let active_cmd = previous_tokens
        .get(command_index)
        .copied()
        .unwrap_or_default();

    if current_token.starts_with('-') {
        return COMMON_FLAGS.iter().map(|s| (*s).to_string()).collect();
    }

    if matches!(active_cmd, "get" | "describe" | "delete" | "edit") {
        return RESOURCES.iter().map(|s| (*s).to_string()).collect();
    }

    COMMON_FLAGS.iter().map(|s| (*s).to_string()).collect()
}

pub fn takes_value_flag(token: &str) -> bool {
    matches!(
        token,
        "-n"
            | "--namespace"
            | "--context"
            | "--kubeconfig"
            | "-l"
            | "--selector"
            | "-o"
            | "--output"
            | "-c"
            | "--container"
    )
}

pub fn first_positional_after_command(tokens: &[&str], command_index: usize) -> Option<String> {
    let mut i = command_index + 1;
    let mut skip_next = false;

    while i < tokens.len() {
        let token = tokens[i];

        if token == "--" {
            break;
        }

        if skip_next {
            skip_next = false;
            i += 1;
            continue;
        }

        if takes_value_flag(token) {
            skip_next = true;
            i += 1;
            continue;
        }

        if token.starts_with('-') {
            i += 1;
            continue;
        }

        return Some(token.to_string());
    }

    None
}

pub fn command_candidates_with_live(
    helper: &KubeShellHelper,
    previous_tokens: &[&str],
    current_token: &str,
) -> Vec<String> {
    if previous_tokens.is_empty() && current_token == "get" {
        return vec!["get".to_string()];
    }

    let command_index = if previous_tokens.first() == Some(&"kubectl") {
        1
    } else {
        0
    };
    let active_cmd = previous_tokens
        .get(command_index)
        .copied()
        .unwrap_or_default();

    if previous_tokens.contains(&"--") {
        if active_cmd == "exec" {
            return helper.exec_inner_commands();
        }
        return Vec::new();
    }

    let mut options = command_candidates(previous_tokens, current_token);
    if previous_tokens.is_empty() {
        options.extend(helper.alias_names());
    }

    helper.refresh_cache_if_needed();

    if active_cmd == "bookmark" {
        if previous_tokens.len() == command_index + 1 {
            options.extend(
                ["add", "use", "list", "remove", "rm", "delete"]
                    .into_iter()
                    .map(str::to_string),
            );
            return options;
        }

        if previous_tokens.last() == Some(&"use")
            || previous_tokens.last() == Some(&"remove")
            || previous_tokens.last() == Some(&"rm")
            || previous_tokens.last() == Some(&"delete")
        {
            options.extend(helper.bookmark_names());
            return options;
        }
    }

    if active_cmd == "alias" {
        if previous_tokens.len() == command_index + 1 {
            options.extend(["list", "test"].into_iter().map(str::to_string));
            return options;
        }

        if previous_tokens.last() == Some(&"test") {
            options.extend(helper.alias_names());
            return options;
        }
    }

    if active_cmd == "dryrun" {
        options.extend(["on", "off", "status"].into_iter().map(str::to_string));
        return options;
    }

    if active_cmd == "showcmd" || active_cmd == "trace" {
        options.extend(["on", "off", "status"].into_iter().map(str::to_string));
        return options;
    }

    if active_cmd == "help" {
        options.extend(BUILT_INS.iter().map(|s| (*s).to_string()));
        options.extend(COMMANDS.iter().map(|s| (*s).to_string()));
        return options;
    }

    if active_cmd == "b" {
        if previous_tokens.len() == command_index + 1 {
            options.extend(helper.bookmark_names());
            options.extend(
                ["add", "use", "list", "remove", "rm", "delete"]
                    .into_iter()
                    .map(str::to_string),
            );
            return options;
        }

        if previous_tokens.last() == Some(&"use")
            || previous_tokens.last() == Some(&"remove")
            || previous_tokens.last() == Some(&"rm")
            || previous_tokens.last() == Some(&"delete")
        {
            options.extend(helper.bookmark_names());
            return options;
        }
    }

    if active_cmd == "pick" {
        options.extend(helper.with_cache(|c| c.resources.clone()));
        return options;
    }

    if active_cmd == "restart" {
        if previous_tokens.len() == command_index + 1 {
            options.extend(["deploy", "deployment", "daemonset", "statefulset"].into_iter().map(str::to_string));
            return options;
        }
    }

    if active_cmd == "tail" {
        if previous_tokens.len() == command_index + 1 {
            options.extend(["deploy", "deployment", "pods"].into_iter().map(str::to_string));
            let ns = current_namespace();
            options.extend(helper.object_names_for("pods", Some(ns.as_str())));
            return options;
        }
    }

    if previous_tokens.last() == Some(&"ctx")
        || previous_tokens.last() == Some(&"context")
        || previous_tokens.ends_with(&["config", "use-context"])
    {
        options.extend(helper.with_cache(|c| c.contexts.clone()));
        return options;
    }

    if previous_tokens.last() == Some(&"ns")
        || previous_tokens.last() == Some(&"namespace")
        || previous_tokens.last() == Some(&"-n")
        || previous_tokens.last() == Some(&"--namespace")
    {
        options.extend(helper.with_cache(|c| c.namespaces.clone()));
        return options;
    }

    if matches!(active_cmd, "use" | "switch") {
        let contexts = helper.with_cache(|c| c.contexts.clone());
        let namespaces = helper.with_cache(|c| c.namespaces.clone());

        if previous_tokens.len() == command_index + 1 {
            if let Some((ctx_prefix, ns_prefix)) = current_token.split_once('/') {
                if ctx_prefix.is_empty() {
                    options.extend(
                        namespaces
                            .iter()
                            .filter(|ns| ns.starts_with(ns_prefix))
                            .map(|ns| format!("/{ns}")),
                    );
                } else {
                    for ctx in contexts.iter().filter(|ctx| ctx.starts_with(ctx_prefix)) {
                        if ns_prefix.is_empty() {
                            options.push(format!("{ctx}/"));
                        }

                        options.extend(
                            namespaces
                                .iter()
                                .filter(|ns| ns.starts_with(ns_prefix))
                                .map(|ns| format!("{ctx}/{ns}")),
                        );
                    }
                }
            } else {
                options.extend(contexts);
                options.extend(namespaces.iter().map(|ns| format!("/{ns}")));
            }
            return options;
        }

        options.extend(namespaces);
        return options;
    }

    if matches!(active_cmd, "get" | "describe" | "delete" | "edit" | "logs") {
        options.extend(helper.with_cache(|c| c.resources.clone()));
        if active_cmd == "logs" {
            options.extend(
                [
                    "--multi",
                    "--pick",
                    "--no-ts",
                    "--no-align",
                    "--include",
                    "--exclude",
                    "--before",
                    "--after",
                    "--ignore-case",
                    "--regex",
                ]
                .into_iter()
                .map(str::to_string),
            );
        }

        if previous_tokens.len() >= command_index + 2 {
            let resource = previous_tokens[command_index + 1];
            if !resource.starts_with('-') && !current_token.starts_with('-') {
                let namespace = explicit_namespace(previous_tokens)
                    .or_else(|| {
                        let ns = current_namespace();
                        if ns.is_empty() {
                            None
                        } else {
                            Some(ns)
                        }
                    });
                options.extend(helper.object_names_for(resource, namespace.as_deref()));
            }
        }
    }

    if matches!(active_cmd, "logs" | "exec") {
        let namespace = explicit_namespace(previous_tokens)
            .or_else(|| {
                let ns = current_namespace();
                if ns.is_empty() {
                    None
                } else {
                    Some(ns)
                }
            });

        if let Some(container_prefix) = current_token.strip_prefix("--container=") {
            if let Some(pod) = first_positional_after_command(previous_tokens, command_index) {
                let containers = helper.containers_for_pod(&pod, namespace.as_deref());
                let mut matches: Vec<String> = containers
                    .into_iter()
                    .filter(|name| name.starts_with(container_prefix))
                    .map(|name| format!("--container={name}"))
                    .collect();
                options.append(&mut matches);
            }
        } else if previous_tokens.last() == Some(&"-c") || previous_tokens.last() == Some(&"--container") {
            if let Some(pod) = first_positional_after_command(previous_tokens, command_index) {
                options.extend(helper.containers_for_pod(&pod, namespace.as_deref()));
            }
        } else if !current_token.starts_with('-') {
            options.extend(helper.object_names_for("pods", namespace.as_deref()));
        }
    }

    options
}

pub fn explicit_namespace(tokens: &[&str]) -> Option<String> {
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "-n" || tokens[i] == "--namespace" {
            if i + 1 < tokens.len() {
                return Some(tokens[i + 1].to_string());
            }
            return None;
        }

        if let Some(ns) = tokens[i].strip_prefix("--namespace=")
            && !ns.is_empty()
        {
            return Some(ns.to_string());
        }

        i += 1;
    }

    None
}

pub fn to_pairs(options: Vec<String>, prefix: &str) -> Vec<Pair> {
    let filtered: Vec<String> = options
        .into_iter()
        .filter(|option| option.starts_with(prefix))
        .collect();

    let mut seen = HashSet::new();
    let deduped: Vec<String> = filtered
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect();

    deduped
        .into_iter()
        .map(|option| {
            let replacement = if option == prefix {
                format!("{option} ")
            } else {
                format!("{option} ")
            };

            Pair {
                display: option,
                replacement,
            }
        })
        .collect()
}

// Cache refresh methods on KubeShellHelper
impl KubeShellHelper {
    pub fn refresh_cache_if_needed(&self) {
        use std::time::{Duration, Instant};
        use crate::kubectl::kubectl_lines;

        let ttl = Duration::from_secs(30);
        let mut cache = match self.cache.lock() {
            Ok(c) => c,
            Err(_) => return,
        };

        if let Some(last) = cache.refreshed_at
            && last.elapsed() < ttl
        {
            return;
        }

        cache.contexts = kubectl_lines(&["config", "get-contexts", "-o", "name"]);
        cache.namespaces = kubectl_lines(&[
            "get",
            "namespaces",
            "-o",
            "jsonpath={range .items[*]}{.metadata.name}{\"\\n\"}{end}",
        ]);
        cache.resources = kubectl_lines(&["api-resources", "-o", "name"]);
        cache.refreshed_at = Some(Instant::now());
    }

    pub fn object_names_for(&self, resource: &str, namespace: Option<&str>) -> Vec<String> {
        use std::time::{Duration, Instant};
        use crate::kubectl::kubectl_object_names;

        let key = format!("{}::{}", resource, namespace.unwrap_or("<all>"));
        let ttl = Duration::from_secs(20);

        {
            let cache = match self.cache.lock() {
                Ok(c) => c,
                Err(_) => return Vec::new(),
            };

            if let Some((at, values)) = cache.object_names.get(&key)
                && at.elapsed() < ttl
            {
                return values.clone();
            }
        }

        let fetched = kubectl_object_names(resource, namespace);

        if let Ok(mut cache) = self.cache.lock() {
            cache
                .object_names
                .insert(key, (Instant::now(), fetched.clone()));
        }

        fetched
    }

    pub fn containers_for_pod(&self, pod: &str, namespace: Option<&str>) -> Vec<String> {
        use std::time::{Duration, Instant};
        use crate::kubectl::kubectl_pod_containers;

        let key = format!("{}::{}", pod, namespace.unwrap_or("<all>"));
        let ttl = Duration::from_secs(20);

        {
            let cache = match self.cache.lock() {
                Ok(c) => c,
                Err(_) => return Vec::new(),
            };

            if let Some((at, values)) = cache.pod_containers.get(&key)
                && at.elapsed() < ttl
            {
                return values.clone();
            }
        }

        let fetched = kubectl_pod_containers(pod, namespace);

        if let Ok(mut cache) = self.cache.lock() {
            cache
                .pod_containers
                .insert(key, (Instant::now(), fetched.clone()));
        }

        fetched
    }
}
