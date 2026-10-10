//! A small, local bridge from shells to companion panels and editor workspaces.
//!
//! Each connection sends one newline-terminated JSON request and receives one
//! JSON acknowledgement. The UI polls nonblocking sockets, so an incomplete or
//! unresponsive command cannot stop rendering. Paths are encoded as Unix bytes
//! rather than strings to preserve names that are not UTF-8.

use serde::{Deserialize, Serialize};
use std::fs::{self, DirBuilder, Permissions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const SOCKET_ENV: &str = "BEDTERM_SOCKET";
pub const PANE_ENV: &str = "BEDTERM_PANE";

const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_PATHS: usize = 64;
const MAX_CLIENTS: usize = 32;
const ACCEPTS_PER_POLL: usize = 16;
const CLIENT_TIMEOUT: Duration = Duration::from_secs(3);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ERROR_CHARS: usize = 1024;
const MAX_PANE_BYTES: usize = 128;
const HELPERS: &[&str] = &["+open", "+o", "+workspace", "+w", "+ducky"];
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Open {
        paths: Vec<PathBuf>,
        pane: Option<String>,
    },
    Workspace {
        root: PathBuf,
        pane: Option<String>,
    },
    Ducky {
        pane: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum WireRequest {
    Open {
        paths: Vec<Vec<u8>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<String>,
    },
    Workspace {
        root: Vec<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<String>,
    },
    Ducky {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    error: Option<String>,
}

struct PrivateDirectory(PathBuf);

impl PrivateDirectory {
    fn new() -> io::Result<Self> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for _ in 0..32 {
            let nonce = timestamp ^ u128::from(NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed));
            let path = std::env::temp_dir().join(format!("bt-{:x}-{nonce:x}", std::process::id()));
            match DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a private bedterm socket directory",
        ))
    }

    fn socket_path(&self) -> PathBuf {
        self.0.join("socket")
    }
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.socket_path());
        for helper in HELPERS {
            let _ = fs::remove_file(self.0.join(helper));
        }
        let _ = fs::remove_dir(&self.0);
    }
}

/// A single application's private command socket. Dropping it removes the socket.
pub struct Bridge {
    listener: UnixListener,
    clients: Vec<Client>,
    socket_path: PathBuf,
    // Keep this last so the sockets are closed before their directory is removed.
    _directory: PrivateDirectory,
}

