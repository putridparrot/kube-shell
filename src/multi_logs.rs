use regex::Regex;
use chrono::{DateTime, Local, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use crate::interrupt::ForegroundCommandGuard;

fn run_kubectl_capture(args: &[&str]) -> Result<String, String> {
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

fn kubectl_lines(args: &[&str]) -> Vec<String> {
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

fn kubectl_object_names(resource: &str, namespace: Option<&str>) -> Vec<String> {
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

fn parse_selection_indexes(input: &str, max: usize) -> Result<Vec<usize>, String> {
    if input.trim().eq_ignore_ascii_case("all") {
        return Ok((0..max).collect());
    }

    let mut selected = HashSet::new();
    for part in input.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if let Some((start, end)) = part.split_once('-') {
            let start = start
                .trim()
                .parse::<usize>()
                .map_err(|_| format!("Invalid range start: {part}"))?;
            let end = end
                .trim()
                .parse::<usize>()
                .map_err(|_| format!("Invalid range end: {part}"))?;
            if start == 0 || end == 0 || start > end || end > max {
                return Err(format!("Selection range out of bounds: {part}"));
            }
            for idx in start..=end {
                selected.insert(idx - 1);
            }
            continue;
        }

        let idx = part
            .parse::<usize>()
            .map_err(|_| format!("Invalid selection number: {part}"))?;
        if idx == 0 || idx > max {
            return Err(format!("Selection out of range: {idx}"));
        }
        selected.insert(idx - 1);
    }

    if selected.is_empty() {
        return Err("No valid selections provided".to_string());
    }

    let mut indexes: Vec<usize> = selected.into_iter().collect();
    indexes.sort_unstable();
    Ok(indexes)
}

fn select_many_from_list(prompt: &str, items: &[String]) -> Result<Vec<String>, String> {
    if items.is_empty() {
        return Ok(Vec::new());
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
            println!(
                "{} matches{}:",
                visible.len(),
                if query.is_empty() { "" } else { " (filtered)" }
            );
            for (idx, item) in visible.iter().take(25).enumerate() {
                println!("{:>2}. {}", idx + 1, item);
            }

            if visible.len() > 25 {
                println!("... {} more", visible.len() - 25);
            }
        }

        print!(
            "{} (numbers like 1,3-5 or all; text to filter; /clear to reset; empty to cancel): ",
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
            return Ok(Vec::new());
        }

        if trimmed == "/clear" {
            query.clear();
            continue;
        }

        if trimmed
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, ',' | '-' | ' '))
            || trimmed.eq_ignore_ascii_case("all")
        {
            let indexes = parse_selection_indexes(trimmed, visible.len().min(25))?;
            let selected = indexes
                .into_iter()
                .map(|idx| visible[idx].clone())
                .collect::<Vec<String>>();
            return Ok(selected);
        }

        query = trimmed.to_string();
    }
}

fn has_namespace_flag(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "-n" || arg == "--namespace" || arg.starts_with("--namespace=") || arg.starts_with("-n=")
    })
}

fn has_follow_flag(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "-f" || arg == "--follow")
}

fn has_timestamps_flag(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--timestamps" || arg == "--timestamps=true")
}

fn explicit_namespace(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-n" || args[i] == "--namespace" {
            if i + 1 < args.len() {
                return Some(args[i + 1].to_string());
            }
            return None;
        }

        if let Some(ns) = args[i].strip_prefix("--namespace=")
            && !ns.is_empty()
        {
            return Some(ns.to_string());
        }

        i += 1;
    }

    None
}

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

fn print_kubectl_command(args: &[String]) {
    let rendered = args
        .iter()
        .map(|arg| shell_escape_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    println!("+ kubectl {rendered}");
}

fn pod_color(idx: usize) -> &'static str {
    const COLORS: [&str; 8] = [
        "\x1b[38;5;39m",
        "\x1b[38;5;46m",
        "\x1b[38;5;214m",
        "\x1b[38;5;207m",
        "\x1b[38;5;81m",
        "\x1b[38;5;190m",
        "\x1b[38;5;203m",
        "\x1b[38;5;44m",
    ];
    COLORS[idx % COLORS.len()]
}

