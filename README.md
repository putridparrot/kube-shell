# kube-shell

kube-shell is an interactive shell for Kubernetes that lets you run kubectl-style commands without typing kubectl every time.

Instead of this:

    kubectl get pods

You can do this inside kube-shell:

    get pods

The prompt shows your current cluster and namespace in this format:

    KS <cluster>/<namespace>

## Features

- Interactive Kubernetes shell experience.
- Prompt includes current context and namespace.
- Command forwarding to kubectl:
  - get pods
  - describe pod my-pod
  - logs my-pod
- Quoted argument parsing for selectors and other complex values.
- Built-in context and namespace switching commands.
- Built-in help command.
- Built-in output profiles.
- Built-in rollout restart helper, restart reason diagnostics, and smart log tailing.
- Multi-pod fuzzy log streaming with per-pod colors.
- Timestamped and aligned multi-pod log output.
- Configurable command aliases.
- Parameterized alias shortcuts (macros).
- Optional safe-delete confirmation.
- Optional risky-context confirmations.
- Optional full kubectl command display before execution.
- Persistent history across sessions.
- Persistent shell state across sessions (view/dryrun/previous ctx+ns).
- History-based hints and interactive completion.
- Live completion data from your current Kubernetes environment:
  - contexts
  - namespaces
  - resources
  - object names
  - pod container names
- Workspace-local and home configuration for exec command suggestions.

## Requirements

- Rust toolchain (cargo)
- kubectl installed and available in PATH
- Access to at least one Kubernetes context in your kubeconfig

## Build and Run

From the project root:

    cargo run

If you hit a Windows file-lock on target/debug/kube-shell.exe, run with an alternate target directory:

    cargo run --target-dir target_alt

To compile only:

    cargo check

## Usage

### Standard kubectl-style commands

Inside kube-shell, type kubectl commands without kubectl:

    get pods
    get pods -A
    describe pod my-pod
    logs my-pod
    logs doc-service
    logs -f my-pod
    logs my-pod --from 09:50 --to 10:30
    logs doc-service --from "2026-04-28 09:50" --to "2026-04-28 10:30"
    exec my-pod -- sh

Notes:
- `logs <pod>` also accepts a partial pod name in the current namespace.
- If multiple pods match (for example `logs doc-service`), kube-shell automatically streams all matches.
- Use `logs --multi` or `logs --pick` when you want interactive pod selection.
- Use `--from <time>` and/or `--to <time>` to show a bounded log window; time-range mode does not allow `-f/--follow`.
- When a time range is used, kube-shell automatically enables kubectl timestamps for filtering.
- You can pipe command output, for example: `get namespaces | findstr kube`.
- Multi-stage pipelines are supported, for example: `get pods -A | findstr Running | sort`.
- If you prefix with `kubectl`, kube-shell passes the command through as-is (including pipelines), for example: `kubectl get namespaces | findstr kube`.

You can still type full commands if you prefer:

    kubectl get pods

### Exit commands

    exit
    quit

### Repeat previous command

    !!

Re-runs the previous command in the current session.

## Built-in Commands

### Help

    help
    help <topic>

Examples:

    help
    help logs

### Namespace switching

    ns <namespace>
    namespace <namespace>

Example:

    ns kube-system

Switch back to previous namespace:

    ns -

### Context switching

    ctx <context>
    context <context>

Example:

    ctx docker-desktop

Switch back to previous context:

    ctx -

### Combined switch command

Switch context and/or namespace in one command:

    use <context>
    use /<namespace>
    use <context>/<namespace>
    use <context> <namespace>

switch is an alias for use:

    switch docker-desktop/default

Examples:

    use docker-desktop
    use /kube-system
    use docker-desktop/default
    use docker-desktop kube-system

### Alias Shortcuts For Switching

Use aliases as saved context/namespace shortcuts:

    alias dev=use docker-desktop/default
    alias ops=use /kube-system
    alias prod=use prod-cluster/default

Examples:

    dev
    ops
    prod

### Output profiles

Set a default output profile for get commands:

    view
    view default
    view wide
    view yaml
    view json

Notes:
- Profile applies automatically to get commands when no explicit -o/--output is provided.
- Explicit -o/--output always wins.

### Dry-run mode

Toggle automatic dry-run for mutating commands:

    dryrun
    dryrun status
    dryrun on
    dryrun off

Notes:
- When enabled, commands such as apply/create/replace/patch/delete get --dry-run=client.
- For apply/create/replace, kube-shell also adds -o yaml unless output is explicitly set.

### Command display mode

Show the exact kubectl command before it runs:

    trace
    trace status
    trace on
    trace off

### Multi-pod logs

