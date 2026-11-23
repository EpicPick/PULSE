#![forbid(unsafe_code)]
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// Capacity-one mailbox; a busy consumer always takes the newest complete snapshot.
pub struct Mailbox<T>(Arc<Mutex<Option<Arc<T>>>>);

impl<T> Clone for Mailbox<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for Mailbox<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

impl<T> Mailbox<T> {
    pub fn publish(&self, value: T) {
        *self.0.lock().unwrap() = Some(Arc::new(value));
    }
    pub fn take(&self) -> Option<Arc<T>> {
        self.0.lock().unwrap().take()
    }
}

pub struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// The factory crosses threads; the sampler (and any OS handle it owns) does not.
    pub fn spawn<F>(init: impl FnOnce() -> F + Send + 'static) -> std::io::Result<Self>
    where
        F: FnMut() -> Duration,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = thread::Builder::new()
            .name("pulse-collector".into())
            .spawn(move || {
                let mut step = init();
                while !flag.load(Ordering::Acquire) {
                    let sleep = step();
                    if !flag.load(Ordering::Acquire) {
                        thread::park_timeout(sleep);
                    }
                }
            })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_consumer_gets_latest_without_mutating_held_snapshot() {
        let m = Mailbox::default();
        m.publish(1);
        let held = m.take().unwrap();
        m.publish(2);
        m.clone().publish(3);
        assert_eq!(*m.take().unwrap(), 3);
        assert_eq!(*held, 1);
        assert!(m.take().is_none());
    }
}
