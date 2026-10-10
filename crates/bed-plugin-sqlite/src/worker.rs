use crate::backend;
pub use crate::backend::{Request, Response};
use std::{
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

pub struct Reply {
    pub id: u64,
    pub result: Result<Response, String>,
}

struct State {
    pending: Option<(u64, Request)>,
    reply: Option<Reply>,
    closed: bool,
}

pub struct Worker {
    state: Arc<(Mutex<State>, Condvar)>,
    generation: Arc<AtomicU64>,
}

impl Worker {
    pub fn new(path: PathBuf) -> Self {
        let state = Arc::new((
            Mutex::new(State {
                pending: None,
                reply: None,
                closed: false,
            }),
            Condvar::new(),
        ));
        let generation = Arc::new(AtomicU64::new(0));
        let active = Arc::clone(&generation);
        let jobs = Arc::clone(&state);
        thread::Builder::new()
            .name("sqlite-viewer".into())
            .spawn(move || {
                loop {
                    let (lock, ready) = &*jobs;
                    let mut state = lock.lock().expect("SQLite worker state");
                    while state.pending.is_none() && !state.closed {
                        state = ready.wait(state).expect("SQLite worker state");
                    }
                    if state.closed {
                        break;
                    }
                    let (id, request) = state.pending.take().expect("pending SQLite request");
                    drop(state);
                    let result = backend::execute(&path, request, Arc::clone(&active), id);
                    let mut state = lock.lock().expect("SQLite worker state");
                    if !state.closed && active.load(Ordering::Relaxed) == id {
                        state.reply = Some(Reply { id, result });
                    }
                }
            })
            .expect("start SQLite worker");
        Self { state, generation }
    }

    // A single pending slot replaces superseded requests, rather than growing
    // a queue while the user changes tables or submits another query.
    pub fn request(&mut self, request: Request) -> u64 {
        let id = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let (lock, ready) = &*self.state;
        let mut state = lock.lock().expect("SQLite worker state");
        state.pending = Some((id, request));
        state.reply = None;
        ready.notify_one();
        id
    }

    pub fn poll(&mut self) -> Option<Reply> {
        let mut state = self.state.0.lock().expect("SQLite worker state");
        state
            .reply
            .take()
            .filter(|reply| reply.id == self.generation.load(Ordering::Relaxed))
    }

    pub fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        let mut state = self.state.0.lock().expect("SQLite worker state");
        state.pending = None;
        state.reply = None;
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        let (lock, ready) = &*self.state;
        lock.lock().expect("SQLite worker state").closed = true;
        ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn only_the_latest_request_is_delivered_and_closing_does_not_block() {
        let path =
            std::env::temp_dir().join(format!("bed-sqlite-worker-{}.db", std::process::id()));
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE IF NOT EXISTS t(n);")
            .unwrap();
        let mut worker = Worker::new(path.clone());
        worker.request(Request::Query { sql: "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n".into() });
        let latest = worker.request(Request::Query {
            sql: "SELECT 42".into(),
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let reply = loop {
            if let Some(reply) = worker.poll() {
                break reply;
            }
            assert!(Instant::now() < deadline, "latest SQLite request was lost");
            thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(reply.id, latest);
        let Response::Data(data) = reply.result.unwrap() else {
            panic!()
        };
        assert_eq!(data.rows[0][0], backend::Cell::Integer(42));
        worker.request(Request::Query {
            sql: "SELECT 1".into(),
        });
        worker.cancel();
        assert!(worker.poll().is_none());
        worker.request(Request::Query { sql: "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n".into() });
        let started = Instant::now();
        drop(worker);
        assert!(started.elapsed() < Duration::from_secs(1));
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match std::fs::remove_file(&path) {
                Ok(()) => break,
                Err(error) if Instant::now() < deadline => {
                    let _ = error;
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("worker did not release its database: {error}"),
            }
        }
    }
}
