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

type Task = Pin<Box<dyn Future<Output = ()> + Send>>;

pub struct Runtime {
    next_token: Cell<usize>,
    poll: RefCell<mio::Poll>,
    pending_tasks: RefCell<HashMap<mio::Token, Waker>>,
    active_tasks: RefCell<VecDeque<Task>>,
}

impl Runtime {
    pub fn new() -> Result<Arc<Self>> {
        Ok(Arc::new(Runtime {
            next_token: Cell::new(1000),
            poll: RefCell::new(mio::Poll::new()?),
            pending_tasks: RefCell::new(HashMap::new()),
            active_tasks: RefCell::new(VecDeque::new()),
        }))
    }

    fn current() -> Arc<Self> {
        CONTEXT.with_borrow(Arc::clone)
    }

    fn block_on<F>(&self, task: F) -> F::Output where
        F: Future<Output = ()> + Send + 'static {
        let task = Box::pin(task);
        self.active_tasks.borrow_mut().push_back(task);
        while !self.active_tasks.borrow().is_empty() || !self.pending_tasks.borrow().is_empty() {
            let mut tq = self.active_tasks.replace(VecDeque::new());
            while let Some(mut task) = tq.pop_front() {
                let event_waker = Arc::new(EventWaker { task: Mutex::new(None) });
                let waker = Arc::clone(&event_waker).into();
                let mut cx = Context::from_waker(&waker);
                if task.as_mut().poll(&mut cx).is_pending() {
                    event_waker.task.lock().unwrap().replace(task);
                }
            }
            self.wait().unwrap();
        }
    }

    fn spawn<F: Future<Output = ()> + Send + 'static>(&self, task: F) -> F::Output {
        self.active_tasks.borrow_mut().push_back(Box::pin(task));
    }

    fn wait(&self) -> Result<()> {
        if self.pending_tasks.borrow().is_empty() {
            return Ok(());
        }
        let mut events = mio::Events::with_capacity(10);
        self.poll.borrow_mut().poll(&mut events, None)?;
        for event in &events {
            if let Some(waker) = self.pending_tasks.borrow_mut().remove(&event.token()) {
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
        self.pending_tasks.borrow_mut().insert(mio::Token(token), waker);
        self.poll.borrow_mut().registry().register(event, mio::Token(token), interest)?;
        self.next_token.set(token + 1);
        Ok(())
    }

    fn sleep(&self, duration: Duration) -> Result<Timer> {
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

thread_local! {
    pub static CONTEXT: RefCell<Arc<Runtime>> = RefCell::new(Runtime::new().unwrap());
}

pub fn spawn<F: Future<Output = ()> + Send + 'static>(task: F) -> F::Output {
    Runtime::current().spawn(task);
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
    F: Future<Output = ()> + Send + 'static {
    Runtime::current().block_on(task)
}

pub fn sleep(duration: Duration) -> Timer {
    Runtime::current().sleep(duration).unwrap()
}