Use fuzzy multi-selection to stream logs from multiple pods at once:

    logs --multi
    logs --pick
    logs --multi -n kube-system
    logs --multi -c app --tail=100
    logs --multi --no-ts
    logs --multi --no-align
    logs --multi --include exception --after 10
    logs --multi --exclude healthz
    logs --multi --include "(exception|panic)" --regex --ignore-case --after 10
    logs --multi --from 09:50 --to 10:30
    logs --multi --from "2026-04-28 09:50" --to "2026-04-28 10:30"

Notes:
- Opens an interactive fuzzy selector for pods in the effective namespace.
- To stream all pods containing a name fragment (for example `doc-service`): run `logs --multi`, type `doc-service` to filter, then enter `all`.
- Select multiple pods with numbers like 1,3-5 or all.
- Each pod stream is prefixed and colorized per pod.
- Multi-stream output includes elapsed timestamps and aligned pod columns by default.
- Use `--from <time>` and/or `--to <time>` to filter logs to a local time window.
- Supported time formats: `HH:MM`, `YYYY-MM-DD`, `YYYY-MM-DD HH:MM`, and RFC3339 timestamps.
- kube-shell automatically adds kubectl timestamps when a time range is used.
- Time-range mode disables follow behavior, even if `-f/--follow` is supplied.
- Use --no-ts to disable elapsed timestamps.
- Use --no-align to disable pod-column alignment.
- Use --include <pattern> to print only matching lines (plus context if configured).
- Use --exclude <pattern> to skip noisy lines.
- Use --before <N> and --after <N> to include context around include matches.
- Use --ignore-case for case-insensitive matching.
- Use --regex to treat include/exclude patterns as regular expressions.
- If -f/--follow is not provided, kube-shell enables follow mode automatically for multi-stream output.

### Restart helper

Rollout restart followed by rollout status:

    restart <deployment>
    restart <resource> <name>

Examples:

    restart api
    restart deployment api

### Restart reason helper

Show likely reasons a pod (or pods behind a workload) restarted:

    restart-reason <pod-name>
    restart-reason pod/<name>
    restart-reason <resource>/<name>
    restart-reason <resource> <name>

Options:

    -n <namespace>
    --namespace=<namespace>
    --all
    --logs
    --tail <N>
    --since <duration>
    -o table|json|markdown
    --output=table|json|markdown

Examples:

    restart-reason api-7c98f4bc9f-vm2kq
    restart-reason pod/api-7c98f4bc9f-vm2kq
    restart-reason deployment/api
    restart-reason statefulset redis
    restart-reason deployment api -n kube-system
    restart-reason deployment/api --logs --tail 75 --since 30m
    restart-reason deployment/api -o json
    restart-reason deployment/api --output=markdown

Notes:
- `--tail` and `--since` imply `--logs`.
- Default output format is table.
- Exit codes include common hints (for example `137` often indicates SIGKILL/OOMKill).

### Tail helper

Follow logs quickly:

    tail <pod>
    tail deploy <deployment>

If a deployment has multiple pods, kube-shell asks you to pick one.

### Port-forward browser helper

`kube-shell` adds an optional `--browse` flag for `port-forward` commands.

Usage:

    port-forward svc/my-service 8080:80 --browse
    port-forward svc/my-service 8443:443 --browse-scheme https
    kubectl port-forward pod/my-pod 9000:9000 --browse

Behavior:
- `--browse` is removed before invoking kubectl.
- kube-shell opens `<scheme>://localhost:<localPort>` after the port-forward process starts.
- `--browse-scheme` accepts `http` or `https` (default is `http`).
- `--browse` requires an explicit local port mapping (for example `8080:80`).

### Alias helper

Inspect/test loaded aliases:

    alias
    alias list
    alias <name>=<expansion>
    alias add <name> <expansion>
    alias remove <name>
    alias test <name> [args...]

Example:

    alias redisinsight=port-forward pod/redisinsight-123456-7890 5540:5540
    alias test logsn kube-system coredns-abc123

Notes:
- Aliases added from the shell are persisted across sessions.
- Runtime aliases are stored in workspace-local ./.kube-shell-aliases when possible (falls back to home directory if current directory cannot be resolved).
- Aliases from .kube-shellrc are also loaded, and runtime aliases override duplicates.

## Completion Behavior

kube-shell completion uses static and live Kubernetes data.

### Top-level completion

Suggests common command verbs and built-ins, including:
- get, describe, logs, exec, apply, delete
- !!, help, ns, ctx, use, switch, alias, dryrun, trace, restart, restart-reason, tail

### Context and namespace completion

- After ctx or context, suggests live contexts.
- After ns or namespace, suggests live namespaces.
- For use and switch, supports context and namespace forms.

### Resource and object completion

