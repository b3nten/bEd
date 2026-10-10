use bed_remote::{FileInfo, LocalBackend, RemoteClient, Request, Response};
use dear_imgui_rs::{StyleVar, Ui};
use std::{
    collections::{HashMap, HashSet},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

struct Job {
    generation: u64,
    path: String,
    root: String,
    remote: Option<RemoteClient>,
}
struct Completed {
    generation: u64,
    path: String,
    result: Result<FileInfo, String>,
}
struct Cached {
    updated: Instant,
    result: Result<FileInfo, String>,
}
struct Worker {
    sender: mpsc::SyncSender<Job>,
    receiver: mpsc::Receiver<Completed>,
}
impl Worker {
    fn new() -> Self {
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, receiver) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                let request = Request::FileInfo {
                    root: job.root,
                    path: job.path.clone(),
                };
                let response = match job.remote {
                    Some(client) => client.call(request),
                    None => LocalBackend.call(request).map_err(|e| e.into_io()),
                };
                let result = response.map_err(|e| e.to_string()).and_then(|r| match r {
                    Response::FileInfo { info } => Ok(info),
                    _ => Err("Unexpected file information response".into()),
                });
                if results
                    .send(Completed {
                        generation: job.generation,
                        path: job.path,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self { sender, receiver }
    }
}
/// Short-lived bounded cache. All filesystem/Git/SSH work runs on a worker.
#[derive(Default)]
pub struct FileInfoHover {
    scope: String,
    root: String,
    remote: Option<RemoteClient>,
    generation: u64,
    worker: Option<Worker>,
    cache: HashMap<String, Cached>,
    pending: HashSet<String>,
}
impl FileInfoHover {
    pub fn configure(&mut self, scope: String, root: &str, remote: Option<RemoteClient>) {
        if self.scope != scope || self.root != root || self.remote.is_some() != remote.is_some() {
            self.scope = scope;
            self.root = root.into();
            self.generation = self.generation.wrapping_add(1);
            self.cache.clear();
            self.pending.clear();
        }
        self.remote = remote;
    }
    fn poll(&mut self) {
        let Some(worker) = &self.worker else {
            return;
        };
        while let Ok(result) = worker.receiver.try_recv() {
            if result.generation != self.generation {
                continue;
            }
            self.pending.remove(&result.path);
            if self.cache.len() >= 128
                && let Some(oldest) = self
                    .cache
                    .iter()
                    .min_by_key(|(_, v)| v.updated)
                    .map(|(k, _)| k.clone())
            {
                self.cache.remove(&oldest);
            }
            self.cache.insert(
                result.path,
                Cached {
                    updated: Instant::now(),
                    result: result.result,
                },
            );
        }
    }
    pub fn draw(&mut self, ui: &Ui, path: &str, root: &str, remote: bool) {
        // Direct tree consumers get local metadata automatically. A remote tree
        // without a configured client never probes a coincident local path.
        if self.root != root {
            self.configure(root.into(), root, None);
        }
        self.poll();
        let expired = self
            .cache
            .get(path)
            .is_none_or(|c| c.updated.elapsed() >= Duration::from_secs(5));
        if expired && !self.pending.contains(path) && (!remote || self.remote.is_some()) {
            let worker = self.worker.get_or_insert_with(Worker::new);
            let job = Job {
                generation: self.generation,
                path: path.into(),
                root: root.into(),
                remote: self.remote.clone(),
            };
            if worker.sender.try_send(job).is_ok() {
                self.pending.insert(path.into());
            }
        }
        let _style = bed_ui::util::popup_style::context_menu_style(ui);
        let padding = [12.0, 10.0];
        let wrap_width = (ui.current_font_size() * 36.0)
            .min((ui.window_viewport().work_size()[0] - padding[0] * 4.0).max(1.0));
        let _padding = ui.push_style_var(StyleVar::WindowPadding(padding));
        bed_ui::util::popup_style::tooltip(ui, || {
            // text_wrapped() resets wrapping to the current window width,
            // preventing an auto-sized tooltip from growing past a narrow one.
            let _wrap = ui.push_text_wrap_pos(ui.cursor_pos()[0] + wrap_width);
            ui.text(path);
            ui.separator();
            match self.cache.get(path).map(|c| &c.result) {
                Some(Ok(info)) => {
                    row(ui, "Type", &info.file_type);
                    if let Some(mime) = &info.mime_type {
                        row(ui, "MIME", mime);
                    }
                    if !info.is_directory {
                        row(ui, "Size", &size_label(info.size));
                    }
                    if let Some(seconds) = info.modified_unix_seconds {
                        let modified = chrono::DateTime::from_timestamp(seconds, 0)
                            .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                            .unwrap_or_else(|| seconds.to_string());
                        row(ui, "Modified", &modified);
                    }
                    if let Some(git) = &info.git {
                        row(ui, "Git", git);
                    }
                    if let Some(target) = &info.symlink_target {
                        row(ui, "Link target", target);
                    }
                    if info.readonly {
                        row(ui, "Permissions", "Read only");
                    }
                    if let Some(binary) = &info.binary {
                        row(ui, "Architecture", &binary.architecture);
                        row(ui, "Debug symbols", &binary.debug_symbols);
                    }
                    if self.pending.contains(path) {
                        ui.text_disabled("Refreshing…");
                    }
                }
                Some(Err(error)) => ui.text(format!("File information unavailable: {error}")),
                None => ui.text_disabled(if remote && self.remote.is_none() {
                    "File information unavailable while disconnected"
                } else {
                    "Loading file information…"
                }),
            }
        });
    }
}
fn row(ui: &Ui, label: &str, value: &str) {
    ui.text_disabled(format!("{label}:"));
    ui.same_line();
    ui.text(value);
}
fn size_label(bytes: u64) -> String {
    let mut value = bytes as f64;
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{value:.2} {} ({bytes} bytes)", units[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn metadata_worker_reads_locally_and_workspace_changes_discard_old_results() {
        let temp = TempDir::new();
        let file = temp.write("file.rs", b"fn main() {}\n");
        let root = temp.root().to_str().unwrap();
        let path = file.to_str().unwrap().to_owned();
        let worker = Worker::new();
        worker
            .sender
            .send(Job {
                generation: 4,
                path: path.clone(),
                root: root.into(),
                remote: None,
            })
            .unwrap();
        let result = worker
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(result.result.as_ref().unwrap().size, 13);
        assert_eq!(result.result.as_ref().unwrap().file_type, "Rust source");
        assert!(result.result.as_ref().unwrap().mime_type.is_none());
        let image = temp.write("image.dat", b"\x89PNG\r\n\x1a\n");
        worker
            .sender
            .send(Job {
                generation: 4,
                path: image.to_str().unwrap().into(),
                root: root.into(),
                remote: None,
            })
            .unwrap();
        let image = worker
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .result
            .unwrap();
        assert_eq!(image.file_type, "PNG image");
        assert_eq!(image.mime_type.as_deref(), Some("image/png"));
        let (sender, receiver) = mpsc::channel();
        let (jobs, _) = mpsc::sync_channel(1);
        let mut hover = FileInfoHover {
            scope: "first".into(),
            root: root.into(),
            generation: 4,
            worker: Some(Worker {
                sender: jobs,
                receiver,
            }),
            ..Default::default()
        };
        hover.pending.insert(path);
        sender.send(result).unwrap();
        hover.configure("second-host".into(), root, None);
        hover.poll();
        assert!(hover.cache.is_empty());
        assert!(hover.pending.is_empty());
        assert_eq!(size_label(2048), "2.00 KiB (2048 bytes)");
    }
}