enum Matcher {
    Substring {
        needle: String,
        ignore_case: bool,
    },
    Regex(Regex),
}

impl Matcher {
    fn is_match(&self, line: &str) -> bool {
        match self {
            Matcher::Substring {
                needle,
                ignore_case,
            } => {
                if *ignore_case {
                    line.to_ascii_lowercase().contains(needle)
                } else {
                    line.contains(needle)
                }
            }
            Matcher::Regex(re) => re.is_match(line),
        }
    }
}

struct FilterOptions {
    include: Option<Matcher>,
    exclude: Option<Matcher>,
    before: usize,
    after: usize,
    time_range: Option<TimeRange>,
}

#[derive(Clone)]
struct TimeRange {
    from_utc: Option<DateTime<Utc>>,
    to_utc: Option<DateTime<Utc>>,
}

#[derive(Default)]
struct PodFilterState {
    before_queue: VecDeque<(u64, String)>,
    after_remaining: usize,
    line_no: u64,
    emitted_until: u64,
}

fn parse_usize(value: &str, flag_name: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("Invalid value for {flag_name}: {value}"))
}

fn resolve_local_datetime(value: &str, is_end: bool) -> Result<DateTime<Utc>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("Time value cannot be empty".to_string());
    }

    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(dt.with_timezone(&Utc));
    }

    let local_dt = if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M") {
        naive
    } else if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S") {
        naive
    } else if let Ok(time) = NaiveTime::parse_from_str(trimmed, "%H:%M") {
        let today = Local::now().date_naive();
        NaiveDateTime::new(today, time)
    } else if let Ok(time) = NaiveTime::parse_from_str(trimmed, "%H:%M:%S") {
        let today = Local::now().date_naive();
        NaiveDateTime::new(today, time)
    } else if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        let time = if is_end {
            NaiveTime::from_hms_opt(23, 59, 59).expect("valid end-of-day time")
        } else {
            NaiveTime::from_hms_opt(0, 0, 0).expect("valid start-of-day time")
        };
        NaiveDateTime::new(date, time)
    } else {
        return Err(format!(
            "Invalid time value '{trimmed}'. Use HH:MM, YYYY-MM-DD, YYYY-MM-DD HH:MM, or RFC3339"
        ));
    };

    match Local.from_local_datetime(&local_dt) {
        LocalResult::Single(dt) => Ok(dt.with_timezone(&Utc)),
        LocalResult::Ambiguous(earliest, latest) => {
            if is_end {
                Ok(latest.with_timezone(&Utc))
            } else {
                Ok(earliest.with_timezone(&Utc))
            }
        }
        LocalResult::None => Err(format!("Local time '{trimmed}' is not valid in this timezone")),
    }
}

fn parse_time_range(args: &[String]) -> Result<(Option<TimeRange>, Vec<String>), String> {
    let mut from_raw: Option<String> = None;
    let mut to_raw: Option<String> = None;
    let mut filtered_args: Vec<String> = Vec::new();

    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];

        if arg == "--from" {
            if idx + 1 >= args.len() {
                return Err("Usage: logs [..] --from <time> [--to <time>]".to_string());
            }
            from_raw = Some(args[idx + 1].clone());
            idx += 2;
            continue;
        }

        if arg == "--to" {
            if idx + 1 >= args.len() {
                return Err("Usage: logs [..] [--from <time>] --to <time>".to_string());
            }
            to_raw = Some(args[idx + 1].clone());
            idx += 2;
            continue;
        }

        if let Some(value) = arg.strip_prefix("--from=") {
            from_raw = Some(value.to_string());
            idx += 1;
            continue;
        }

        if let Some(value) = arg.strip_prefix("--to=") {
            to_raw = Some(value.to_string());
            idx += 1;
            continue;
        }

        filtered_args.push(arg.clone());
        idx += 1;
    }

    if from_raw.is_none() && to_raw.is_none() {
        return Ok((None, filtered_args));
    }

    let from_utc = match from_raw {
        Some(value) => Some(resolve_local_datetime(&value, false)?),
        None => None,
    };
    let to_utc = match to_raw {
        Some(value) => Some(resolve_local_datetime(&value, true)?),
        None => None,
    };

    if let (Some(from), Some(to)) = (from_utc.as_ref(), to_utc.as_ref())
        && from > to
    {
        return Err("--from must be earlier than or equal to --to".to_string());
    }

    Ok((Some(TimeRange { from_utc, to_utc }), filtered_args))
}