impl Bridge {
    pub fn new() -> io::Result<Self> {
        let directory = PrivateDirectory::new()?;
        let executable = std::env::current_exe()?;
        for helper in HELPERS {
            symlink(&executable, directory.0.join(helper))?;
        }
        let socket_path = directory.socket_path();
        let listener = UnixListener::bind(&socket_path)?;
        fs::set_permissions(&socket_path, Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            clients: Vec::new(),
            socket_path,
            _directory: directory,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Prepend this directory to shell PATH to expose the private shell helpers.
    pub fn helper_directory(&self) -> &Path {
        &self._directory.0
    }

    /// Poll once per frame. Handler failures are returned to the requesting CLI.
    /// Malformed, disconnected, and timed-out clients are handled independently;
    /// only listener errors are returned to the application.
    pub fn poll(
        &mut self,
        mut handler: impl FnMut(Request) -> Result<(), String>,
    ) -> io::Result<()> {
        for _ in 0..ACCEPTS_PER_POLL {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if self.clients.len() < MAX_CLIENTS && stream.set_nonblocking(true).is_ok() {
                        self.clients.push(Client::new(stream));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        self.clients.retain_mut(|client| client.poll(&mut handler));
        Ok(())
    }
}

struct Client {
    stream: UnixStream,
    input: Vec<u8>,
    output: Option<Vec<u8>>,
    written: usize,
    deadline: Instant,
}

impl Client {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            input: Vec::new(),
            output: None,
            written: 0,
            deadline: Instant::now() + CLIENT_TIMEOUT,
        }
    }

    fn reply(&mut self, result: Result<(), String>) {
        let reply = Reply {
            error: result
                .err()
                .map(|error| error.chars().take(MAX_ERROR_CHARS).collect()),
        };
        // Reply contains only a bounded string and cannot fail JSON serialization.
        let mut bytes = serde_json::to_vec(&reply).expect("serialize bridge acknowledgement");
        bytes.push(b'\n');
        self.output = Some(bytes);
        self.input.clear();
    }

    /// Returns whether this connection needs another frame.
    fn poll(&mut self, handler: &mut impl FnMut(Request) -> Result<(), String>) -> bool {
        if Instant::now() >= self.deadline {
            return false;
        }

        if self.output.is_none() {
            let mut chunk = [0_u8; 4096];
            // Bound the work even when a client continuously writes or signals
            // interrupt reads. At most one bounded request is handled per client.
            for _ in 0..32 {
                match self.stream.read(&mut chunk) {
                    Ok(0) => {
                        if self.input.is_empty() {
                            return false;
                        }
                        self.reply(Err("incomplete bedterm request".into()));
                        break;
                    }
                    Ok(count) => {
                        self.input.extend_from_slice(&chunk[..count]);
                        if let Some(end) = self.input.iter().position(|&byte| byte == b'\n') {
                            let result = decode_request(&self.input[..end]).and_then(&mut *handler);
                            self.reply(result);
                            break;
                        }
                        if self.input.len() > MAX_MESSAGE_BYTES {
                            self.reply(Err("bedterm request is too large".into()));
                            break;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => return false,
                }
            }
        }

        if let Some(output) = &self.output {
            for _ in 0..8 {
                match self.stream.write(&output[self.written..]) {
                    Ok(0) => return false,
                    Ok(count) => {
                        self.written += count;
                        if self.written == output.len() {
                            return false;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => return false,
                }
            }
        }
        true
    }
}

fn decode_request(bytes: &[u8]) -> Result<Request, String> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err("bedterm request is too large".into());
    }
    match serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid bedterm request: {error}"))?
    {
        WireRequest::Open { paths, pane } => {
            validate_pane(pane.as_deref())?;
            if paths.is_empty() || paths.len() > MAX_PATHS {
                return Err(format!("open requires between 1 and {MAX_PATHS} paths"));
            }
            let paths = paths
                .into_iter()
                .map(decode_path)
                .collect::<Result<_, _>>()?;
            Ok(Request::Open { paths, pane })
        }
        WireRequest::Workspace { root, pane } => {
            validate_pane(pane.as_deref())?;
            Ok(Request::Workspace {
                root: decode_path(root)?,
                pane,
            })
        }
        WireRequest::Ducky { pane } => {
            validate_pane(pane.as_deref())?;
            Ok(Request::Ducky { pane })
        }
    }
}

fn decode_path(bytes: Vec<u8>) -> Result<PathBuf, String> {
    let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
    if !path.is_absolute() || path.as_os_str().as_bytes().contains(&0) {
        return Err("bedterm paths must be absolute and contain no NUL bytes".into());
    }
    Ok(path)
}

fn validate_pane(pane: Option<&str>) -> Result<(), String> {
    if pane.is_some_and(|pane| {
        pane.is_empty() || pane.len() > MAX_PANE_BYTES || pane.as_bytes().contains(&0)
    }) {
        return Err("invalid bedterm pane token".into());
    }
    Ok(())
}

fn resolve_paths(base: &Path, paths: Vec<PathBuf>) -> io::Result<Vec<PathBuf>> {
    if paths.is_empty() || paths.len() > MAX_PATHS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("open requires between 1 and {MAX_PATHS} paths"),
        ));
    }
    paths
        .into_iter()
        .map(|path| {
            let absolute = if path.is_absolute() {
                path
            } else {
                base.join(path)
            };
            absolute.canonicalize().map_err(|error| {
                io::Error::new(error.kind(), format!("{}: {error}", absolute.display()))
            })
        })
        .collect()
}

fn encode_request(paths: Vec<PathBuf>, pane: Option<String>) -> io::Result<Vec<u8>> {
    encode_wire_request(WireRequest::Open {
        paths: paths
            .into_iter()
            .map(|path| path.as_os_str().as_bytes().to_vec())
            .collect(),
        pane,
    })
}

fn encode_wire_request(request: WireRequest) -> io::Result<Vec<u8>> {
    let pane = match &request {
        WireRequest::Open { pane, .. }
        | WireRequest::Workspace { pane, .. }
        | WireRequest::Ducky { pane } => pane,
    };
    validate_pane(pane.as_deref())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut bytes = serde_json::to_vec(&request).map_err(io::Error::other)?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "bedterm request is too large",
        ));
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// Ask the instance that spawned this shell to open existing files.
///
/// Relative paths are resolved in the caller's current directory, so `cd` in a
/// shell works independently of the application's original working directory.
pub fn open(paths: Vec<PathBuf>) -> io::Result<()> {
    let paths = resolve_paths(&std::env::current_dir()?, paths)?;
    send_request(encode_request(paths, shell_pane()?)?)
}

