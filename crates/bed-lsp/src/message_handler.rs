//! Main-thread JSON-RPC dispatch with owned, bounded stdio workers.
//! No worker invokes a callback or retains an editor/UI reference.
use std::io;
use std::thread;

use crate::{
    connection::{Connection, ConnectionError, encode_packet},
    jsonrpc::{
        CONNECTION_CLOSED, METHOD_NOT_FOUND, Message, Packet, REQUEST_CANCELLED, Request, Response,
        ResponseError, RpcId,
    },
    lsp_document_sync::NotificationSink,
    process::{Process, ProcessOptions, ProcessPipes},
};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    io::{BufReader, Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub const MAX_PENDING_REQUESTS: usize = 1024;
pub const MAX_QUEUED_WRITE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_STDERR_BYTES: usize = 64 * 1024;
pub const MAX_CONSECUTIVE_MESSAGE_ERRORS: usize = 16;
const INBOUND_CAPACITY: usize = 8;
const OUTBOUND_CAPACITY: usize = 64;
static NEXT_ID: AtomicI32 = AtomicI32::new(1);

type ResponseCallback = Box<dyn FnOnce(Result<Value, ResponseError>)>;
type RequestHandler = Box<dyn FnMut(Option<Value>) -> Result<Value, ResponseError>>;
type NotificationHandler = Box<dyn FnMut(Option<Value>) -> Result<(), ResponseError>>;

#[derive(Debug)]
pub enum RpcEvent {
    Notification {
        method: String,
        params: Option<Value>,
    },
    ProtocolError(ResponseError),
    Disconnected(String),
}

enum WorkerEvent {
    Packet(Packet),
    Invalid(ResponseError),
    Disconnected(String),
}

struct QueuedFrame {
    bytes: Vec<u8>,
    budget: Arc<AtomicUsize>,
    completion: Option<mpsc::Sender<io::Result<()>>>,
}

impl Drop for QueuedFrame {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.bytes.len(), Ordering::AcqRel);
    }
}

enum WriteCommand {
    Frame(QueuedFrame),
    Stop,
}

pub struct RpcSession {
    process: Process,
    writer: Option<mpsc::SyncSender<WriteCommand>>,
    receiver: Option<mpsc::Receiver<WorkerEvent>>,
    workers: Vec<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    write_budget: Arc<AtomicUsize>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    pending: HashMap<RpcId, ResponseCallback>,
    handlers: HashMap<String, RequestHandler>,
    notification_handlers: HashMap<String, NotificationHandler>,
    connected: bool,
    shutdown_complete: bool,
}

impl RpcSession {
    pub fn start(program: &Path, arguments: &[String]) -> io::Result<Self> {
        Self::start_with_options(program, arguments, &ProcessOptions::default())
    }
    pub fn start_with_options(
        program: &Path,
        arguments: &[String],
        options: &ProcessOptions,
    ) -> io::Result<Self> {
        let (
            process,
            ProcessPipes {
                mut stdin,
                stdout,
                mut stderr,
            },
        ) = Process::start_with_options(program, arguments, options)?;
        let (event_sender, receiver) = mpsc::sync_channel(INBOUND_CAPACITY);
        let (writer, write_receiver) = mpsc::sync_channel(OUTBOUND_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let write_budget = Arc::new(AtomicUsize::new(0));
        let error_bytes = Arc::new(Mutex::new(VecDeque::new()));
        let mut workers = Vec::new();
        let reader_stop = stop.clone();
        let read_events = event_sender.clone();
        workers.push(
            thread::Builder::new()
                .name("bed-lsp-read".into())
                .spawn(move || {
                    let mut connection = Connection::new(BufReader::new(stdout));
                    let mut errors = 0;
                    while !reader_stop.load(Ordering::Acquire) {
                        let event = match connection.read_packet() {
                            Ok(packet) => {
                                errors = 0;
                                WorkerEvent::Packet(packet)
                            }
                            Err(ConnectionError::Message(error)) => {
                                errors += 1;
                                if read_events.send(WorkerEvent::Invalid(error)).is_err() {
                                    break;
                                }
                                if errors < MAX_CONSECUTIVE_MESSAGE_ERRORS {
                                    continue;
                                }
                                WorkerEvent::Disconnected(
                                    "Too many consecutive malformed LSP messages".into(),
                                )
                            }
                            Err(error) => WorkerEvent::Disconnected(error.to_string()),
                        };
                        let finished = matches!(event, WorkerEvent::Disconnected(_));
                        if read_events.send(event).is_err() || finished {
                            break;
                        }
                    }
                })?,
        );
        let writer_stop = stop.clone();
        workers.push(
            thread::Builder::new()
                .name("bed-lsp-write".into())
                .spawn(move || {
                    while let Ok(command) = write_receiver.recv() {
                        let WriteCommand::Frame(mut frame) = command else {
                            break;
                        };
                        if writer_stop.load(Ordering::Acquire) {
                            break;
                        }
                        let result = stdin.write_all(&frame.bytes).and_then(|()| stdin.flush());
                        if let Some(completion) = frame.completion.take() {
                            let _ =
                                completion.send(result.as_ref().map(|_| ()).map_err(|error| {
                                    io::Error::new(error.kind(), error.to_string())
                                }));
                        }
                        if let Err(error) = result {
                            let _ = event_sender.send(WorkerEvent::Disconnected(error.to_string()));
                            break;
                        }
                    }
                })?,
        );
        let retained_errors = error_bytes.clone();
        workers.push(
            thread::Builder::new()
                .name("bed-lsp-stderr".into())
                .spawn(move || {
                    let mut bytes = [0; 4096];
                    loop {
                        match stderr.read(&mut bytes) {
                            Ok(0) => break,
                            Ok(length) => {
                                let mut retained = retained_errors
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner());
                                let excess = retained
                                    .len()
                                    .saturating_add(length)
                                    .saturating_sub(MAX_STDERR_BYTES);
                                retained.drain(..excess);
                                retained.extend(&bytes[..length]);
                            }
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                            Err(_) => break,
                        }
                    }
                })?,
        );
        Ok(Self {
            process,
            writer: Some(writer),
            receiver: Some(receiver),
            workers,
            stop,
            write_budget,
            stderr: error_bytes,
            pending: HashMap::new(),
            handlers: HashMap::new(),
            notification_handlers: HashMap::new(),
            connected: true,
            shutdown_complete: false,
        })
    }

