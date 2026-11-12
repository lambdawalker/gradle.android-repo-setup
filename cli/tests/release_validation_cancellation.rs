#![cfg(unix)]
use std::{
    fs,
    path::Path,
    process::{Command, Output},
    time::{Duration, Instant},
};
use tempfile::TempDir;
fn git(root: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
#[test]
fn signals_terminate_running_validation_and_remove_worktree() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let root = TempDir::new().unwrap();
        git(root.path(), &["init"]);
        git(root.path(), &["config", "user.name", "Test"]);
        git(
            root.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        fs::create_dir_all(root.path().join(".project/verification")).unwrap();
        fs::write(root.path().join("sentinel"), "original\n").unwrap();
        fs::write(root.path().join(".project/verification/checks.json"), serde_json::to_vec(&serde_json::json!({
            "release_validation": [["sh", "-c", "echo changed > sentinel; sleep 60 & echo $! > child.pid; touch ready; wait"]]
        })).unwrap()).unwrap();
        git(root.path(), &["add", "."]);
        git(root.path(), &["commit", "-m", "fixture"]);
        let source = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"]).stdout).unwrap();
        let before = git(root.path(), &["worktree", "list", "--porcelain"]).stdout;
        let mut child = Command::new(env!("CARGO_BIN_EXE_android-repo-setup"))
            .arg("--project")
            .arg(root.path())
            .args(["release-validate", "--source", source.trim()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let checkout = loop {
            let listing =
                String::from_utf8(git(root.path(), &["worktree", "list", "--porcelain"]).stdout)
                    .unwrap();
            if let Some(path) = listing
                .lines()
                .filter_map(|s| s.strip_prefix("worktree "))
                .map(std::path::PathBuf::from)
                .find(|p| *p != root.path() && p.join("ready").exists())
            {
                break path;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("Validation command never started");
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "Validation exited before cancellation"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let descendant: i32 = fs::read_to_string(checkout.join("child.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // Send only to the CLI, not its child: the CLI must cancel its own tree.
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("Cancellation hung");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cancelled"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!checkout.exists(), "Temporary checkout remained");
        assert_eq!(
            git(root.path(), &["worktree", "list", "--porcelain"]).stdout,
            before
        );
        assert_eq!(
            fs::read_to_string(root.path().join("sentinel")).unwrap(),
            "original\n"
        );
        assert!(git(root.path(), &["status", "--porcelain"])
            .stdout
            .is_empty());
        // On Linux an orphan may briefly remain as a reparented zombie, but no
        // validation process may still execute after the CLI has returned.
        #[cfg(target_os = "linux")]
        if let Ok(stat) = fs::read_to_string(format!("/proc/{descendant}/stat")) {
            assert!(
                stat.split_once(") ").unwrap().1.starts_with('Z'),
                "Descendant still running: {stat}"
            );
        }
    }
}