fn line_timestamp_utc(line: &str) -> Option<DateTime<Utc>> {
    let candidate = line.split_whitespace().next()?;
    DateTime::parse_from_rfc3339(candidate)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn line_in_time_range(line: &str, range: &TimeRange) -> bool {
    let Some(ts) = line_timestamp_utc(line) else {
        return false;
    };

    if let Some(from) = range.from_utc.as_ref()
        && ts < *from
    {
        return false;
    }

    if let Some(to) = range.to_utc.as_ref()
        && ts > *to
    {
        return false;
    }

    true
}

fn build_matcher(pattern: &str, is_regex: bool, ignore_case: bool) -> Result<Matcher, String> {
    if is_regex {
        let compiled = if ignore_case {
            Regex::new(&format!("(?i){pattern}"))
        } else {
            Regex::new(pattern)
        }
        .map_err(|err| format!("Invalid regex '{pattern}': {err}"))?;

        return Ok(Matcher::Regex(compiled));
    }

    let needle = if ignore_case {
        pattern.to_ascii_lowercase()
    } else {
        pattern.to_string()
    };

    Ok(Matcher::Substring {
        needle,
        ignore_case,
    })
}

fn parse_multi_log_options(args: &[String]) -> Result<(Vec<String>, bool, bool, FilterOptions), String> {
    let mut include_elapsed_ts = true;
    let mut align_pod_column = true;

    let mut include_pattern: Option<String> = None;
    let mut exclude_pattern: Option<String> = None;
    let mut before: usize = 0;
    let mut after: usize = 0;
    let mut ignore_case = false;
    let mut use_regex = false;

    let (time_range, parsed_args) = parse_time_range(args)?;
    let mut base_logs_args: Vec<String> = Vec::new();

    let mut idx = 1;
    while idx < parsed_args.len() {
        let arg = &parsed_args[idx];

        match arg.as_str() {
            "--multi" | "--pick" => {
                idx += 1;
            }
            "--no-ts" => {
                include_elapsed_ts = false;
                idx += 1;
            }
            "--no-align" => {
                align_pod_column = false;
                idx += 1;
            }
            "--ignore-case" => {
                ignore_case = true;
                idx += 1;
            }
            "--regex" => {
                use_regex = true;
                idx += 1;
            }
            "--include" => {
                if idx + 1 >= args.len() {
                    return Err("Usage: logs --multi|--pick --include <pattern>".to_string());
                }
                include_pattern = Some(parsed_args[idx + 1].clone());
                idx += 2;
            }
            "--exclude" => {
                if idx + 1 >= parsed_args.len() {
                    return Err("Usage: logs --multi|--pick --exclude <pattern>".to_string());
                }
                exclude_pattern = Some(parsed_args[idx + 1].clone());
                idx += 2;
            }
            "--before" => {
                if idx + 1 >= parsed_args.len() {
                    return Err("Usage: logs --multi|--pick --before <N>".to_string());
                }
                before = parse_usize(&parsed_args[idx + 1], "--before")?;
                idx += 2;
            }
            "--after" => {
                if idx + 1 >= parsed_args.len() {
                    return Err("Usage: logs --multi|--pick --after <N>".to_string());
                }
                after = parse_usize(&parsed_args[idx + 1], "--after")?;
                idx += 2;
            }
            _ => {
                if let Some(value) = arg.strip_prefix("--include=") {
                    include_pattern = Some(value.to_string());
                    idx += 1;
                } else if let Some(value) = arg.strip_prefix("--exclude=") {
                    exclude_pattern = Some(value.to_string());
                    idx += 1;
                } else if let Some(value) = arg.strip_prefix("--before=") {
                    before = parse_usize(value, "--before")?;
                    idx += 1;
                } else if let Some(value) = arg.strip_prefix("--after=") {
                    after = parse_usize(value, "--after")?;
                    idx += 1;
                } else {
                    base_logs_args.push(arg.clone());
                    idx += 1;
                }
            }
        }
    }

    let include = match include_pattern {
        Some(pattern) => Some(build_matcher(&pattern, use_regex, ignore_case)?),
        None => None,
    };
    let exclude = match exclude_pattern {
        Some(pattern) => Some(build_matcher(&pattern, use_regex, ignore_case)?),
        None => None,
    };

    Ok((
        base_logs_args,
        include_elapsed_ts,
        align_pod_column,
        FilterOptions {
            include,
            exclude,
            before,
            after,
            time_range,
        },
    ))
}

fn stream_logs_for_pods(
    pods: &[String],
    namespace: &str,
    base_logs_args: &[String],
    show_commands: bool,
    include_elapsed_ts: bool,
    align_pod_column: bool,
    filters: FilterOptions,
) -> Result<(), String> {
    let _guard = ForegroundCommandGuard::new();
    let (tx, rx) = mpsc::channel::<(usize, String, String)>();

    for (idx, pod) in pods.iter().enumerate() {
        let mut args = vec!["logs".to_string()];
        args.extend(base_logs_args.iter().cloned());
        if !has_namespace_flag(&args) {
            args.push("-n".to_string());
            args.push(namespace.to_string());
        }
        if filters.time_range.is_some() && !has_timestamps_flag(&args) {
            args.push("--timestamps=true".to_string());
        }
        if filters.time_range.is_none() && !has_follow_flag(&args) {
            args.push("-f".to_string());
        }
        args.push(pod.clone());

        if show_commands {
            print_kubectl_command(&args);
        }

        let tx_clone = tx.clone();
        let pod_name = pod.clone();
        thread::spawn(move || {
            let spawn_result = Command::new("kubectl")
                .args(args.iter().map(String::as_str))
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn();

            let mut child = match spawn_result {
                Ok(child) => child,
                Err(err) => {
                    let _ = tx_clone.send((
                        idx,
                        pod_name,
                        format!("Failed to start kubectl logs: {err}"),
                    ));
                    return;
                }
            };

            if let Some(stdout) = child.stdout.take() {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(line) => {
                            let _ = tx_clone.send((idx, pod_name.clone(), line));
                        }
                        Err(err) => {
                            let _ = tx_clone.send((
                                idx,
                                pod_name.clone(),
                                format!("Error reading log output: {err}"),
                            ));
                            break;
                        }
                    }
                }
            }

            if let Ok(status) = child.wait()
                && !status.success()
            {
                let _ = tx_clone.send((
                    idx,
                    pod_name,
                    format!("kubectl logs exited with status: {status}"),
                ));
            }
        });
    }

    drop(tx);

    let start = Instant::now();
    let pod_width = if align_pod_column {
        pods.iter().map(|p| p.len()).max().unwrap_or(1)
    } else {
        0
    };

    let mut pod_states: HashMap<String, PodFilterState> = HashMap::new();

    for (idx, pod, line) in rx {
        let state = pod_states.entry(pod.clone()).or_default();
        state.line_no += 1;
        let current_no = state.line_no;

        if let Some(range) = filters.time_range.as_ref()
            && !line_in_time_range(&line, range)
        {
            continue;
        }

        if filters.exclude.as_ref().is_some_and(|m| m.is_match(&line)) {
            continue;
        }

        let include_match = filters.include.as_ref().is_none_or(|m| m.is_match(&line));

        let color = pod_color(idx);
        let print_line = |line_text: &str| {
            if include_elapsed_ts {
                let elapsed = start.elapsed().as_secs_f32();
                if align_pod_column {
                    println!(
                        "{color}[+{elapsed:7.3}s] [{pod:<pod_width$}] {line_text}\x1b[0m",
                        pod_width = pod_width
                    );
                } else {
                    println!("{color}[+{elapsed:7.3}s] [{pod}] {line_text}\x1b[0m");
                }
            } else if align_pod_column {
                println!(
                    "{color}[{pod:<pod_width$}] {line_text}\x1b[0m",
                    pod_width = pod_width
                );
            } else {
                println!("{color}[{pod}] {line_text}\x1b[0m");
            }
        };

        if filters.include.is_some() {
            if include_match {
                for (line_no, ctx_line) in state.before_queue.iter() {
                    if *line_no > state.emitted_until {
                        print_line(ctx_line);
                        state.emitted_until = *line_no;
                    }
                }
                if current_no > state.emitted_until {
                    print_line(&line);
                    state.emitted_until = current_no;
                }
                state.after_remaining = filters.after;
            } else if state.after_remaining > 0 {
                if current_no > state.emitted_until {
                    print_line(&line);
                    state.emitted_until = current_no;
                }
                state.after_remaining -= 1;
            }
        } else {
            print_line(&line);
        }

        if filters.before > 0 {
            state.before_queue.push_back((current_no, line));
            while state.before_queue.len() > filters.before {
                state.before_queue.pop_front();
            }
        }
    }

    Ok(())
}

