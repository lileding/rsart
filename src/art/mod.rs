use std::io::Result;
use std::cell::{Cell, RefCell};
use std::collections::{VecDeque, HashMap};
use std::future::Future;
use std::task::{Context, Poll, Wake, Waker};
use std::sync::{Arc, Mutex};
use std::pin::Pin;
use std::time::Duration;

use self::timer::TimerFd;

extern crate mio;

mod timer;

struct EventWaker {
    task: Mutex<Option<Task>>,
}

impl Wake for EventWaker {
    fn wake(self: Arc<Self>) {
        if let Some(task) = self.task.lock().unwrap().take() {
            Runtime::current().wake(task);
        }
    }
}

pub struct Runtime {
    next_token: Cell<usize>,
    poll: RefCell<mio::Poll>,
    event_handlers: RefCell<HashMap<mio::Token, Waker>>,
    active_tasks: RefCell<VecDeque<Task>>,
}

unsafe impl Send for Runtime {}
unsafe impl Sync for Runtime {}

impl Runtime {
    pub fn new() -> Result<Arc<Self>> {
        Ok(Arc::new(Runtime {
            next_token: Cell::new(1000),
            poll: RefCell::new(mio::Poll::new()?),
            event_handlers: RefCell::new(HashMap::new()),
            active_tasks: RefCell::new(VecDeque::new()),
        }))
    }

    fn current() -> Arc<Self> {
        CONTEXT.with_borrow(Arc::clone)
    }

    pub fn block_on<F>(&self, task: F) -> F::Output where
            F: Future<Output = ()> + Send + Sync + 'static {
        let task = Box::pin(task);
        self.active_tasks.borrow_mut().push_back(task);
        loop {
            let mut tq = self.active_tasks.borrow_mut();
            let task = tq.pop_front();
            drop(tq);
            if let Some(mut task) = task {
                let event_waker = Arc::new(EventWaker { task: Mutex::new(None) });
                let waker = Arc::clone(&event_waker).into();
                let mut cx = Context::from_waker(&waker);
                if task.as_mut().poll(&mut cx).is_pending() {
                    event_waker.task.lock().unwrap().replace(task);
                }
            }
            if !self.event_handlers.borrow().is_empty() {
                self.wait().unwrap();
            }
            if self.active_tasks.borrow().is_empty() {
                break;
            }
        }
    }

    fn wait(&self) -> Result<()> {
        let mut events = mio::Events::with_capacity(10);
        self.poll.borrow_mut().poll(&mut events, None)?;
        for event in &events {
            if let Some(waker) = self.event_handlers.borrow_mut().remove(&event.token()) {
                waker.wake_by_ref();
            }
        }
        Ok(())
    }

    fn wake(&self, task: Task) {
        self.active_tasks.borrow_mut().push_back(task);
    }

    fn submit(&self, event: &mut impl mio::event::Source, interest: mio::Interest, waker: Waker) -> Result<()> {
        let token = self.next_token.get();
        self.event_handlers.borrow_mut().insert(mio::Token(token), waker);
        self.poll.borrow_mut().registry().register(event, mio::Token(token), interest)?;
        self.next_token.set(token + 1);
        Ok(())
    }

    pub fn sleep(&self, duration: Duration) -> Result<Timer> {
        let mut timer = timer::TimerFd::new(timer::ClockId::Monotonic)?;
        timer.set_timeout(&duration)?;
        let state = Arc::new(Mutex::new(TimerState {
            completed: false,
            waker: None,
        }));
        let waker = Arc::new(TimerWaker(Arc::clone(&state)));
        self.submit(&mut timer, mio::Interest::READABLE, Waker::from(waker))?;
        Ok(Timer { state, timer })
    }

}

pub type Task = Pin<Box<dyn Future<Output = ()> + Send + Sync>>;

thread_local! {
    pub static CONTEXT: RefCell<Arc<Runtime>> = RefCell::new(Runtime::new().unwrap());
}

pub fn spawn<F: Future<Output = ()> + Send + Sync + 'static>(task: F) -> F::Output {
    Runtime::current().active_tasks.borrow_mut().push_back(Box::pin(task));
}

pub struct Timer {
    state: Arc<Mutex<TimerState>>,
    #[allow(dead_code)]
    timer: TimerFd,
}

struct TimerState {
    completed: bool,
    waker: Option<Waker>,
}

impl Future for Timer {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.lock().unwrap();
        if state.completed {
            Poll::Ready(())
        } else {
            state.waker.replace(cx.waker().clone());
            Poll::Pending
        }
    }
}

struct TimerWaker(Arc<Mutex<TimerState>>);

impl Wake for TimerWaker {
    fn wake(self: Arc<Self>) {
        let mut state = self.0.lock().unwrap();
        state.completed = true;
        if let Some(waker) = &state.waker {
            waker.wake_by_ref();
        }
    }
}

pub fn block_on<F>(task: F) where
    F: Future<Output = ()> + Send + Sync + 'static {
    Runtime::current().block_on(task)
}

pub fn sleep(duration: Duration) -> Result<Timer> {
    Runtime::current().sleep(duration)
}
