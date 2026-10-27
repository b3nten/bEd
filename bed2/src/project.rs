//! Application project admission. The OS releases the claim on process exit,
//! including crashes; the lock file itself is deliberately never unlinked.
use bevy::prelude::*;
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

#[derive(Resource)]
pub struct ProjectLock {
    _file: File,
}

impl ProjectLock {
    pub fn acquire(root: &Path) -> Result<Self, String> {
        // Project exclusivity is independent of XDG_CONFIG_HOME: choosing a
        // different preferences directory must not admit a second editor.
        #[cfg(unix)]
        let directory =
            std::env::temp_dir().join(format!("bed2-project-locks-{}", unsafe { libc::geteuid() }));
        #[cfg(not(unix))]
        let directory = std::env::temp_dir().join("bed2-project-locks");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let digest = blake3::hash(root.as_os_str().as_encoded_bytes());
        let path = directory.join(format!("{digest}.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| e.to_string())?;
        file.try_lock().map_err(|e| format!("Project {} is already open or cannot be locked: {e}. Multiple independent sessions for the same project are unsupported.", root.display()))?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    
    fn fixture() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bed2-admission-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    
    #[test]
    fn child_admission_probe() {
        let Some(root) = std::env::var_os("BED_PROJECT_LOCK_TEST_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        match std::env::var("BED_PROJECT_LOCK_TEST_MODE")
            .unwrap()
            .as_str()
        {
            "race" => {
                let id = std::env::var("BED_PROJECT_LOCK_TEST_PARTICIPANT").unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while !root.join("start").exists() {
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                let claim = ProjectLock::acquire(&root);
                std::fs::write(
                    root.join(format!("result-{id}")),
                    if claim.is_ok() {
                        "admitted"
                    } else {
                        "rejected"
                    },
                )
                .unwrap();
                if claim.is_ok() {
                    while !root.join("release").exists() {
                        assert!(std::time::Instant::now() < deadline);
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
            }
            "blocked" => assert!(ProjectLock::acquire(&root).is_err()),
            "crash" => {
                let _claim = ProjectLock::acquire(&root).unwrap();
                std::process::exit(0);
            }
            _ => {
                let _claim = ProjectLock::acquire(&root).unwrap();
            }
        }
    }
    
    fn child(root: &Path, mode: &str) {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "project::tests::child_admission_probe",
                "--nocapture",
            ])
            .env("XDG_CONFIG_HOME", root.join("different-config"))
            .env("BED_PROJECT_LOCK_TEST_ROOT", root)
            .env("BED_PROJECT_LOCK_TEST_MODE", mode)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    
    #[test]
    fn exclusive_project_admission_covers_processes_and_releases_after_exit_or_crash() {
        let root = fixture();
        let claim = ProjectLock::acquire(&root).unwrap();
        assert!(ProjectLock::acquire(&root).is_err());
        child(&root, "blocked");
        drop(claim);
        child(&root, "allowed");
        child(&root, "crash");
        let claim = ProjectLock::acquire(&root).unwrap();
        drop(claim);
        std::fs::remove_dir_all(root).unwrap();
    }
    
    #[test]
    fn simultaneous_project_launches_admit_exactly_one_writer() {
        let root = fixture();
        let mut children: Vec<_> = (0..2)
            .map(|id| {
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "project::tests::child_admission_probe"])
                    .env("BED_PROJECT_LOCK_TEST_ROOT", &root)
                    .env("BED_PROJECT_LOCK_TEST_MODE", "race")
                    .env("BED_PROJECT_LOCK_TEST_PARTICIPANT", id.to_string())
                    .spawn()
                    .unwrap()
            })
            .collect();
        std::fs::write(root.join("start"), b"start").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !(0..2).all(|id| {
            std::fs::read_to_string(root.join(format!("result-{id}"))).is_ok_and(|s| !s.is_empty())
        }) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let results: Vec<_> = (0..2)
            .map(|id| std::fs::read_to_string(root.join(format!("result-{id}"))).unwrap())
            .collect();
        std::fs::write(root.join("release"), b"release").unwrap();
        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        assert_eq!(
            results.iter().filter(|r| r.as_str() == "admitted").count(),
            1
        );
        assert!(ProjectLock::acquire(&root).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
    
    #[cfg(unix)]
    #[test]
    fn canonical_aliases_share_project_admission() {
        let root = fixture();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&project, &alias).unwrap();
        let claim = ProjectLock::acquire(&std::fs::canonicalize(&project).unwrap()).unwrap();
        assert!(ProjectLock::acquire(&std::fs::canonicalize(alias).unwrap()).is_err());
        drop(claim);
        std::fs::remove_dir_all(root).unwrap();
    }
}
