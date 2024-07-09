use std::io::Result;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Waker;
use std::sync::Mutex;
use std::collections::HashMap;

use mio::event::Source;

use log::trace;

pub(crate) struct Executor {
    next_token: AtomicUsize,
    poll: Mutex<mio::Poll>,
    waitings: Mutex<HashMap<mio::Token, Waker>>,
}

impl Executor {
    pub(crate) fn new() -> Result<Self> {
        Ok(Executor {
            next_token: AtomicUsize::new(0),
            poll: Mutex::new(mio::Poll::new()?),
            waitings: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn submit(&self, src: &mut impl Source, interest: mio::Interest, waker: Waker) -> Result<()> {
        let token = mio::Token(self.next_token.fetch_add(1, Ordering::AcqRel));
        self.waitings.lock().unwrap().insert(token, waker);
        self.poll.lock().unwrap().registry().register(src, token, interest)
    }

    // return true if there is NO pending task
    pub(crate) fn execute(&self) -> Result<bool> {
        if self.waitings.lock().unwrap().is_empty() {
            trace!("[Executor] there is no waiting task, pending=false");
            return Ok(true);
        }
        trace!("[Executor] enter event wait");
        let mut events = mio::Events::with_capacity(10);
        self.poll.lock().unwrap().poll(&mut events, None)?;
        for event in &events {
            if let Some(waker) = self.waitings.lock().unwrap().remove(&event.token()) {
                trace!("[Executor] got event and wake task");
                waker.wake_by_ref();
            }
        }
        trace!("[Executor] exit event wait");
        Ok(self.waitings.lock().unwrap().is_empty())
    }
}
