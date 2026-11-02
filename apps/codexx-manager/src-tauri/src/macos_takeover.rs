use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const RECENT_ACTIVITY_WINDOW: Duration = Duration::from_secs(10 * 60);
const QUIT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq, Eq)]
struct CodexProcess {
    pid: u32,
    app_name: String,
    has_debugging_port: bool,
}

pub fn start() {
    let _ = thread::Builder::new()
        .name("codexgo-auto-takeover".to_string())
        .spawn(supervise);
}

fn supervise() {
    let mut observed = HashMap::<u32, Instant>::new();
    let mut protected = HashSet::<u32>::new();
    loop {
        if let Ok(settings) = codexx_core::settings::SettingsStore::default().load() {
            if settings.setup_completed && settings.macos_auto_takeover_enabled {
                supervise_once(&mut observed, &mut protected);
            } else {
                observed.clear();
                protected.clear();
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn supervise_once(observed: &mut HashMap<u32, Instant>, protected: &mut HashSet<u32>) {
    let processes = list_codex_processes();
    let process_ids = processes
        .iter()
        .map(|process| process.pid)
        .collect::<HashSet<_>>();
    observed.retain(|pid, _| process_ids.contains(pid));
    protected.retain(|pid| process_ids.contains(pid));

    if processes.len() != 1 {
        return;
    }
    let process = &processes[0];
    if protected.contains(&process.pid) {
        return;
    }
    let first_seen = *observed.entry(process.pid).or_insert_with(Instant::now);
    let observed_for = first_seen.elapsed();
    if observed_for < Duration::from_secs(30) {
        return;
    }

    if has_recent_codex_activity() {
        protected.insert(process.pid);
        log_event("manager.auto_takeover.skipped_recent_activity", process.pid);
        return;
    }
    if !codexx_core::auto_takeover::should_auto_takeover(
        processes.len(),
        process.has_debugging_port,
        observed_for,
        false,
    ) {
        return;
    }

    if !request_normal_quit(&process.app_name) {
        protected.insert(process.pid);
        log_event("manager.auto_takeover.quit_request_failed", process.pid);
        return;
    }
    log_event("manager.auto_takeover.quit_requested", process.pid);

    let deadline = Instant::now() + QUIT_TIMEOUT;
    while Instant::now() < deadline {
        if !list_codex_processes()
            .iter()
            .any(|candidate| candidate.pid == process.pid)
        {
            if let Ok(current_settings) = codexx_core::settings::SettingsStore::default().load() {
                if current_settings.setup_completed && current_settings.macos_auto_takeover_enabled
                {
                    let request = crate::commands::LaunchRequest {
                        app_path: current_settings.codex_app_path,
                        debug_port: 9329,
                        helper_port: 58321,
                    };
                    let result = crate::commands::launch_codex_plus(request);
                    log_event(
                        if result.status == "accepted" {
                            "manager.auto_takeover.launch_started"
                        } else {
                            "manager.auto_takeover.launch_failed"
                        },
                        process.pid,
                    );
                }
            }
            observed.remove(&process.pid);
            return;
        }
        thread::sleep(Duration::from_millis(400));
    }

    protected.insert(process.pid);
    log_event("manager.auto_takeover.quit_timeout", process.pid);
}

fn list_codex_processes() -> Vec<CodexProcess> {
    let Ok(output) = Command::new("ps").args(["-axo", "pid=,args="]).output() else {
        return Vec::new();
    };
    parse_codex_processes(&String::from_utf8_lossy(&output.stdout))
}

fn parse_codex_processes(output: &str) -> Vec<CodexProcess> {
    output
        .lines()
        .filter_map(|line| {
            let (pid, args) = line.trim_start().split_once(char::is_whitespace)?;
            let pid = pid.parse().ok()?;
            let app_name = app_name_from_process_args(args)?;
            let has_debugging_port = args
                .to_ascii_lowercase()
                .contains("--remote-debugging-port");
            Some(CodexProcess {
                pid,
                app_name,
                has_debugging_port,
            })
        })
        .collect()
}

fn app_name_from_process_args(args: &str) -> Option<String> {
    let is_main_process = (args.contains(".app/Contents/MacOS/Codex")
        || args.contains(".app/Contents/MacOS/ChatGPT"))
        && !args.contains(".app/Contents/Frameworks/")
        && !args.contains(" Helper.app/")
        && !args.contains("/Helpers/");
    if !is_main_process {
        return None;
    }
    let marker = ".app/Contents/MacOS/";
    let app_path = args.split_once(marker)?.0;
    let app_name = Path::new(app_path).file_name()?.to_str()?;
    matches!(app_name, "Codex" | "ChatGPT").then(|| app_name.to_string())
}

fn request_normal_quit(app_name: &str) -> bool {
    if !matches!(app_name, "Codex" | "ChatGPT") {
        return false;
    }
    Command::new("osascript")
        .args(["-e", &format!("tell application \"{app_name}\" to quit")])
        .status()
        .is_ok_and(|status| status.success())
}

fn has_recent_codex_activity() -> bool {
    let home = codexx_core::codex_home::default_codex_home_dir();
    let cutoff = SystemTime::now() - RECENT_ACTIVITY_WINDOW;
    latest_activity_under(&home.join("sessions"), 4, cutoff)
        || [home.join("logs_2.sqlite"), home.join("sqlite")]
            .iter()
            .any(|path| latest_activity_under(path, 2, cutoff))
}

fn latest_activity_under(path: &Path, max_depth: usize, cutoff: SystemTime) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if metadata.is_file() {
        return metadata.modified().is_ok_and(|modified| modified >= cutoff);
    }
    if max_depth == 0 {
        return false;
    }
    std::fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .take(10_000)
        .any(|entry| latest_activity_under(&entry.path(), max_depth - 1, cutoff))
}

fn log_event(event: &str, pid: u32) {
    let _ = codexx_core::diagnostic_log::append_diagnostic_log(
        event,
        serde_json::json!({ "pid": pid }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_codex_main_processes_and_debug_flags() {
        let processes = parse_codex_processes(
            "101 /Applications/Codex.app/Contents/MacOS/Codex --user-data-dir=/tmp/codex\n102 /Applications/Codex.app/Contents/Frameworks/Codex Helper.app/Contents/MacOS/Codex Helper\n103 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT --remote-debugging-port=9329\n",
        );

        assert_eq!(processes.len(), 2);
        assert_eq!(processes[0].app_name, "Codex");
        assert!(!processes[0].has_debugging_port);
        assert_eq!(processes[1].app_name, "ChatGPT");
        assert!(processes[1].has_debugging_port);
    }
}
