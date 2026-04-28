use std::process::{Command, Stdio};

use crate::interrupt::ForegroundCommandGuard;

/// Run kubectl and capture output
pub fn run_kubectl_capture(args: &[&str]) -> Result<String, String> {
    let output = Command::new("kubectl")
        .args(args)
        .output()
        .map_err(|err| format!("Failed to execute kubectl: {err}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Run kubectl with inherited stdio
pub fn run_kubectl_status(args: &[&str], show_commands: bool) -> Result<(), String> {
    if show_commands {
        print_kubectl_command_refs(args);
    }

    let _guard = ForegroundCommandGuard::new();
    let status = Command::new("kubectl")
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|err| format!("Failed to execute kubectl: {err}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("kubectl exited with status: {status}"))
    }
}

/// Run kubectl with String args and inherited stdio
pub fn run_kubectl_args(args: &[String], show_commands: bool) -> Result<(), String> {
    if show_commands {
        print_kubectl_command(args);
    }

    let _guard = ForegroundCommandGuard::new();
    let status = Command::new("kubectl")
        .args(args.iter().map(String::as_str))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|err| format!("Failed to execute kubectl: {err}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("kubectl exited with status: {status}"))
    }
}

/// Get kubectl output as lines
pub fn kubectl_lines(args: &[&str]) -> Vec<String> {
    run_kubectl_capture(args)
        .ok()
        .map(|s| {
            s.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Get object names for a kubectl resource
pub fn kubectl_object_names(resource: &str, namespace: Option<&str>) -> Vec<String> {
    let mut owned_args: Vec<String> = vec!["get".to_string()];

    if let Some(ns) = namespace {
        owned_args.push("-n".to_string());
        owned_args.push(ns.to_string());
    }

    owned_args.push(resource.to_string());
    owned_args.push("-o".to_string());
    owned_args.push("jsonpath={range .items[*]}{.metadata.name}{\"\\n\"}{end}".to_string());

    let arg_refs: Vec<&str> = owned_args.iter().map(String::as_str).collect();
    kubectl_lines(&arg_refs)
}

/// Get container names for a pod
pub fn kubectl_pod_containers(pod: &str, namespace: Option<&str>) -> Vec<String> {
    let mut owned_args: Vec<String> = vec!["get".to_string(), "pod".to_string(), pod.to_string()];

    if let Some(ns) = namespace {
        owned_args.push("-n".to_string());
        owned_args.push(ns.to_string());
    }

    owned_args.push("-o".to_string());
    owned_args.push(
        "jsonpath={range .spec.initContainers[*]}{.name}{\"\\n\"}{end}{range .spec.containers[*]}{.name}{\"\\n\"}{end}".to_string(),
    );

    let arg_refs: Vec<&str> = owned_args.iter().map(String::as_str).collect();
    kubectl_lines(&arg_refs)
}

/// Get the current context
pub fn current_context() -> String {
    run_kubectl_capture(&["config", "current-context"])
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown-cluster".to_string())
}

/// Get the current namespace
pub fn current_namespace() -> String {
    let ns = run_kubectl_capture(&[
        "config",
        "view",
        "--minify",
        "-o",
        "jsonpath={..namespace}",
    ])
    .ok()
    .unwrap_or_default();

    if ns.is_empty() {
        "default".to_string()
    } else {
        ns
    }
}

/// Set the current namespace
pub fn set_namespace(namespace: &str, show_commands: bool) -> Result<(), String> {
    run_kubectl_status(
        &["config", "set-context", "--current", "--namespace", namespace],
        show_commands,
    )
}

/// Set the current context
pub fn set_context(context: &str, show_commands: bool) -> Result<(), String> {
    run_kubectl_status(&["config", "use-context", context], show_commands)
}

/// Format arguments for display
fn shell_escape_arg(arg: &str) -> String {
    if arg.is_empty()
        || arg
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '\\' | '\''))
    {
        format!("{:?}", arg)
    } else {
        arg.to_string()
    }
}

/// Print kubectl command with string refs
pub fn print_kubectl_command_refs(args: &[&str]) {
    let rendered = args
        .iter()
        .map(|arg| shell_escape_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    println!("+ kubectl {rendered}");
}

/// Print kubectl command with String args
pub fn print_kubectl_command(args: &[String]) {
    let rendered = args
        .iter()
        .map(|arg| shell_escape_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    println!("+ kubectl {rendered}");
}
