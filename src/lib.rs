use std::io::Result;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::future::Future;
use std::task::{Context, Poll, Wake, Waker};
use std::pin::{Pin, pin};
use std::time::Duration;

use self::executor::Executor;
use self::scheduler::Scheduler;
use self::timer::TimerFd;

use log::trace;

mod timer;
mod task;
mod executor;
mod scheduler;

pub use rsart_macros::main;

pub struct Runtime {
    next_taskid: AtomicUsize,
    scheduler: Scheduler,
    executor: Executor,
}

impl Runtime {
    pub fn new() -> Result<Arc<Self>> {
        Ok(Arc::new(Runtime {
            next_taskid: AtomicUsize::new(0),
            scheduler: Scheduler::new(),
            executor: Executor::new()?,
        }))
    }

    fn current() -> Arc<Self> {
        CONTEXT.with_borrow(Arc::clone)
    }

    fn block_on<F: Future>(&self, task: F) -> F::Output {
        let mut ptask = pin!(task);
        let task_waker = task::Waker::new();
        let waker = Arc::clone(&task_waker).into();
        let mut cx = Context::from_waker(&waker);

        loop {
            let rv = ptask.as_mut().poll(&mut cx);
            self.run();
            if let Poll::Ready(rv) = rv {
                return rv;
            }
        }
    }

    pub fn spawn<F>(&self, task: F) -> JoinHandle<F::Output> where
        F: Future + Send + 'static, F::Output: Send + 'static {
        let tid = self.next_taskid.fetch_add(1, Ordering::AcqRel);
        let wrapper = TaskWrapper::new(tid, self.scheduler.current_taskid(), task);
        let handle = wrapper.join();
        let task = task::Task::new(tid, wrapper);
        self.scheduler.activate(task);
        trace!("[Spawner] new task, id={}", tid);
        handle
    }

    fn run(&self) {
        loop {
            let scheduler_complete = self.scheduler.schedule();
            let executor_complete = self.executor.execute().unwrap();
            if scheduler_complete && executor_complete {
                break;
            }
        }
    }

    fn sleep(&self, duration: Duration) -> Result<SleepFuture> {
        let mut timer = timer::TimerFd::new(timer::ClockId::Monotonic)?;
        timer.set_timeout(&duration)?;
        let state = Arc::new(Mutex::new(SleepState {
            completed: false,
            waker: None,
        }));
        let waker = Arc::new(SleepWaker(Arc::clone(&state)));
        self.executor.submit(&mut timer, mio::Interest::READABLE, Waker::from(waker))?;
        Ok(SleepFuture { state, timer })
    }
}

pub fn block_on<F: Future>(task: F) -> F::Output {
    Runtime::current().block_on(task)
}

pub fn spawn<F>(task: F) -> JoinHandle<F::Output> where
    F: Future + Send + 'static, F::Output: Send + 'static {
    Runtime::current().spawn(task)
}

pub fn sleep(duration: Duration) -> SleepFuture {
    Runtime::current().sleep(duration).unwrap()
}

thread_local! {
    pub static CONTEXT: RefCell<Arc<Runtime>> = RefCell::new(Runtime::new().unwrap());
}

pub struct SleepFuture {
    state: Arc<Mutex<SleepState>>,
    #[allow(dead_code)]
    timer: TimerFd,
}

struct SleepState {
    completed: bool,
    waker: Option<Waker>,
}

impl Future for SleepFuture {
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

struct SleepWaker(Arc<Mutex<SleepState>>);

impl Wake for SleepWaker {
    fn wake(self: Arc<Self>) {
        let mut state = self.0.lock().unwrap();
        state.completed = true;
        if let Some(waker) = &state.waker {
            waker.wake_by_ref();
        }
    }
}

struct TaskWrapper<F> where F: Future + Send + 'static, F::Output: Send + 'static {
    id: usize,
    parent_id: usize,
    task: Pin<Box<F>>,
    result: JoinHandle<F::Output>,
}

impl <F> TaskWrapper<F> where F: Future + Send + 'static, F::Output: Send + 'static {
    fn new(id: usize, parent_id: usize, task: F) -> Self {
        TaskWrapper {
            id,
            parent_id,
            task: Box::pin(task),
            result: JoinHandle::new(),
        }
    }

    fn join(&self) -> JoinHandle<F::Output> {
        self.result.clone()
    }
}

impl <F> Future for TaskWrapper<F> where
F: Future + Send + 'static, F::Output: Send + 'static {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.task.as_mut().poll(cx) {
            Poll::Ready(rv) => {
                trace!("[TaskWrapper] task ready and wake parent, tid={} parent={}", self.id, self.parent_id);
                self.result.set_value(rv);
                Runtime::current().scheduler.wake_by_id(self.parent_id);
                Poll::Ready(())
            },
            Poll::Pending => {
                trace!("[TaskWrapper] task not ready, tid={}", self.id);
                Poll::Pending
            },
        }
    }
}

pub struct JoinHandle<T>(Arc<Mutex<Option<T>>>);

impl <T> JoinHandle<T> {
    fn new() -> Self {
        JoinHandle(Arc::new(Mutex::new(None)))
    }

    fn take_value(&self) -> Option<T> {
        self.0.lock().unwrap().take()
    }

    fn set_value(&self, value: T) -> Option<T> {
        self.0.lock().unwrap().replace(value)
    }
}

impl <T> Clone for JoinHandle<T> {
    fn clone(&self) -> Self {
        JoinHandle(self.0.clone())
    }
}

impl <T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.take_value() {
            Some(rv) => Poll::Ready(rv),
            None => Poll::Pending,
        }
    }
}