/// Promote this shell's window to an editor workspace at its current directory.
pub fn workspace() -> io::Result<()> {
    send_request(encode_wire_request(WireRequest::Workspace {
        root: std::env::current_dir()?.as_os_str().as_bytes().to_vec(),
        pane: shell_pane()?,
    })?)
}

/// Reveal the hidden Ducky mascot in the instance that spawned this shell.
pub fn ducky() -> io::Result<()> {
    send_request(encode_wire_request(WireRequest::Ducky {
        pane: shell_pane()?,
    })?)
}

fn shell_pane() -> io::Result<Option<String>> {
    match std::env::var(PANE_ENV) {
        Ok(pane) => Ok(Some(pane)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "BEDTERM_PANE is not UTF-8",
        )),
    }
}

fn send_request(request: Vec<u8>) -> io::Result<()> {
    let socket = std::env::var_os(SOCKET_ENV).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "BEDTERM_SOCKET is not set; run this command from a shell inside bedterm",
        )
    })?;
    let mut stream = UnixStream::connect(PathBuf::from(socket)).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not connect to bedterm: {error}"),
        )
    })?;
    stream.set_read_timeout(Some(COMMAND_TIMEOUT))?;
    stream.set_write_timeout(Some(COMMAND_TIMEOUT))?;
    stream.write_all(&request)?;
    read_reply(stream)
}

