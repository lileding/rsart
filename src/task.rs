use std::pin::Pin;
use std::future::Future;
use std::task::{Poll, Context, Wake};
use std::sync::{Arc, Mutex};

use super::Runtime;

use log::trace;

pub(crate) struct Task {
    id: usize,
    fut: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl Task {
    pub(crate) fn new(id: usize, fut: impl Future<Output = ()> + Send + 'static) -> Self {
        Task { id, fut: Box::pin(fut) }
    }

    pub(crate) fn id(&self) -> usize {
        self.id
    }

    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.fut.as_mut().poll(cx)
    }
}

pub(crate) struct Waker(Mutex<Option<Task>>);

impl Waker {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Waker(Mutex::new(None)))
    }

    pub(crate) fn park(self: Arc<Self>, task: Task) {
        let tid = task.id;
        trace!("[Waker] park task, tid={}", tid);
        self.0.lock().unwrap().replace(task);
        Runtime::current().scheduler.park(tid, self);
    }
}

impl Wake for Waker {
    fn wake(self: Arc<Self>) {
        if let Some(task) = self.0.lock().unwrap().take() {
            trace!("[Waker] wake task, tid={}", task.id);
            Runtime::current().scheduler.activate(task);
        }
    }
}