pub fn execute_multi_logs_command(
    args: &[String],
    show_commands: bool,
    current_namespace: String,
) -> Result<(), String> {
    let (base_logs_args, include_elapsed_ts, align_pod_column, filters) =
        parse_multi_log_options(args)?;

    let namespace = explicit_namespace(&base_logs_args).unwrap_or(current_namespace);

    let pods = kubectl_object_names("pods", Some(namespace.as_str()));
    if pods.is_empty() {
        return Err(format!("No pods found in namespace '{namespace}'"));
    }

    let selected = select_many_from_list("Select pods for logs", &pods)?;
    if selected.is_empty() {
        println!("Log selection cancelled.");
        return Ok(());
    }

    stream_logs_for_pods(
        &selected,
        &namespace,
        &base_logs_args,
        show_commands,
        include_elapsed_ts,
        align_pod_column,
        filters,
    )
}

pub fn stream_logs_for_pod_list(
    pods: &[String],
    namespace: &str,
    base_logs_args: &[String],
    show_commands: bool,
) -> Result<(), String> {
    if pods.is_empty() {
        return Err("No pods selected for logs".to_string());
    }

    stream_logs_for_pods(
        pods,
        namespace,
        base_logs_args,
        show_commands,
        true,
        true,
        FilterOptions {
            include: None,
            exclude: None,
            before: 0,
            after: 0,
            time_range: None,
        },
    )
}

