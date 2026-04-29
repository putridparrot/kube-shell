use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::interrupt;

/// Output lines shared between the reader threads and consumers (fg / jobs).
struct JobState {
    lines: Vec<String>,
    done: bool,
}

/// A single background job.
pub struct BackgroundJob {
    pub id: usize,
    pub command: String,
    /// Shared state: buffered output + done flag.
    state: Arc<(Mutex<JobState>, Condvar)>,
    /// The child process, kept for kill().  The poll thread never holds this
    /// lock while blocking, so kill() can always take the child immediately.
    child: Arc<Mutex<Option<Child>>>,
    /// Reader / poll threads.  Kept so their resources are cleaned up when
    /// the job is dropped.
    _threads: Vec<thread::JoinHandle<()>>,
}

impl BackgroundJob {
    pub fn is_done(&self) -> bool {
        let (lock, _) = &*self.state;
        lock.lock().unwrap().done
    }

    pub fn status_label(&self) -> &'static str {
        if self.is_done() { "done" } else { "running" }
    }

    /// Send SIGKILL / TerminateProcess to the background process.
    pub fn kill(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}

/// Manages all background jobs for the shell session.
pub struct JobManager {
    jobs: Vec<BackgroundJob>,
    next_id: usize,
}

impl JobManager {
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            next_id: 1,
        }
    }

    /// Spawn a kubectl command in the background.
    /// `kubectl_args` are the arguments passed to kubectl (e.g. ["port-forward", "svc/foo", "8080:80"]).
    pub fn spawn(&mut self, kubectl_args: &[String], show_commands: bool) -> Result<usize, String> {
        let id = self.next_id;
        self.next_id += 1;

        let command_str = format!("kubectl {}", kubectl_args.join(" "));

        if show_commands {
            println!("{}", command_str);
        }

        let mut child = Command::new("kubectl")
            .args(kubectl_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("Failed to spawn background job: {err}"))?;

        let state: Arc<(Mutex<JobState>, Condvar)> = Arc::new((
            Mutex::new(JobState {
                lines: Vec::new(),
                done: false,
            }),
            Condvar::new(),
        ));

        let child_arc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let mut threads: Vec<thread::JoinHandle<()>> = Vec::new();

        // --- stdout reader thread ---
        if let Some(stdout) = child.stdout.take() {
            let state_clone = Arc::clone(&state);
            threads.push(thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().flatten() {
                    let (lock, cvar) = &*state_clone;
                    lock.lock().unwrap().lines.push(line);
                    cvar.notify_all();
                }
            }));
        }

        // --- stderr reader thread ---
        if let Some(stderr) = child.stderr.take() {
            let state_clone = Arc::clone(&state);
            threads.push(thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().flatten() {
                    let (lock, cvar) = &*state_clone;
                    // Prefix stderr so the user can distinguish it.
                    lock.lock().unwrap().lines.push(format!("[err] {}", line));
                    cvar.notify_all();
                }
            }));
        }

        // Move the child into the shared Arc now that we've taken the pipe handles.
        *child_arc.lock().unwrap() = Some(child);

        // --- poll thread — marks done when the process exits ---
        // Uses try_wait() so it never holds the child_arc lock while blocking,
        // allowing kill() to take the child at any time.
        {
            let child_arc_clone = Arc::clone(&child_arc);
            let state_clone = Arc::clone(&state);
            threads.push(thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_millis(100));

                    let exited = {
                        let mut guard = child_arc_clone.lock().unwrap();
                        match guard.as_mut() {
                            Some(c) => match c.try_wait() {
                                Ok(Some(_)) => true,
                                Ok(None) => false,
                                Err(_) => true,
                            },
                            // Child was taken by kill() — treat as done.
                            None => true,
                        }
                    };

                    if exited {
                        // Give the reader threads a moment to drain the last lines.
                        thread::sleep(Duration::from_millis(80));
                        let (lock, cvar) = &*state_clone;
                        lock.lock().unwrap().done = true;
                        cvar.notify_all();
                        break;
                    }
                }
            }));
        }

        self.jobs.push(BackgroundJob {
            id,
            command: command_str.clone(),
            state,
            child: child_arc,
            _threads: threads,
        });

        println!("[{}] {} &", id, command_str);
        Ok(id)
    }

    /// List all background jobs with their current status.
    pub fn list(&self) {
        if self.jobs.is_empty() {
            println!("No background jobs.");
            return;
        }
        for job in &self.jobs {
            println!("[{}] {}  ({})", job.id, job.command, job.status_label());
        }
    }

    /// Bring a background job to the foreground: print buffered output then
    /// stream live output until the job finishes or the user presses Ctrl+C.
    pub fn foreground(&self, id: usize) -> Result<(), String> {
        let job = self
            .jobs
            .iter()
            .find(|j| j.id == id)
            .ok_or_else(|| format!("No job with id {id}"))?;

        println!("[fg {}] {}", job.id, job.command);
        println!("[fg] Press Ctrl+C to return to shell (job keeps running).");

        let (lock, cvar) = &*job.state;
        let mut last_idx;

        // Print already-buffered lines.
        {
            let state = lock.lock().unwrap();
            for line in &state.lines {
                println!("{}", line);
            }
            last_idx = state.lines.len();
            if state.done {
                println!("[{}] Job has already finished.", id);
                return Ok(());
            }
        }

        // Stream live output until Ctrl+C or process exits.
        let _guard = interrupt::ForegroundCommandGuard::new();
        loop {
            let (done, new_lines) = {
                let state = lock.lock().unwrap();
                // Wake at most every 100 ms so we can check for Ctrl+C.
                let (state, _) = cvar
                    .wait_timeout(state, Duration::from_millis(100))
                    .unwrap();
                let new_lines: Vec<String> = state.lines[last_idx..].to_vec();
                last_idx = state.lines.len();
                (state.done, new_lines)
            };

            for line in &new_lines {
                println!("{}", line);
            }

            if done {
                println!("[{}] Job finished.", id);
                break;
            }

            if interrupt::consume_pending_interrupt() {
                println!("\nBack to shell. Job [{}] still running in background.", id);
                break;
            }
        }

        Ok(())
    }

    /// Kill a background job and remove it from the list.
    pub fn kill_job(&mut self, id: usize) -> Result<(), String> {
        let pos = self
            .jobs
            .iter()
            .position(|j| j.id == id)
            .ok_or_else(|| format!("No job with id {id}"))?;

        self.jobs[pos].kill();
        self.jobs.remove(pos);
        println!("[{}] Job killed.", id);
        Ok(())
    }

    /// Remove all finished jobs from the list.
    pub fn clean(&mut self) {
        let before = self.jobs.len();
        self.jobs.retain(|j| !j.is_done());
        let removed = before - self.jobs.len();
        if removed > 0 {
            println!("Removed {removed} finished job(s).");
        } else {
            println!("No finished jobs to remove.");
        }
    }

    /// Print a notification for any jobs that finished since the last check
    /// (called before each prompt so the user sees completion messages).
    pub fn notify_finished(&self) {
        for job in &self.jobs {
            let (lock, _) = &*job.state;
            let state = lock.lock().unwrap();
            if state.done {
                println!("[{}] Done: {}", job.id, job.command);
            }
        }
    }
}
