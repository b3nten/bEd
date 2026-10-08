use crate::model::{self, ColumnFilter, Sort, Table};
use bed_plugin::Revision;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

pub struct Job {
    pub serial: u64,
    pub revision: Revision,
    pub bytes: Arc<[u8]>,
    pub path: String,
    pub delimiter: Option<u8>,
    pub header: Option<bool>,
    pub global_filter: String,
    pub filters: Vec<ColumnFilter>,
    pub sort: Option<Sort>,
}
pub struct Output {
    pub serial: u64,
    pub revision: Revision,
    pub result: Result<View, String>,
}
pub struct View {
    pub table: Arc<Table>,
    pub rows: Arc<[usize]>,
    pub header: bool,
}
pub struct Worker {
    sender: Option<mpsc::SyncSender<Job>>,
    receiver: mpsc::Receiver<Output>,
    serial: Arc<AtomicU64>,
}
impl Worker {
    pub fn new() -> Self {
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, receiver) = mpsc::channel();
        let serial = Arc::new(AtomicU64::new(0));
        let active = Arc::clone(&serial);
        thread::Builder::new()
            .name("csv-table".into())
            .spawn(move || {
                let mut cached: Option<(Revision, Option<u8>, Arc<Table>)> = None;
                while let Ok(job) = jobs.recv() {
                    let cancelled = || active.load(Ordering::Relaxed) != job.serial;
                    if cancelled() {
                        continue;
                    }
                    let result = (|| {
                        let table = if let Some((revision, delimiter, table)) = &cached
                            && *revision == job.revision
                            && *delimiter == job.delimiter
                        {
                            Arc::clone(table)
                        } else {
                            let table = Arc::new(model::parse(
                                job.bytes,
                                job.delimiter,
                                &job.path,
                                cancelled,
                            )?);
                            cached = Some((job.revision, job.delimiter, Arc::clone(&table)));
                            table
                        };
                        let header = job.header.unwrap_or(table.detected_header)
                            && !table.records.is_empty();
                        // Raw text edits can remove columns. Obsolete view preferences must not
                        // turn a valid document into a parse failure.
                        let filters: Vec<_> = job
                            .filters
                            .into_iter()
                            .filter(|filter| filter.column < table.columns)
                            .collect();
                        let sort = job.sort.filter(|sort| sort.column < table.columns);
                        let rows = table.visible_rows(
                            header,
                            &job.global_filter,
                            &filters,
                            sort.as_ref(),
                            cancelled,
                        )?;
                        Ok(View {
                            table,
                            rows: rows.into(),
                            header,
                        })
                    })();
                    if !cancelled()
                        && results
                            .send(Output {
                                serial: job.serial,
                                revision: job.revision,
                                result,
                            })
                            .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("start CSV worker");
        Self {
            sender: Some(sender),
            receiver,
            serial,
        }
    }
    pub fn invalidate(&self) -> u64 {
        self.serial.fetch_add(1, Ordering::Relaxed) + 1
    }
    pub fn submit(&self, job: Job) -> bool {
        self.sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(job).is_ok())
    }
    pub fn poll(&self) -> impl Iterator<Item = Output> + '_ {
        self.receiver.try_iter()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.invalidate();
        self.sender.take();
    }
}
