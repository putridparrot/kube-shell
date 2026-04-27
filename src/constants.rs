/// kubectl commands supported for completion
pub const COMMANDS: &[&str] = &[
    "get",
    "describe",
    "logs",
    "delete",
    "apply",
    "edit",
    "exec",
    "port-forward",
    "top",
    "rollout",
    "scale",
    "create",
    "api-resources",
    "api-versions",
    "version",
    "config",
    "cluster-info",
    "help",
];

/// Resource types for kubectl
pub const RESOURCES: &[&str] = &[
    "pods",
    "po",
    "deployments",
    "deploy",
    "services",
    "svc",
    "ingresses",
    "ing",
    "nodes",
    "no",
    "namespaces",
    "ns",
    "configmaps",
    "cm",
    "secrets",
    "jobs",
    "cronjobs",
    "statefulsets",
    "sts",
    "daemonsets",
    "ds",
    "replicasets",
    "rs",
    "events",
    "ev",
];

/// Common kubectl flags for completion
pub const COMMON_FLAGS: &[&str] = &[
    "--all-namespaces",
    "--namespace",
    "-n",
    "-o",
    "--output",
    "--selector",
    "-l",
    "--watch",
    "-w",
    "--context",
    "--kubeconfig",
];

/// Built-in kube-shell commands
pub const BUILT_INS: &[&str] = &[
    "!!",
    "trace",
    "showcmd",
    "help",
    "ns",
    "namespace",
    "ctx",
    "context",
    "use",
    "switch",
    "bookmark",
    "b",
    "alias",
    "view",
    "dryrun",
    "pick",
    "restart",
    "tail",
    "exit",
    "quit",
];

/// Common commands used inside exec
pub const EXEC_INNER_COMMANDS: &[&str] = &[
    "sh",
    "bash",
    "ash",
    "zsh",
    "ls",
    "cat",
    "env",
    "printenv",
    "ps",
    "top",
    "whoami",
    "id",
    "pwd",
    "cd",
    "echo",
    "grep",
    "awk",
    "sed",
    "curl",
    "wget",
    "nslookup",
    "dig",
    "netstat",
    "ss",
];

/// Configuration file name
pub const CONFIG_FILE_NAME: &str = ".kube-shellrc";

/// Bookmarks file name
pub const BOOKMARKS_FILE_NAME: &str = ".kube-shell-bookmarks";

/// State file name
pub const STATE_FILE_NAME: &str = ".kube-shell-state";

/// Default prompt template
pub const DEFAULT_PROMPT_TEMPLATE: &str = "KS {risk}{context}/{namespace}> ";
