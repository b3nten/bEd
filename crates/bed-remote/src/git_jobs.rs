//! Connection-owned Git jobs. All control requests return without waiting on Git.
use crate::{ErrorKind, RemoteError, Request, Response};
use std::{collections::HashMap, fs};

const MAX_JOBS: usize = 8;
#[derive(Default)]
pub(crate) struct GitJobs {
    next: u64,
    jobs: HashMap<u64, Entry>,
}
struct Entry {
    job: bed_git::Job,
    result: Option<bed_git::Result<bed_git::Output>>,
}
impl GitJobs {
    pub fn handles(request: &Request) -> bool {
        matches!(
            request,
            Request::GitStart { .. }
                | Request::GitPoll { .. }
                | Request::GitCancel { .. }
                | Request::GitRelease { .. }
        )
    }
    pub fn call(&mut self, request: Request) -> Result<Response, RemoteError> {
        match request {
            Request::GitStart { root, operation } => {
                if self.jobs.len() >= MAX_JOBS {
                    return Err(RemoteError::new(
                        ErrorKind::Conflict,
                        "Git job limit reached; release completed jobs",
                    ));
                }
                let root = fs::canonicalize(root)?;
                if !root.is_dir() {
                    return Err(RemoteError::new(
                        ErrorKind::InvalidInput,
                        "Git workspace root is not a directory",
                    ));
                }
                self.next = self.next.checked_add(1).ok_or_else(|| {
                    RemoteError::new(ErrorKind::Other, "Git job identifiers exhausted")
                })?;
                let job_id = self.next;
                self.jobs.insert(
                    job_id,
                    Entry {
                        job: bed_git::Job::start(root, operation),
                        result: None,
                    },
                );
                Ok(Response::GitStarted { job_id })
            }
            Request::GitPoll { job_id } => {
                let entry = self.jobs.get_mut(&job_id).ok_or_else(expired)?;
                if entry.result.is_none() {
                    entry.result = entry.job.try_result();
                }
                Ok(Response::GitJob {
                    result: entry.result.clone(),
                })
            }
            Request::GitCancel { job_id } => {
                self.jobs.get(&job_id).ok_or_else(expired)?.job.cancel();
                Ok(Response::Unit)
            }
            Request::GitRelease { job_id } => {
                self.jobs.remove(&job_id);
                Ok(Response::Unit)
            }
            _ => Err(RemoteError::new(
                ErrorKind::InvalidInput,
                "Not a Git job request",
            )),
        }
    }
}
impl Drop for GitJobs {
    fn drop(&mut self) {
        for entry in self.jobs.values() {
            entry.job.cancel();
        }
    }
}
fn expired() -> RemoteError {
    RemoteError::new(
        ErrorKind::NotFound,
        "Git job expired; refresh repository state before retrying",
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{PROTOCOL_VERSION, RequestFrame, ResponseFrame, read_frame, write_frame};
    use std::{
        fs,
        os::unix::{fs::PermissionsExt, net::UnixStream},
        path::PathBuf,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
        thread,
        time::{Duration, Instant},
    };

    struct Repo(PathBuf);
    impl Repo {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "bed-remote-git-{}-{}",
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
            fs::write(repo.0.join("a.txt"), b"initial\n").unwrap();
            repo.git(&["add", "a.txt"]);
            repo.git(&["commit", "-m", "Initial"]);
            repo
        }
        fn git(&self, args: &[&str]) {
            let result = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        fn root(&self) -> String {
            self.0.to_str().unwrap().into()
        }
        fn hook(&self) {
            fs::write(self.0.join("a.txt"), b"new\n").unwrap();
            self.git(&["add", "a.txt"]);
            let path = self.0.join(".git/hooks/pre-commit");
            fs::write(&path, b"#!/bin/sh\ntouch .git/ready\nsleep 30 &\nwait\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fn ready(&self) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !self.0.join(".git/ready").exists() {
                assert!(Instant::now() < deadline, "hook never started");
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn call(stream: &mut UnixStream, request: Request) -> Result<Response, RemoteError> {
        write_frame(stream, &RequestFrame { id: 1, request }).unwrap();
        let response: ResponseFrame = read_frame(stream).unwrap().unwrap();
        assert_eq!(response.id, 1);
        response.response
    }
    fn connect() -> (UnixStream, thread::JoinHandle<()>) {
        let (mut client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let worker =
            thread::spawn(move || crate::serve(server.try_clone().unwrap(), server).unwrap());
        assert_eq!(
            call(
                &mut client,
                Request::Hello {
                    version: PROTOCOL_VERSION
                }
            )
            .unwrap(),
            Response::Hello {
                version: PROTOCOL_VERSION
            }
        );
        (client, worker)
    }
    fn start(stream: &mut UnixStream, root: String, operation: bed_git::Operation) -> u64 {
        let Response::GitStarted { job_id } =
            call(stream, Request::GitStart { root, operation }).unwrap()
        else {
            panic!()
        };
        job_id
    }

    #[test]
    fn running_hook_does_not_block_file_io_and_can_be_canceled() {
        let repo = Repo::new();
        repo.hook();
        let (mut client, server) = connect();
        let job_id = start(
            &mut client,
            repo.root(),
            bed_git::Operation::Commit {
                message: "Slow commit".into(),
            },
        );
        repo.ready();
        let started = Instant::now();
        let result = call(
            &mut client,
            Request::ReadFile {
                root: repo.root(),
                path: "a.txt".into(),
            },
        )
        .unwrap();
        assert!(matches!(result, Response::File { .. }));
        assert!(started.elapsed() < Duration::from_secs(1));
        call(
            &mut client,
            Request::WriteFile {
                root: repo.root(),
                path: "other.txt".into(),
                bytes: b"save while hook runs\n".to_vec(),
                baseline: None,
            },
        )
        .unwrap();
        call(&mut client, Request::GitCancel { job_id }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let completion = loop {
            let Response::GitJob { result } =
                call(&mut client, Request::GitPoll { job_id }).unwrap()
            else {
                panic!()
            };
            if let Some(result) = result {
                break result;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(completion.unwrap_err().kind, bed_git::ErrorKind::Canceled);
        assert!(matches!(
            call(&mut client, Request::GitPoll { job_id }).unwrap(),
            Response::GitJob {
                result: Some(Err(_))
            }
        ));
        call(&mut client, Request::GitRelease { job_id }).unwrap();
        assert_eq!(
            call(&mut client, Request::GitPoll { job_id })
                .unwrap_err()
                .kind,
            ErrorKind::NotFound
        );
        drop(client);
        server.join().unwrap();
        assert_eq!(
            fs::read(repo.0.join("other.txt")).unwrap(),
            b"save while hook runs\n"
        );
    }

    #[test]
    fn connection_eof_cancels_hook_and_job_capacity_is_bounded() {
        let repo = Repo::new();
        repo.hook();
        let (mut client, server) = connect();
        start(
            &mut client,
            repo.root(),
            bed_git::Operation::Commit {
                message: "Disconnected".into(),
            },
        );
        repo.ready();
        let started = Instant::now();
        drop(client);
        server.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        let log = Command::new("git")
            .arg("-C")
            .arg(&repo.0)
            .args(["log", "-1", "--format=%s"])
            .output()
            .unwrap();
        assert_eq!(log.stdout, b"Initial\n");
        let (mut client, server) = connect();
        let mut ids = Vec::new();
        for _ in 0..MAX_JOBS {
            ids.push(start(&mut client, repo.root(), bed_git::Operation::Status));
        }
        assert_eq!(
            call(
                &mut client,
                Request::GitStart {
                    root: repo.root(),
                    operation: bed_git::Operation::Status
                }
            )
            .unwrap_err()
            .kind,
            ErrorKind::Conflict
        );
        for job_id in ids {
            call(&mut client, Request::GitRelease { job_id }).unwrap();
        }
        start(&mut client, repo.root(), bed_git::Operation::Status);
        drop(client);
        server.join().unwrap();
    }
}
