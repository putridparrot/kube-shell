mod multi_logs;
mod types;
mod constants;
mod completion;
mod kubectl;
mod config;
mod commands;
mod interrupt;
mod jobs;
mod ai;

use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::{Config, Editor};

use types::{KubeShellHelper, ShellState, OutputProfile};
use jobs::JobManager;
use kubectl::current_context;
use config::*;
use commands::execute_kubectl_command;

fn main() {
    if let Err(err) = interrupt::install_ctrlc_handler() {
        eprintln!("{err}");
        return;
    }

    println!("kube-shell started. Type 'exit' or 'quit' to leave.");
    println!(
        "Built-ins: !!, help, ns, ctx, use/switch, alias, view, dryrun, trace, restart, restart-reason, tail, jobs, fg, ask, ai"
    );

    let config_path = resolve_config_file();
    let aliases_path = aliases_file();
    let exec_commands = load_exec_commands(&config_path);
    let hint_color_prefix = load_hint_color_prefix(&config_path);
    let mut aliases = load_aliases(&config_path);
    aliases.extend(load_runtime_aliases(&aliases_path));
    let dry_run = load_dry_run(&config_path);
    let show_commands = load_show_commands(&config_path);
    let session_namespace_mode = load_session_namespace_mode(&config_path);
    let safe_delete = load_safe_delete(&config_path);
    let risky_contexts = load_risky_contexts(&config_path);
    let prompt_template = load_prompt_template(&config_path);
    let ai_url = load_ai_url(&config_path);
    let ai_model = load_ai_model(&config_path);
    let ai_ask_prompt_template = load_ai_ask_prompt_template(&config_path);
    let ai_explain_prompt_template = load_ai_explain_prompt_template(&config_path);
    let state_path = state_file();
    let (saved_profile, saved_dry_run, saved_show_commands, saved_prev_ctx, saved_prev_ns) =
        load_shell_state(&state_path);

    let mut shell_state = ShellState {
        aliases,
        aliases_file: aliases_path,
        output_profile: saved_profile.unwrap_or(OutputProfile::Default),
        dry_run: saved_dry_run.unwrap_or(dry_run),
        show_commands: saved_show_commands.unwrap_or(show_commands),
        session_namespace_mode,
        session_namespace: if session_namespace_mode {
            Some(kubectl::current_namespace())
        } else {
            None
        },
        safe_delete,
        risky_contexts,
        previous_context: saved_prev_ctx,
        previous_namespace: saved_prev_ns,
        prompt_template,
        state_file: state_path,
        job_manager: JobManager::new(),
        ai_client: ai::AiClient::new(
            ai_url,
            ai_model,
            ai_ask_prompt_template,
            ai_explain_prompt_template,
        ),
    };
    let mut last_command: Option<String> = None;

    let config = Config::builder()
        .history_ignore_dups(true)
        .expect("history_ignore_dups should be configurable")
        .build();

    let mut rl = match Editor::<KubeShellHelper, FileHistory>::with_config(config) {
        Ok(editor) => editor,
        Err(err) => {
            eprintln!("Failed to initialize line editor: {err}");
            return;
        }
    };

    rl.set_helper(Some(KubeShellHelper::new(
        exec_commands,
        hint_color_prefix,
        shell_state.aliases.keys().cloned().collect(),
    )));

    let history_file = history_file();
    if history_file.exists() {
        if let Err(err) = rl.load_history(&history_file) {
            eprintln!("Warning: failed to load history: {err}");
        }
    }

    loop {
        // Clear any stale foreground-interrupt marker before waiting for prompt input.
        let _ = interrupt::consume_pending_interrupt();

        // Notify user of any background jobs that finished since the last prompt.
        shell_state.job_manager.notify_finished();

        if let Some(helper) = rl.helper_mut() {
            helper.set_alias_names(shell_state.aliases.keys().cloned().collect());
        }

        let cluster = current_context();
        let namespace = commands::effective_namespace(&shell_state);
        let risk_marker = if shell_state.risky_contexts.contains(&cluster) {
            "[RISK] "
        } else {
            ""
        };
        let prompt = commands::render_prompt(
            &shell_state.prompt_template,
            risk_marker,
            &cluster,
            &namespace,
        );

        match rl.readline(&prompt) {
            Ok(input) => {
                let mut command = input.trim().to_string();

                if command.is_empty() {
                    continue;
                }

                if command == "!!" {
                    let Some(previous) = last_command.clone() else {
                        println!("No previous command to re-run.");
                        continue;
                    };

                    println!("{previous}");
                    command = previous;
                }

                if matches!(command.as_str(), "exit" | "quit") {
                    break;
                }

                if let Err(err) = rl.add_history_entry(command.as_str()) {
                    eprintln!("Warning: failed to record history entry: {err}");
                }

                last_command = Some(command.clone());

                if let Err(err) = execute_kubectl_command(command.as_str(), &mut shell_state) {
                    eprintln!("{err}");
                } else {
                    if let Err(err) = save_shell_state(&shell_state) {
                        eprintln!("Warning: {err}");
                    }

                    if let Err(err) = save_runtime_aliases(&shell_state.aliases_file, &shell_state.aliases) {
                        eprintln!("Warning: {err}");
                    }
                }
            }
            Err(ReadlineError::Eof) => {
                println!();
                break;
            }
            Err(ReadlineError::Interrupted) => {
                if interrupt::consume_pending_interrupt() {
                    println!();
                    continue;
                }
                println!();
                break;
            }
            Err(err) => {
                eprintln!("Failed to read input: {err}");
                break;
            }
        }
    }

    if let Err(err) = rl.save_history(&history_file) {
        eprintln!("Warning: failed to save history: {err}");
    }

    if let Err(err) = save_shell_state(&shell_state) {
        eprintln!("Warning: {err}");
    }

    if let Err(err) = save_runtime_aliases(&shell_state.aliases_file, &shell_state.aliases) {
        eprintln!("Warning: {err}");
    }

    println!("Bye.");
}