pub fn stream_logs_for_pod_list_with_filters(
    pods: &[String],
    namespace: &str,
    logs_args: &[String],
    show_commands: bool,
) -> Result<(), String> {
    if pods.is_empty() {
        return Err("No pods selected for logs".to_string());
    }

    let (base_logs_args, include_elapsed_ts, align_pod_column, filters) =
        parse_multi_log_options(logs_args)?;

    stream_logs_for_pods(
        pods,
        namespace,
        &base_logs_args,
        show_commands,
        include_elapsed_ts,
        align_pod_column,
        filters,
    )
}

pub fn execute_filtered_logs_command(args: &[String], show_commands: bool) -> Result<(), String> {
    let (time_range, mut kubectl_args) = parse_time_range(args)?;
    let Some(range) = time_range else {
        return Err("No log time range provided".to_string());
    };

    let _guard = ForegroundCommandGuard::new();

    if !has_timestamps_flag(&kubectl_args) {
        kubectl_args.push("--timestamps=true".to_string());
    }

    if show_commands {
        print_kubectl_command(&kubectl_args);
    }

    let mut child = Command::new("kubectl")
        .args(kubectl_args.iter().map(String::as_str))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|err| format!("Failed to start kubectl logs: {err}"))?;

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = line.map_err(|err| format!("Error reading log output: {err}"))?;
            if line_in_time_range(&line, &range) {
                println!("{line}");
            }
        }
    }

    let status = child
        .wait()
        .map_err(|err| format!("Failed waiting for kubectl logs: {err}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("kubectl logs exited with status: {status}"))
    }
}
