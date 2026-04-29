use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use crate::jobs::JobManager;
use crate::ai::AiClient;

use rustyline::validate::Validator;
use rustyline::highlight::Highlighter;
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::completion::{Completer, Pair};
use rustyline::{Context, Helper};
use std::borrow::Cow;

/// Output profile for kubectl get commands
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputProfile {
    Default,
    Wide,
    Yaml,
    Json,
}

impl OutputProfile {
    pub fn as_output_value(self) -> Option<&'static str> {
        match self {
            OutputProfile::Default => None,
            OutputProfile::Wide => Some("wide"),
            OutputProfile::Yaml => Some("yaml"),
            OutputProfile::Json => Some("json"),
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "default" | "none" => Some(OutputProfile::Default),
            "wide" => Some(OutputProfile::Wide),
            "yaml" => Some(OutputProfile::Yaml),
            "json" => Some(OutputProfile::Json),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OutputProfile::Default => "default",
            OutputProfile::Wide => "wide",
            OutputProfile::Yaml => "yaml",
            OutputProfile::Json => "json",
        }
    }
}

/// Main shell state managed throughout the session
pub struct ShellState {
    pub aliases: HashMap<String, String>,
    pub aliases_file: PathBuf,
    pub output_profile: OutputProfile,
    pub dry_run: bool,
    pub show_commands: bool,
    pub session_namespace_mode: bool,
    pub session_namespace: Option<String>,
    pub safe_delete: bool,
    pub risky_contexts: HashSet<String>,
    pub previous_context: Option<String>,
    pub previous_namespace: Option<String>,
    pub prompt_template: String,
    pub state_file: PathBuf,
    pub job_manager: JobManager,
    pub ai_client: AiClient,
}

/// Cache for completion data with TTL
pub struct CompletionCache {
    pub refreshed_at: Option<Instant>,
    pub contexts: Vec<String>,
    pub namespaces: Vec<String>,
    pub resources: Vec<String>,
    pub object_names: HashMap<String, (Instant, Vec<String>)>,
    pub pod_containers: HashMap<String, (Instant, Vec<String>)>,
}

/// Helper for rustyline providing completion, hints, and highlighting
pub struct KubeShellHelper {
    pub hinter: HistoryHinter,
    pub cache: Mutex<CompletionCache>,
    pub exec_commands: Vec<String>,
    pub hint_color_prefix: String,
    pub alias_names: Vec<String>,
}

impl KubeShellHelper {
    pub fn new(
        exec_commands: Vec<String>,
        hint_color_prefix: String,
        alias_names: Vec<String>,
    ) -> Self {
        Self {
            hinter: HistoryHinter::default(),
            cache: Mutex::new(CompletionCache {
                refreshed_at: None,
                contexts: Vec::new(),
                namespaces: Vec::new(),
                resources: Vec::new(),
                object_names: HashMap::new(),
                pod_containers: HashMap::new(),
            }),
            exec_commands,
            hint_color_prefix,
            alias_names,
        }
    }

    pub fn exec_inner_commands(&self) -> Vec<String> {
        self.exec_commands.clone()
    }

    pub fn alias_names(&self) -> Vec<String> {
        self.alias_names.clone()
    }

    pub fn set_alias_names(&mut self, names: Vec<String>) {
        self.alias_names = names;
    }

    pub fn with_cache<T, F>(&self, f: F) -> T
    where
        F: FnOnce(&CompletionCache) -> T,
    {
        let cache = self.cache.lock().expect("completion cache lock poisoned");
        f(&cache)
    }
}

impl Helper for KubeShellHelper {}

impl Hinter for KubeShellHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<Self::Hint> {
        if let Some(history_hint) = self.hinter.hint(line, pos, ctx) {
            return Some(history_hint);
        }

        if pos != line.len() {
            return None;
        }

        let start = crate::completion::completion_start(line, pos);
        let current_token = &line[start..pos];
        let previous_tokens: Vec<&str> = line[..start].split_whitespace().collect();

        let mut options = crate::completion::command_candidates_with_live(self, &previous_tokens, current_token);
        options.sort();
        options.dedup();

        let mut matches: Vec<String> = options
            .into_iter()
            .filter(|option| option.starts_with(current_token) && option.len() > current_token.len())
            .collect();

        matches.sort();
        matches.dedup();

        if matches.len() == 1 {
            let completion = &matches[0][current_token.len()..];
            return Some(completion.to_string());
        }

        None
    }
}

impl Highlighter for KubeShellHelper {
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(format!("{}{hint}\x1b[0m", self.hint_color_prefix))
    }
}

impl Validator for KubeShellHelper {}

impl Completer for KubeShellHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let start = crate::completion::completion_start(line, pos);
        let current_token = &line[start..pos];
        let previous_tokens: Vec<&str> = line[..start].split_whitespace().collect();
        let options = crate::completion::command_candidates_with_live(self, &previous_tokens, current_token);
        Ok((start, crate::completion::to_pairs(options, current_token)))
    }
}
