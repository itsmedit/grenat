//! A server's process group: the server leads a group of its own, so that
//! killing it kills what it started too — the interpreter a `bin/server`
//! script runs without `exec`, a worker it spawned — rather than leaving
//! them running, orphaned, after a timeout or the end of the program.

use std::process::{Child, Command};

/// Starts `command`'s process as the leader of a new process group.
pub fn lead(command: &mut Command) {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    #[cfg(not(unix))]
    let _ = command;
}

/// Kills the group `child` leads, then `child` itself, and waits for it.
pub fn kill(child: &mut Child) {
    #[cfg(unix)]
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: a plain system call; the group is the one `lead` made
        // for `child`, not waited for yet, so its id is still its own
        unsafe {
            libc::killpg(group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn alive(pid: &str) -> bool {
        Command::new("kill").args(["-0", pid]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
    }

    #[test]
    fn killing_a_leader_kills_what_it_started() {
        let pidfile = std::env::temp_dir().join(format!("grenat-bridge-group-{}", std::process::id()));
        let _ = std::fs::remove_file(&pidfile);
        // a wrapper that does not `exec`: its child outlives it unless the group is killed
        let script = format!("sh -c 'echo $$ > {}; exec sleep 30'; true", pidfile.display());
        let mut command = Command::new("sh");
        command.args(["-c", &script]);
        lead(&mut command);
        let mut child = command.spawn().unwrap();
        let started = Instant::now();
        let pid = loop {
            match std::fs::read_to_string(&pidfile) {
                Ok(pid) if pid.ends_with('\n') => break pid.trim().to_string(),
                _ if started.elapsed() < Duration::from_secs(10) => std::thread::sleep(Duration::from_millis(10)),
                _ => panic!("the wrapper did not start its child"),
            }
        };
        assert!(alive(&pid));
        kill(&mut child);
        // the orphan is killed (reaped by init soon after)
        let killed = Instant::now();
        while alive(&pid) && killed.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!alive(&pid), "the child of the wrapper still runs");
        let _ = std::fs::remove_file(&pidfile);
    }
}