fn read_reply(stream: UnixStream) -> io::Result<()> {
    let mut bytes = Vec::new();
    BufReader::new(stream)
        .take((MAX_MESSAGE_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if bytes.len() > MAX_MESSAGE_BYTES || bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid bedterm acknowledgement",
        ));
    }
    let reply: Reply = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    match reply.error {
        Some(error) => Err(io::Error::other(error)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connect(bridge: &Bridge) -> UnixStream {
        let stream = UnixStream::connect(bridge.socket_path()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        stream
    }

    #[test]
    fn wire_round_trip_preserves_non_utf8_paths() {
        let path = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/a\xff.png".to_vec()));
        let bytes = encode_request(vec![path.clone()], Some("pane-123".into())).unwrap();
        assert_eq!(
            decode_request(&bytes).unwrap(),
            Request::Open {
                paths: vec![path],
                pane: Some("pane-123".into())
            }
        );
    }

    #[test]
    fn workspace_round_trip_preserves_the_callers_directory_and_pane() {
        let root = b"/tmp/project \xff".to_vec();
        for pane in [None, Some("pane-workspace".into())] {
            let bytes = encode_wire_request(WireRequest::Workspace {
                root: root.clone(),
                pane: pane.clone(),
            })
            .unwrap();
            assert_eq!(
                decode_request(&bytes).unwrap(),
                Request::Workspace {
                    root: PathBuf::from(std::ffi::OsString::from_vec(root.clone())),
                    pane,
                }
            );
        }
        for root in [Vec::new(), b"relative".to_vec(), b"/tmp/a\0b".to_vec()] {
            let bytes = encode_wire_request(WireRequest::Workspace { root, pane: None }).unwrap();
            assert!(decode_request(&bytes).is_err());
        }
        assert!(decode_request(br#"{"command":"workspace","root":[47],"pane":""}"#).is_err());
        assert!(decode_request(br#"{"command":"workspace"}"#).is_err());
    }

    #[test]
    fn ducky_requests_preserve_the_pane_and_reject_arguments() {
        for pane in [None, Some("pane-ducky".into())] {
            let bytes = encode_wire_request(WireRequest::Ducky { pane: pane.clone() }).unwrap();
            assert_eq!(decode_request(&bytes).unwrap(), Request::Ducky { pane });
        }
        assert!(decode_request(br#"{"command":"ducky","pane":""}"#).is_err());
        assert!(decode_request(br#"{"command":"ducky","paths":[[47]]}"#).is_err());
    }

    #[test]
    fn requests_without_a_pane_remain_supported_and_invalid_tokens_are_rejected() {
        let paths = vec![PathBuf::from("/tmp/a.png")];
        let bytes = encode_request(paths.clone(), None).unwrap();
        assert_eq!(
            decode_request(&bytes).unwrap(),
            Request::Open {
                paths: paths.clone(),
                pane: None
            }
        );
        assert!(encode_request(paths.clone(), Some(String::new())).is_err());
        assert!(encode_request(paths.clone(), Some("a".repeat(MAX_PANE_BYTES + 1))).is_err());
        assert!(encode_request(paths, Some("a\0b".into())).is_err());
        assert!(decode_request(br#"{"command":"open","paths":[[47,97]],"pane":""}"#).is_err());
    }

    #[test]
    fn rejects_malformed_relative_empty_and_oversize_requests() {
        assert!(decode_request(b"not json").is_err());
        assert!(
            decode_request(&encode_request(vec![PathBuf::from("relative")], None).unwrap())
                .is_err()
        );
        assert!(decode_request(&encode_request(Vec::new(), None).unwrap()).is_err());
        assert!(decode_request(&vec![b' '; MAX_MESSAGE_BYTES + 1]).is_err());
        let nul = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/a\0b".to_vec()));
        assert!(decode_request(&encode_request(vec![nul], None).unwrap()).is_err());
    }

    #[test]
    fn resolves_paths_using_the_callers_directory() {
        let directory = PrivateDirectory::new().unwrap();
        fs::create_dir(directory.0.join("subdir")).unwrap();
        fs::write(directory.0.join("sample.csv"), "a,b\n1,2\n").unwrap();
        let result =
            resolve_paths(&directory.0.join("subdir"), vec!["../sample.csv".into()]).unwrap();
        assert_eq!(
            result,
            vec![directory.0.join("sample.csv").canonicalize().unwrap()]
        );
        assert!(resolve_paths(&directory.0, vec!["missing".into()]).is_err());
        assert!(resolve_paths(&directory.0, Vec::new()).is_err());
        fs::remove_file(directory.0.join("sample.csv")).unwrap();
        fs::remove_dir(directory.0.join("subdir")).unwrap();
    }

    #[test]
    fn partial_requests_do_not_block_and_are_handled_once() {
        let mut bridge = Bridge::new().unwrap();
        let mut stream = connect(&bridge);
        let expected = PathBuf::from("/tmp/sample.csv");
        let bytes = encode_request(vec![expected.clone()], Some("pane-456".into())).unwrap();
        let middle = bytes.len() / 2;
        stream.write_all(&bytes[..middle]).unwrap();
        bridge
            .poll(|_| panic!("partial request must not dispatch"))
            .unwrap();
        stream.write_all(&bytes[middle..]).unwrap();
        let mut requests = Vec::new();
        bridge
            .poll(|request| {
                requests.push(request);
                Ok(())
            })
            .unwrap();
        bridge
            .poll(|_| panic!("request must dispatch only once"))
            .unwrap();
        assert_eq!(
            requests,
            vec![Request::Open {
                paths: vec![expected],
                pane: Some("pane-456".into()),
            }]
        );
        read_reply(stream).unwrap();
    }

    #[test]
    fn handler_errors_reach_the_command() {
        let mut bridge = Bridge::new().unwrap();
        let mut stream = connect(&bridge);
        stream
            .write_all(&encode_request(vec!["/tmp/sample.csv".into()], None).unwrap())
            .unwrap();
        bridge
            .poll(|_| Err("unsupported file type".into()))
            .unwrap();
        assert_eq!(
            read_reply(stream).unwrap_err().to_string(),
            "unsupported file type"
        );
    }

    #[test]
    fn abandoned_clients_expire_and_socket_is_private_and_removed() {
        let mut bridge = Bridge::new().unwrap();
        let socket = bridge.socket_path().to_owned();
        let directory = socket.parent().unwrap().to_owned();
        let helpers = HELPERS
            .iter()
            .map(|helper| bridge.helper_directory().join(helper))
            .collect::<Vec<_>>();
        for helper in &helpers {
            assert_eq!(
                fs::read_link(helper).unwrap(),
                std::env::current_exe().unwrap()
            );
        }
        assert_eq!(
            fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let _stream = connect(&bridge);
        bridge
            .poll(|_| panic!("empty request must not dispatch"))
            .unwrap();
        assert_eq!(bridge.clients.len(), 1);
        bridge.clients[0].deadline = Instant::now();
        bridge
            .poll(|_| panic!("expired request must not dispatch"))
            .unwrap();
        assert!(bridge.clients.is_empty());
        drop(bridge);
        assert!(!socket.exists());
        assert!(helpers.iter().all(|helper| !helper.exists()));
        assert!(!directory.exists());
    }
}