    pub fn process_id(&self) -> u32 {
        self.process.id()
    }
    pub fn is_connected(&self) -> bool {
        self.connected
    }
    pub fn pending_request_count(&self) -> usize {
        self.pending.len()
    }

    pub fn register_request_handler(
        &mut self,
        method: impl Into<String>,
        handler: impl FnMut(Option<Value>) -> Result<Value, ResponseError> + 'static,
    ) {
        self.handlers.insert(method.into(), Box::new(handler));
    }

    pub fn register_notification_handler(
        &mut self,
        method: impl Into<String>,
        handler: impl FnMut(Option<Value>) -> Result<(), ResponseError> + 'static,
    ) {
        self.notification_handlers
            .insert(method.into(), Box::new(handler));
    }

    pub fn send_request(
        &mut self,
        method: &str,
        params: Option<Value>,
        callback: impl FnOnce(Result<Value, ResponseError>) + 'static,
    ) -> io::Result<RpcId> {
        if self.pending.len() >= MAX_PENDING_REQUESTS {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Too many pending LSP requests",
            ));
        }
        let id = loop {
            let number = NEXT_ID
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |number| {
                    Some(if number == i32::MAX { 1 } else { number + 1 })
                })
                .unwrap();
            let id = RpcId::Number(number);
            if !self.pending.contains_key(&id) {
                break id;
            }
        };
        let packet = Packet::Single(Message::Request(Request {
            id: Some(id.clone()),
            method: method.into(),
            params,
        }));
        self.queue(&packet, None)?;
        self.pending.insert(id.clone(), Box::new(callback));
        Ok(id)
    }

    pub fn send_notification(&self, method: &str, params: Option<Value>) -> io::Result<()> {
        self.queue(
            &Packet::Single(Message::Request(Request {
                id: None,
                method: method.into(),
                params,
            })),
            None,
        )
    }

    fn queue(
        &self,
        packet: &Packet,
        completion: Option<mpsc::Sender<io::Result<()>>>,
    ) -> io::Result<()> {
        if !self.connected {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "LSP server is disconnected",
            ));
        }
        let bytes = encode_packet(packet)?;
        self.write_budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes.len())
                    .filter(|total| *total <= MAX_QUEUED_WRITE_BYTES)
            })
            .map_err(|_| {
                io::Error::new(io::ErrorKind::WouldBlock, "LSP write queue exceeds 64 MiB")
            })?;
        let frame = QueuedFrame {
            bytes,
            budget: self.write_budget.clone(),
            completion,
        };
        let writer = self
            .writer
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "LSP writer stopped"))?;
        writer
            .try_send(WriteCommand::Frame(frame))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    io::Error::new(io::ErrorKind::WouldBlock, "LSP write queue is full")
                }
                mpsc::TrySendError::Disconnected(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "LSP writer stopped")
                }
            })
    }

    /// Invoke responses and server request handlers only on the calling thread.
    pub fn poll(&mut self) -> Vec<RpcEvent> {
        self.poll_after_message(|_| {})
    }

    pub(crate) fn poll_after_message(
        &mut self,
        mut after_message: impl FnMut(&mut Self),
    ) -> Vec<RpcEvent> {
        let mut events = Vec::new();
        for _ in 0..256 {
            let event = match self.receiver.as_ref().map(mpsc::Receiver::try_recv) {
                Some(Ok(event)) => event,
                Some(Err(mpsc::TryRecvError::Empty)) | None => break,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    if self.connected {
                        self.disconnect("LSP workers stopped".into(), &mut events);
                    }
                    break;
                }
            };
            match event {
                WorkerEvent::Packet(packet) => {
                    self.dispatch(packet, &mut events, &mut after_message)
                }
                WorkerEvent::Invalid(error) => {
                    let _ = self.queue(
                        &Packet::Single(Message::Response(Response {
                            id: RpcId::Null,
                            result: Err(error.clone()),
                        })),
                        None,
                    );
                    events.push(RpcEvent::ProtocolError(error));
                }
                WorkerEvent::Disconnected(error) => {
                    self.disconnect(error, &mut events);
                    break;
                }
            }
        }
        if self.connected {
            match self.process.try_wait() {
                Ok(Some(status)) => {
                    self.disconnect(format!("LSP process exited: {status}"), &mut events)
                }
                Err(error) => self.disconnect(error.to_string(), &mut events),
                Ok(None) => {}
            }
        }
        events
    }

    fn dispatch(
        &mut self,
        packet: Packet,
        events: &mut Vec<RpcEvent>,
        after_message: &mut impl FnMut(&mut Self),
    ) {
        let (messages, batch) = match packet {
            Packet::Single(message) => (vec![message], false),
            Packet::Batch(messages) => (messages, true),
        };
        let mut responses = Vec::new();
        for message in messages {
            match message {
                Message::Response(response) => {
                    if let Some(callback) = self.pending.remove(&response.id) {
                        callback(response.result);
                    }
                }
                Message::Request(request) => {
                    if let Some(handler) = self.notification_handlers.get_mut(&request.method) {
                        if let Err(error) = handler(request.params) {
                            events.push(RpcEvent::ProtocolError(error));
                        }
                    } else if let Some(id) = request.id {
                        let result = match self.handlers.get_mut(&request.method) {
                            Some(handler) => handler(request.params),
                            None => Err(ResponseError::new(METHOD_NOT_FOUND, "Method not found")),
                        };
                        responses.push(Message::Response(Response { id, result }));
                    } else {
                        events.push(RpcEvent::Notification {
                            method: request.method,
                            params: request.params,
                        });
                    }
                }
            }
            // Initialization has observable side effects before the next item
            // in an incoming batch. The client consumes its callback channel at
            // this boundary rather than delaying it until an entire poll ends.
            after_message(self);
        }
        if !responses.is_empty() {
            let packet = if batch {
                Packet::Batch(responses)
            } else {
                Packet::Single(responses.remove(0))
            };
            if let Err(error) = self.queue(&packet, None) {
                self.disconnect(error.to_string(), events);
            }
        }
    }

    fn fail_pending(&mut self, error: ResponseError) {
        for (_, callback) in self.pending.drain() {
            callback(Err(error.clone()));
        }
    }

    fn disconnect(&mut self, error: String, events: &mut Vec<RpcEvent>) {
        if !self.connected {
            return;
        }
        self.connected = false;
        self.fail_pending(ResponseError::new(CONNECTION_CLOSED, error.clone()));
        self.stop_workers(Duration::ZERO);
        events.push(RpcEvent::Disconnected(error));
    }

    pub fn take_stderr(&self) -> Vec<u8> {
        self.stderr
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .drain(..)
            .collect()
    }

    /// Keep upstream's 500 ms shutdown handshake, then close/kill/reap with a
    /// bounded worker join. Dropping the receiver releases blocked event senders.
    pub fn shutdown(&mut self) {
        if self.shutdown_complete {
            return;
        }
        if self.connected {
            let (sent, received) = mpsc::channel();
            if self
                .send_request("shutdown", None, move |result| {
                    let _ = sent.send(result);
                })
                .is_ok()
            {
                let deadline = Instant::now() + Duration::from_millis(500);
                while self.connected && Instant::now() < deadline {
                    self.poll();
                    if received.try_recv().is_ok() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
            }
            if self.connected {
                let (sent, received) = mpsc::channel();
                let packet = Packet::Single(Message::Request(Request {
                    id: None,
                    method: "exit".into(),
                    params: None,
                }));
                if self.queue(&packet, Some(sent)).is_ok() {
                    let _ = received.recv_timeout(Duration::from_millis(100));
                }
            }
        }
        self.connected = false;
        self.fail_pending(ResponseError::new(REQUEST_CANCELLED, "LSP client shutdown"));
        self.stop_workers(Duration::from_millis(100));
        self.shutdown_complete = true;
    }

    fn stop_workers(&mut self, grace: Duration) {
        self.receiver.take();
        if let Some(writer) = self.writer.take() {
            let _ = writer.try_send(WriteCommand::Stop);
        }
        let _ = self.process.terminate(grace);
        self.stop.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_millis(100);
        while self.workers.iter().any(|worker| !worker.is_finished()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        for worker in self.workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            }
            // A descendant may retain a pipe. The remaining worker owns its pipe,
            // atomic flags and channels; dropping JoinHandle safely detaches it.
        }
    }
}

impl NotificationSink for RpcSession {
    fn send_notification(&self, method: &str, params: Option<Value>) -> io::Result<()> {
        self.send_notification(method, params)
    }
}

impl Drop for RpcSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}
