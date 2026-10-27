//! View-owned cancellable CPU work. Workers only read immutable snapshots;
//! accepted results are applied explicitly during the next editor draw.
use bedtime_ui::egui;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

pub(crate) struct Job<T> {
    cancel: Arc<AtomicBool>,
    receiver: Mutex<mpsc::Receiver<T>>,
}
impl<T: Send + 'static> Job<T> {
    pub fn start(
        ctx: &egui::Context,
        work: impl FnOnce(&AtomicBool) -> Option<T> + Send + 'static,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(value) = work(&flag) {
                if !flag.load(Ordering::Relaxed) {
                    let _ = sender.send(value);
                }
            }
            ctx.request_repaint();
        });
        Self {
            cancel,
            receiver: Mutex::new(receiver),
        }
    }
    pub fn poll(&self) -> Result<Option<T>, ()> {
        match self.receiver.lock().unwrap().try_recv() {
            Ok(value) => Ok(Some(value)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(()),
        }
    }
}
impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