- After get, describe, delete, edit, logs, suggests resources.
- For resource-position arguments, suggests live object names.

### logs and exec enhancements

- Suggests pod names where appropriate.
- Suggests logs multi-stream helpers:
    - --multi
    - --pick
    - --no-ts
    - --no-align
- Suggests port-forward helper flag:
    - --browse
    - --browse-scheme
- Suggests container names after:
  - -c
  - --container
  - --container=<prefix>

### exec inner command mode

After you type exec ... --, completion switches to inner command suggestions (shell commands like sh, bash, ls, env, and others).

## History and Hints

- Command history is persisted to:
  - %USERPROFILE%/.kube-shell-history on Windows (or HOME equivalent)
- Arrow key history navigation is supported.
- History-based hints appear as you type.
- Use !! to quickly re-run the previous command.

## Configuration

kube-shell supports .kube-shellrc for customizing exec inner command suggestions.

Configuration lookup order:
1. Workspace-local: ./.kube-shellrc
2. Home directory: %USERPROFILE%/.kube-shellrc (or HOME equivalent)
3. Built-in defaults if no valid config entries are found

### .kube-shellrc format

Supported keys:

    exec_inner_command=<command>
    exec_inner_commands=cmd1,cmd2,cmd3
    hint_color=<ansi-code-or-name>
    alias <name>=<expansion>
    alias.<name>=<expansion>
    dry_run=<true|false>
    show_commands=<true|false>
    safe_delete=<true|false>
    prompt_template=<template>
    risky_context=<context-name>
    risky_contexts=ctx1,ctx2,ctx3

Example:

    # one per line
    exec_inner_command=sh
    exec_inner_command=bash

    # comma-separated list
    exec_inner_commands=ls,cat,env,printenv,ps,top

    # autocomplete hint color
    # default is light gray (90)
    hint_color=90

    # named value aliases
    # hint_color=light-gray

    # aliases
    alias gp=get pods
    alias gl=logs
    alias.gd=get deployments

    # enable dry-run mode at startup (default false)
    dry_run=false

    # show full kubectl commands before execution (default false)
    show_commands=false

    # delete confirmation (default true)
    safe_delete=true

    # prompt format (supports {risk}, {context}, {namespace})
    prompt_template=KS {risk}{context}/{namespace}> 

    # contexts requiring explicit confirmation for risky commands
    risky_context=production
    risky_contexts=prod-cluster,live-us-east

Notes:
- Empty lines are ignored.
- Lines starting with # are comments.
- Duplicates are removed.
- hint_color supports:
    - ANSI code fragments, e.g. 90 or 38;5;245
    - Named aliases: light-gray, light_grey, lightgray, gray, grey
- alias supports either:
    - alias <name>=<expansion>
    - alias.<name>=<expansion>
- alias placeholders:
  - {1}, {2}, ... for positional arguments
  - {all} for all arguments
  - Unused arguments are appended automatically unless {all} is used
- safe_delete=true prompts before delete commands unless --yes is supplied.
- risky_context/risky_contexts require confirmation for risky commands in matching contexts.
- prompt_template placeholders:
    - {risk}: [RISK] when current context is risky, otherwise empty
    - {context}: current kubectl context
    - {namespace}: current namespace

## Persistent State

kube-shell persists runtime state to a workspace-local file:

        ./.kube-shell-state

Saved values include:
- output profile (view)
- dry-run mode
- command display mode (trace)
- previous context (for ctx -)
- previous namespace (for ns -)

### Risk badge

If current context is listed in risky_context/risky_contexts, prompt shows:

    KS [RISK] <context>/<namespace>

### Alias shortcut examples

Simple shortcut:

    alias pods=get pods

Then run:

    pods

Parameterized shortcut:

    alias logsn=logs -n {1} {2}

Then run:

    logsn kube-system coredns-abc123

Use all args:

    alias kg=get {all}

Then run:

    kg pods -A

## Troubleshooting

### kubectl not found

Ensure kubectl is installed and available in PATH.

### Access or authentication errors

Verify your kubeconfig, current context, and cluster credentials:

    kubectl config get-contexts
    kubectl config current-context

### No completion data for resources

If completion cannot query the cluster, kube-shell falls back to static suggestions.

### Risky context prompts

When current context matches risky_context(s), kube-shell prompts before risky commands such as:
- delete
- apply
- replace
- patch
- rollout
- restart

### Windows executable lock

If cargo run fails with access denied on target/debug/kube-shell.exe, use:

    cargo run --target-dir target_alt

## Development Notes

Core application entry point:
- src/main.rs

Main behavior areas in code:
- Interactive prompt and command loop
- Built-in command handling
- kubectl forwarding
- Completion and caching
- Config and history loading
