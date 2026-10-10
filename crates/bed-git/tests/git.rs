use bed_git::{
    ConflictChoice, Diff, DiffSide, ErrorKind, Job, Operation, Output, RepositoryOperation, Status,
    execute,
};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output as ProcessOutput},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

struct Repo(PathBuf);
impl Repo {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bed-git-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let repo = Self(root);
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.name", "Bed Test"]);
        repo.git(&["config", "user.email", "bed@example.invalid"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.git(&["config", "core.hooksPath", ".git/hooks"]);
        repo.git(&["config", "push.default", "simple"]);
        repo
    }
    fn command(&self, args: &[&str]) -> ProcessOutput {
        Command::new("git")
            .arg("--literal-pathspecs")
            .arg("-C")
            .arg(&self.0)
            .args(args)
            .env("LC_ALL", "C")
            .env("GIT_EDITOR", "true")
            .output()
            .unwrap()
    }
    fn git(&self, args: &[&str]) -> ProcessOutput {
        let out = self.command(args);
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
    fn write(&self, path: &str, bytes: &[u8]) {
        fs::write(self.0.join(path), bytes).unwrap();
    }
    fn run(&self, operation: Operation) -> Output {
        execute(&self.0, &operation).unwrap()
    }
    fn status(&self) -> Status {
        let Output::Status(status) = self.run(Operation::Status) else {
            panic!()
        };
        status
    }
    fn diff(&self, path: &str, side: DiffSide) -> Diff {
        let Output::Diff(diff) = self.run(Operation::Diff {
            path: path.into(),
            side,
        }) else {
            panic!()
        };
        diff
    }
    fn initial(&self) {
        self.write("a.txt", b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n");
        self.run(Operation::Stage { paths: vec![] });
        self.run(Operation::Commit {
            message: "Initial commit".into(),
        });
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn first_commit_and_unborn_unstage() {
    let repo = Repo::new();
    let initial = repo.status();
    assert!(initial.head.is_none());
    assert_eq!(initial.branch, "main");
    assert!(initial.writable);
    repo.write("new.txt", b"new\n");
    repo.run(Operation::Stage { paths: vec![] });
    assert_eq!(repo.status().entries[0].index_status, 'A');
    repo.write("new.txt", b"newer unstaged edit\n");
    repo.run(Operation::Unstage { paths: vec![] });
    assert_eq!(repo.status().entries[0].index_status, '?');
    assert_eq!(
        fs::read(repo.0.join("new.txt")).unwrap(),
        b"newer unstaged edit\n"
    );
    repo.write("new.txt", b"new\n");
    repo.run(Operation::Stage {
        paths: vec!["new.txt".into()],
    });
    let diff = repo.diff("new.txt", DiffSide::Staged);
    assert!(diff.old_bytes.is_empty());
    assert_eq!(diff.new_bytes, b"new\n");
    repo.run(Operation::Commit {
        message: "Create first commit\n\nBody retained".into(),
    });
    assert!(repo.status().head.is_some());
    assert!(repo.status().entries.is_empty());
}

#[test]
fn hunks_stage_and_unstage_independently_with_stale_rejection() {
    let repo = Repo::new();
    repo.initial();
    repo.write("a.txt", b"ONE\ntwo\nthree\nfour\nfive\nsix\nseven\nEIGHT\n");
    let diff = repo.diff("a.txt", DiffSide::Unstaged);
    assert_eq!(diff.hunks.len(), 2);
    assert!(diff.can_apply_hunks);
    assert_eq!(diff.hunks[0].old_start, 0);
    repo.run(Operation::StageHunk {
        path: "a.txt".into(),
        snapshot: diff.snapshot.clone(),
        hunk: 0,
    });
    let status = repo.status();
    assert_eq!(
        (
            status.entries[0].index_status,
            status.entries[0].worktree_status
        ),
        ('M', 'M')
    );
    let err = execute(
        &repo.0,
        &Operation::StageHunk {
            path: "a.txt".into(),
            snapshot: diff.snapshot,
            hunk: 1,
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Stale);
    let staged = repo.diff("a.txt", DiffSide::Staged);
    assert_eq!(staged.hunks.len(), 1);
    assert!(staged.new_bytes.starts_with(b"ONE\n"));
    assert!(staged.new_bytes.ends_with(b"eight\n"));
    repo.run(Operation::UnstageHunk {
        path: "a.txt".into(),
        snapshot: staged.snapshot,
        hunk: 0,
    });
    assert_eq!(repo.status().entries[0].index_status, ' ');
    assert_eq!(repo.diff("a.txt", DiffSide::Unstaged).hunks.len(), 2);
}

#[test]
fn discard_hunk_preserves_other_changes_and_missing_final_newline() {
    let repo = Repo::new();
    repo.initial();
    repo.write("a.txt", b"ONE\ntwo\nthree\nfour\nfive\nsix\nseven\nEIGHT");
    let diff = repo.diff("a.txt", DiffSide::Unstaged);
    repo.run(Operation::DiscardHunk {
        path: "a.txt".into(),
        snapshot: diff.snapshot,
        hunk: 0,
    });
    assert_eq!(
        fs::read(repo.0.join("a.txt")).unwrap(),
        b"one\ntwo\nthree\nfour\nfive\nsix\nseven\nEIGHT"
    );
    let diff = repo.diff("a.txt", DiffSide::Unstaged);
    repo.run(Operation::StageHunk {
        path: "a.txt".into(),
        snapshot: diff.snapshot,
        hunk: 0,
    });
    assert_eq!(
        repo.diff("a.txt", DiffSide::Staged).new_bytes.last(),
        Some(&b'T')
    );
}

#[test]
fn literal_paths_renames_and_untracked_discard() {
    let repo = Repo::new();
    let name = "- odd [:glob]\nfile.txt";
    repo.write(name, b"hello\n");
    repo.run(Operation::Stage {
        paths: vec![name.into()],
    });
    repo.run(Operation::Commit {
        message: "Literal file".into(),
    });
    repo.git(&["mv", "--", name, "renamed.txt"]);
    let status = repo.status();
    assert_eq!(status.entries[0].old_path.as_deref(), Some(name));
    assert_eq!(
        repo.diff("renamed.txt", DiffSide::Staged).old_bytes,
        b"hello\n"
    );
    repo.run(Operation::Unstage {
        paths: vec!["renamed.txt".into()],
    });
    let status = repo.status();
    assert!(
        status
            .entries
            .iter()
            .any(|entry| entry.path == name && entry.worktree_status == 'D')
    );
    repo.write(":(glob)*", b"literal\n");
    let diff = repo.diff(":(glob)*", DiffSide::Unstaged);
    assert_eq!(diff.new_bytes, b"literal\n");
    repo.run(Operation::Discard {
        path: ":(glob)*".into(),
        snapshot: diff.snapshot,
    });
    assert!(!repo.0.join(":(glob)*").exists());
}

#[test]
fn stale_discard_and_workspace_escape_leave_files_unchanged() {
    let repo = Repo::new();
    repo.initial();
    repo.write("a.txt", b"changed\n");
    let diff = repo.diff("a.txt", DiffSide::Unstaged);
    repo.write("a.txt", b"newer\n");
    assert_eq!(
        execute(
            &repo.0,
            &Operation::Discard {
                path: "a.txt".into(),
                snapshot: diff.snapshot
            }
        )
        .unwrap_err()
        .kind,
        ErrorKind::Stale
    );
    assert_eq!(fs::read(repo.0.join("a.txt")).unwrap(), b"newer\n");
    assert_eq!(
        execute(
            &repo.0,
            &Operation::Diff {
                path: "../outside".into(),
                side: DiffSide::Unstaged
            }
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidInput
    );
    fs::create_dir(repo.0.join("subdir")).unwrap();
    let Output::Status(status) = execute(&repo.0.join("subdir"), &Operation::Status).unwrap()
    else {
        panic!()
    };
    assert!(!status.writable);
    assert!(status.entries.is_empty());
    assert_eq!(
        execute(
            &repo.0.join("subdir"),
            &Operation::Commit {
                message: "Hidden scope".into()
            }
        )
        .unwrap_err()
        .kind,
        ErrorKind::PermissionDenied
    );
}

#[test]
fn binary_and_symlink_diffs_use_whole_file_actions() {
    let repo = Repo::new();
    repo.write("bin", b"a\0b");
    repo.run(Operation::Stage { paths: vec![] });
    repo.run(Operation::Commit {
        message: "Binary".into(),
    });
    repo.write("bin", b"a\0c");
    let diff = repo.diff("bin", DiffSide::Unstaged);
    assert!(diff.binary);
    assert!(!diff.can_apply_hunks);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/outside/secret", repo.0.join("link")).unwrap();
        let diff = repo.diff("link", DiffSide::Unstaged);
        assert_eq!(diff.new_bytes, b"/outside/secret");
        assert!(diff.binary);
        assert!(!diff.can_apply_hunks);
        repo.run(Operation::Stage {
            paths: vec!["link".into()],
        });
    }
}

fn conflicted(repo: &Repo, rebase: bool) {
    repo.initial();
    repo.git(&["switch", "-c", "incoming"]);
    repo.write("a.txt", b"incoming\n");
    repo.git(&["commit", "-am", "Incoming"]);
    repo.git(&["switch", "main"]);
    repo.write("a.txt", b"current\n");
    repo.git(&["commit", "-am", "Current"]);
    assert!(
        !repo
            .command(if rebase {
                &["rebase", "incoming"]
            } else {
                &["merge", "incoming"]
            })
            .status
            .success()
    );
}

#[test]
fn merge_conflicts_require_explicit_marker_free_resolution() {
    let repo = Repo::new();
    conflicted(&repo, false);
    let status = repo.status();
    assert_eq!(status.operation, RepositoryOperation::Merge);
    assert!(status.entries[0].conflicted);
    assert!(execute(&repo.0, &Operation::Stage { paths: vec![] }).is_err());
    assert!(
        execute(
            &repo.0,
            &Operation::Stage {
                paths: vec!["a.txt".into()]
            }
        )
        .is_err()
    );
    assert!(
        execute(
            &repo.0,
            &Operation::Commit {
                message: "Premature".into()
            }
        )
        .is_err()
    );
    repo.run(Operation::Resolve {
        path: "a.txt".into(),
        choice: ConflictChoice::Current,
    });
    assert_eq!(fs::read(repo.0.join("a.txt")).unwrap(), b"current\n");
    assert!(repo.status().entries[0].conflicted);
    repo.run(Operation::Stage {
        paths: vec!["a.txt".into()],
    });
    repo.run(Operation::Commit {
        message: "Resolve merge".into(),
    });
    assert_eq!(repo.status().operation, RepositoryOperation::None);
}

#[test]
fn rebase_continue_and_abort_follow_repository_state() {
    let repo = Repo::new();
    conflicted(&repo, true);
    assert_eq!(repo.status().operation, RepositoryOperation::Rebase);
    repo.run(Operation::Abort);
    assert_eq!(repo.status().operation, RepositoryOperation::None);
    assert!(!repo.command(&["rebase", "incoming"]).status.success());
    repo.write("a.txt", b"combined\n");
    repo.run(Operation::Stage {
        paths: vec!["a.txt".into()],
    });
    repo.run(Operation::Continue);
    assert_eq!(repo.status().operation, RepositoryOperation::None);
    assert_eq!(repo.status().branch, "main");
}

#[test]
fn canceling_stage_removes_its_index_lock_and_allows_the_next_stage() {
    let repo = Repo::new();
    repo.initial();
    repo.write(".gitattributes", b"a.txt filter=slow\n");
    repo.git(&[
        "config",
        "filter.slow.clean",
        "touch .git/filter-ready; sleep 30; cat",
    ]);
    repo.write("a.txt", b"next\n");
    let mut job = Job::start(
        repo.0.clone(),
        Operation::Stage {
            paths: vec!["a.txt".into()],
        },
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !repo.0.join(".git/filter-ready").exists() {
        assert!(Instant::now() < deadline, "clean filter did not start");
        thread::sleep(Duration::from_millis(5));
    }
    let lock = repo.0.join(".git/index.lock");
    assert!(lock.exists(), "stage must be holding its index lock");
    job.cancel();
    let result = loop {
        if let Some(result) = job.try_result() {
            break result;
        }
        assert!(Instant::now() < deadline, "cancellation did not finish");
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(result.unwrap_err().kind, ErrorKind::Canceled);
    assert!(!lock.exists(), "canceled Git must clean up its index lock");
    repo.git(&["config", "filter.slow.clean", "cat"]);
    repo.run(Operation::Stage {
        paths: vec!["a.txt".into()],
    });
    assert_eq!(repo.status().entries[0].index_status, 'M');
}

#[test]
fn canceled_hook_job_finishes_and_releases_repository_lock() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.initial();
    repo.write("a.txt", b"next\n");
    repo.run(Operation::Stage { paths: vec![] });
    let hook = repo.0.join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        b"#!/bin/sh\ntouch .git/hook-ready\nsleep 30 &\nwait\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let mut job = Job::start(
        repo.0.clone(),
        Operation::Commit {
            message: "Canceled".into(),
        },
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !repo.0.join(".git/hook-ready").exists() {
        assert!(Instant::now() < deadline, "hook did not start");
        thread::sleep(Duration::from_millis(10));
    }
    job.cancel();
    let result = loop {
        if let Some(result) = job.try_result() {
            break result;
        }
        assert!(Instant::now() < deadline, "cancellation did not finish");
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(result.unwrap_err().kind, ErrorKind::Canceled);
    fs::remove_file(hook).unwrap();
    repo.run(Operation::Commit {
        message: "After cancellation".into(),
    });
    assert!(repo.status().entries.is_empty());
}

#[test]
fn branches_reject_option_injection_and_preserve_working_edits() {
    let repo = Repo::new();
    repo.initial();
    repo.run(Operation::CreateBranch {
        name: "feature".into(),
    });
    assert_eq!(repo.status().branch, "feature");
    repo.run(Operation::SwitchBranch {
        name: "main".into(),
    });
    assert_eq!(repo.status().branch, "main");
    assert!(
        execute(
            &repo.0,
            &Operation::SwitchBranch {
                name: "--discard-changes".into()
            }
        )
        .is_err()
    );
    repo.write("a.txt", b"local edit\n");
    repo.run(Operation::SwitchBranch {
        name: "feature".into(),
    });
    assert_eq!(fs::read(repo.0.join("a.txt")).unwrap(), b"local edit\n");
}

#[test]
fn configured_fetch_pull_push_work_against_a_local_remote() {
    let repo = Repo::new();
    repo.initial();
    // Keep the bare remote and second checkout outside the worktree so status
    // and stage-all exercise the same scope as a normal configured remote.
    let remote = Repo::new();
    remote.git(&["config", "core.bare", "true"]);
    repo.git(&["remote", "add", "origin", remote.0.to_str().unwrap()]);
    repo.git(&["push", "--set-upstream", "origin", "main"]);
    repo.write("a.txt", b"published\n");
    repo.run(Operation::Stage { paths: vec![] });
    repo.run(Operation::Commit {
        message: "Publish".into(),
    });
    repo.run(Operation::Push);
    let peer = Repo::new();
    peer.git(&["remote", "add", "origin", remote.0.to_str().unwrap()]);
    peer.git(&["fetch"]);
    peer.git(&["reset", "--hard", "origin/main"]);
    peer.write("a.txt", b"remote change\n");
    peer.git(&["commit", "-am", "Remote change"]);
    peer.git(&["push", "origin", "HEAD:main"]);
    repo.run(Operation::Fetch);
    assert_eq!(repo.status().behind, 1);
    repo.run(Operation::Pull);
    assert_eq!(repo.status().behind, 0);
    assert_eq!(fs::read(repo.0.join("a.txt")).unwrap(), b"remote change\n");
    repo.run(Operation::CreateBranch {
        name: "publish-new-branch".into(),
    });
    assert!(repo.status().upstream.is_none());
    repo.run(Operation::Push);
    assert_eq!(
        repo.status().upstream.as_deref(),
        Some("origin/publish-new-branch")
    );
}

#[test]
fn linked_worktrees_share_repository_lock_and_remain_writable() {
    let repo = Repo::new();
    repo.initial();
    let worktree = repo.0.with_extension("worktree");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "linked",
        worktree.to_str().unwrap(),
    ]);
    let Output::Status(status) = execute(&worktree, &Operation::Status).unwrap() else {
        panic!()
    };
    assert!(status.writable);
    assert_eq!(status.branch, "linked");
    assert_eq!(
        bed_git::repository_key(&repo.0).unwrap(),
        bed_git::repository_key(&worktree).unwrap()
    );
    fs::write(worktree.join("a.txt"), b"linked edit\n").unwrap();
    execute(
        &worktree,
        &Operation::Stage {
            paths: vec!["a.txt".into()],
        },
    )
    .unwrap();
    execute(
        &worktree,
        &Operation::Commit {
            message: "Linked worktree".into(),
        },
    )
    .unwrap();
    repo.git(&["worktree", "remove", worktree.to_str().unwrap()]);
}

#[test]
fn cancellation_remains_effective_after_git_exits_with_a_signal_ignoring_hook() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.initial();
    repo.write("a.txt", b"next\n");
    repo.run(Operation::Stage { paths: vec![] });
    let hook = repo.0.join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        b"#!/bin/sh\n(trap '' TERM; exec sleep 30) &\ntouch .git/background-ready\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let mut job = Job::start(
        repo.0.clone(),
        Operation::Commit {
            message: "Background hook".into(),
        },
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !repo.0.join(".git/background-ready").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    // Git can exit while the hook's child retains our capture descriptors and
    // ignores graceful termination. Cancellation must still finish promptly.
    thread::sleep(Duration::from_millis(50));
    job.cancel();
    loop {
        if let Some(result) = job.try_result() {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Canceled);
            break;
        }
        assert!(Instant::now() < deadline, "pipe drain ignored cancellation");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn displayed_ranges_group_git_hunks_and_preserve_unselected_offsets() {
    let repo = Repo::new();
    repo.initial();
    repo.write(
        "a.txt",
        b"prefix\none\ntwo\nTHREE\nfour\nFIVE\nsix\nseven\neight\n",
    );
    let diff = repo.diff("a.txt", DiffSide::Unstaged);
    assert_eq!(diff.hunks.len(), 3);
    // One displayed block may group two Git hunks and their unchanged middle
    // row. The earlier insertion stays unstaged and must not shift this patch.
    repo.run(Operation::StageRange {
        path: "a.txt".into(),
        snapshot: diff.snapshot,
        old_range: 2..5,
        new_range: 3..6,
    });
    let staged = repo.diff("a.txt", DiffSide::Staged);
    assert_eq!(
        staged.new_bytes,
        b"one\ntwo\nTHREE\nfour\nFIVE\nsix\nseven\neight\n"
    );
    repo.run(Operation::UnstageRange {
        path: "a.txt".into(),
        snapshot: staged.snapshot,
        old_range: 2..5,
        new_range: 2..5,
    });
    assert_eq!(repo.status().entries[0].index_status, ' ');
    let diff = repo.diff("a.txt", DiffSide::Unstaged);
    repo.run(Operation::DiscardRange {
        path: "a.txt".into(),
        snapshot: diff.snapshot,
        old_range: 2..5,
        new_range: 3..6,
    });
    assert_eq!(
        fs::read(repo.0.join("a.txt")).unwrap(),
        b"prefix\none\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n"
    );
}

#[test]
fn range_insert_delete_no_final_newline_and_stale_validation() {
    let repo = Repo::new();
    repo.initial();
    repo.write(
        "a.txt",
        b"prefix\none\ntwo\nthree\nfour\nfive\nsix\nseven\nEIGHT",
    );
    let diff = repo.diff("a.txt", DiffSide::Unstaged);
    repo.run(Operation::StageRange {
        path: "a.txt".into(),
        snapshot: diff.snapshot,
        old_range: 7..8,
        new_range: 8..9,
    });
    let staged = repo.diff("a.txt", DiffSide::Staged);
    assert!(staged.new_bytes.ends_with(b"EIGHT"));
    repo.run(Operation::UnstageRange {
        path: "a.txt".into(),
        snapshot: staged.snapshot,
        old_range: 7..8,
        new_range: 7..8,
    });
    let diff = repo.diff("a.txt", DiffSide::Unstaged);
    repo.run(Operation::StageRange {
        path: "a.txt".into(),
        snapshot: diff.snapshot,
        old_range: 0..0,
        new_range: 0..1,
    });
    let staged = repo.diff("a.txt", DiffSide::Staged);
    assert!(staged.new_bytes.starts_with(b"prefix\none\n"));
    repo.run(Operation::UnstageRange {
        path: "a.txt".into(),
        snapshot: staged.snapshot,
        old_range: 0..0,
        new_range: 0..1,
    });
    let diff = repo.diff("a.txt", DiffSide::Unstaged);
    repo.write("a.txt", b"external edit\n");
    let err = execute(
        &repo.0,
        &Operation::StageRange {
            path: "a.txt".into(),
            snapshot: diff.snapshot,
            old_range: 0..0,
            new_range: 0..1,
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Stale);
    let diff = repo.diff("a.txt", DiffSide::Unstaged);
    assert_eq!(
        execute(
            &repo.0,
            &Operation::StageRange {
                path: "a.txt".into(),
                snapshot: diff.snapshot,
                old_range: 100..101,
                new_range: 0..1
            }
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidInput
    );
}

#[test]
fn range_staging_respects_clean_crlf_and_discard_preserves_worktree_crlf() {
    let repo = Repo::new();
    repo.git(&["config", "core.autocrlf", "true"]);
    repo.write("a.txt", b"one\r\ntwo\r\nthree\r\nfour\r\n");
    repo.run(Operation::Stage { paths: vec![] });
    repo.run(Operation::Commit {
        message: "CRLF".into(),
    });
    repo.write("a.txt", b"ONE\r\ntwo\r\nTHREE\r\nfour\r\n");
    let diff = repo.diff("a.txt", DiffSide::Unstaged);
    assert_eq!(diff.old_bytes, b"one\ntwo\nthree\nfour\n");
    repo.run(Operation::StageRange {
        path: "a.txt".into(),
        snapshot: diff.snapshot,
        old_range: 0..1,
        new_range: 0..1,
    });
    assert_eq!(
        repo.diff("a.txt", DiffSide::Staged).new_bytes,
        b"ONE\ntwo\nthree\nfour\n"
    );
    let diff = repo.diff("a.txt", DiffSide::Unstaged);
    repo.run(Operation::DiscardRange {
        path: "a.txt".into(),
        snapshot: diff.snapshot,
        old_range: 2..3,
        new_range: 2..3,
    });
    assert_eq!(
        fs::read(repo.0.join("a.txt")).unwrap(),
        b"ONE\r\ntwo\r\nthree\r\nfour\r\n"
    );
}

#[test]
fn deleted_symlinks_and_gitlink_directories_use_nontext_summary() {
    use std::os::unix::fs::symlink;
    let repo = Repo::new();
    repo.initial();
    symlink("a.txt", repo.0.join("link")).unwrap();
    repo.run(Operation::Stage {
        paths: vec!["link".into()],
    });
    repo.run(Operation::Commit {
        message: "Link".into(),
    });
    repo.git(&["rm", "link"]);
    let diff = repo.diff("link", DiffSide::Staged);
    assert!(diff.binary);
    assert!(!diff.can_apply_hunks);
    assert_eq!(diff.old_bytes, b"a.txt");
    // A gitlink can be represented without network/submodule initialization.
    let commit = repo.status().head.unwrap();
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{commit},submodule"),
    ]);
    fs::create_dir(repo.0.join("submodule")).unwrap();
    let diff = repo.diff("submodule", DiffSide::Staged);
    assert!(diff.binary);
    assert!(diff.new_bytes.starts_with(b"Subproject commit "));
}

#[test]
fn literal_bom_range_deletion_ignores_diff_prefix_configuration() {
    let repo = Repo::new();
    repo.git(&["config", "diff.noprefix", "true"]);
    let path = "- [:literal]\nfile";
    repo.write(path, b"\xef\xbb\xbfone\ntwo\nthree\nfour\n");
    repo.run(Operation::Stage {
        paths: vec![path.into()],
    });
    repo.run(Operation::Commit {
        message: "BOM literal".into(),
    });
    repo.write(path, b"\xef\xbb\xbfprefix\none\ntwo\nfour\n");
    let diff = repo.diff(path, DiffSide::Unstaged);
    repo.run(Operation::StageRange {
        path: path.into(),
        snapshot: diff.snapshot,
        old_range: 2..3,
        new_range: 3..3,
    });
    let staged = repo.diff(path, DiffSide::Staged);
    assert_eq!(staged.new_bytes, b"\xef\xbb\xbfone\ntwo\nfour\n");
    repo.run(Operation::UnstageRange {
        path: path.into(),
        snapshot: staged.snapshot,
        old_range: 2..3,
        new_range: 2..2,
    });
    let diff = repo.diff(path, DiffSide::Unstaged);
    repo.run(Operation::DiscardRange {
        path: path.into(),
        snapshot: diff.snapshot,
        old_range: 2..3,
        new_range: 3..3,
    });
    assert_eq!(
        fs::read(repo.0.join(path)).unwrap(),
        b"\xef\xbb\xbfprefix\none\ntwo\nthree\nfour\n"
    );
}
