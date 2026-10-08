use crate::{
    PROTOCOL_VERSION, Request, RequestFrame, Response, ResponseFrame, read_frame, write_frame,
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    io,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SshTarget {
    pub host: String,
    pub agent: String,
}

impl SshTarget {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            agent: "bed-headless".into(),
        }
    }
}

impl Default for SshTarget {
    fn default() -> Self {
        Self::new(String::new())
    }
}

/// Quote one literal argument for the POSIX shell invoked by OpenSSH.
pub fn shell_quote(value: &str) -> io::Result<String> {
    if value.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "remote command argument contains NUL",
        ));
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

pub fn remote_command(arguments: &[String]) -> io::Result<String> {
    arguments
        .iter()
        .map(|argument| shell_quote(argument))
        .collect::<io::Result<Vec<_>>>()
        .map(|args| args.join(" "))
}

struct Job {
    request: Request,
    reply: mpsc::SyncSender<io::Result<Response>>,
}

struct Connection {
    jobs: mpsc::Sender<Job>,
    child: Mutex<Child>,
    connected: Arc<AtomicBool>,
}

impl Connection {
    fn stop(&self) {
        self.connected.store(false, Ordering::Release);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Cloneable synchronous handle. All pipe I/O runs on a dedicated worker;
/// consumers should submit blocking calls from their own service workers.
#[derive(Clone)]
pub struct RemoteClient {
    connection: Arc<Connection>,
}

impl std::fmt::Debug for RemoteClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteClient")
            .field("connected", &self.is_connected())
            .finish()
    }
}

impl RemoteClient {
    /// Whether two handles share the same live transport and capability scope.
    pub fn same_connection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.connection, &other.connection)
    }
    pub fn launch_ssh(target: &SshTarget) -> io::Result<Self> {
        if target.host.is_empty()
            || target.host.starts_with('-')
            || target.host.chars().any(char::is_whitespace)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid SSH host or configuration alias",
            ));
        }
        let mut command = Command::new("ssh");
        command.args(["-T", "--", &target.host]);
        command.arg(remote_command(&[target.agent.clone(), "--stdio".into()])?);
        Self::launch_command(command)
    }

    pub fn launch_local(program: impl AsRef<OsStr>) -> io::Result<Self> {
        let mut command = Command::new(program);
        command.arg("--stdio");
        Self::launch_command(command)
    }

    pub fn launch_command(mut command: Command) -> io::Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("missing agent stdin"))?;
        let mut output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing agent stdout"))?;
        let (jobs, receiver) = mpsc::channel::<Job>();
        let connected = Arc::new(AtomicBool::new(true));
        let worker_connected = connected.clone();
        thread::Builder::new()
            .name("bed-remote-transport".into())
            .spawn(move || {
                let mut next_id = 1_u64;
                for job in receiver {
                    if !worker_connected.load(Ordering::Acquire) {
                        let _ = job.reply.send(Err(disconnected()));
                        continue;
                    }
                    let id = next_id;
                    next_id = next_id.wrapping_add(1);
                    let result = (|| {
                        write_frame(
                            &mut input,
                            &RequestFrame {
                                id,
                                request: job.request,
                            },
                        )?;
                        let response: ResponseFrame =
                            read_frame(&mut output)?.ok_or_else(disconnected)?;
                        if response.id != id {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "remote response request ID mismatch",
                            ));
                        }
                        Ok(response.response)
                    })();
                    let response = match result {
                        Ok(response) => response.map_err(|error| error.into_io()),
                        Err(error) => {
                            worker_connected.store(false, Ordering::Release);
                            Err(error)
                        }
                    };
                    let _ = job.reply.send(response);
                }
            })?;
        let client = Self {
            connection: Arc::new(Connection {
                jobs,
                child: Mutex::new(child),
                connected,
            }),
        };
        match client.call(Request::Hello {
            version: PROTOCOL_VERSION,
        })? {
            Response::Hello {
                version: PROTOCOL_VERSION,
            } => Ok(client),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported remote protocol handshake",
            )),
        }
    }

    pub fn call(&self, request: Request) -> io::Result<Response> {
        if !self.is_connected() {
            return Err(disconnected());
        }
        let (reply, response) = mpsc::sync_channel(1);
        self.connection
            .jobs
            .send(Job { request, reply })
            .map_err(|_| disconnected())?;
        match response.recv_timeout(Duration::from_secs(60)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.connection.stop();
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "remote service request timed out",
                ))
            }
            Err(_) => Err(disconnected()),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connection.connected.load(Ordering::Acquire)
    }

    pub fn disconnect(&self) {
        self.connection.stop();
    }
}

fn disconnected() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "remote workspace disconnected")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_preserves_shell_metacharacters() {
        assert_eq!(
            shell_quote("a'b; $(echo bad)").unwrap(),
            "'a'\\''b; $(echo bad)'"
        );
        assert!(shell_quote("a\0b").is_err());
        assert_eq!(
            remote_command(&["bed-headless".into(), "".into()]).unwrap(),
            "'bed-headless' ''"
        );
    }
}
