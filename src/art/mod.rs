use std::io::Result;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::future::Future;
use std::task::{Context, Poll, Wake, Waker};
use std::pin::Pin;
use std::time::Duration;

use self::executor::Executor;
use self::scheduler::Scheduler;
use self::timer::TimerFd;

use log::debug;

mod timer;
mod task;
mod executor;
mod scheduler;

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

    fn block_on<F>(&self, task: F) -> F::Output where
        F: Future + Send + 'static, F::Output: Send + 'static {
        let rv = self.spawn(task);

        loop {
            let scheduler_complete = self.scheduler.schedule();
            let executor_complete = self.executor.execute().unwrap();
            if scheduler_complete && executor_complete {
                break;
            }
        }

        let mut lock = rv.0.lock().unwrap();
        lock.take().unwrap()
    }

    pub fn spawn<F>(&self, task: F) -> JoinHandle<F::Output> where
        F: Future + Send + 'static, F::Output: Send + 'static {
        let tid = self.next_taskid.fetch_add(1, Ordering::AcqRel);
        let wrapper = TaskWrapper::new(tid, self.scheduler.current_taskid(), task);
        let handle = wrapper.join();
        let task = task::Task::new(tid, wrapper);
        self.scheduler.activate(task);
        debug!("[Spawner] new task, id={}", tid);
        handle
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

pub fn block_on<F>(task: F) -> F::Output where
    F: Future + Send + 'static, F::Output: Send + 'static {
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
    task: Mutex<Pin<Box<F>>>,
    result: Arc<Mutex<Option<F::Output>>>,
}

impl <F> TaskWrapper<F> where F: Future + Send + 'static, F::Output: Send + 'static {
    fn new(id: usize, parent_id: usize, task: F) -> Self {
        TaskWrapper {
            id,
            parent_id,
            task: Mutex::new(Box::pin(task)),
            result: Arc::new(Mutex::new(None)),
        }
    }

    fn join(&self) -> JoinHandle<F::Output> {
        JoinHandle(Arc::clone(&self.result))
    }
}

impl <F> Future for TaskWrapper<F> where
F: Future + Send + 'static, F::Output: Send + 'static {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.task.lock().unwrap().as_mut().poll(cx) {
            Poll::Ready(rv) => {
                debug!("[TaskWrapper] task ready and wake parent, tid={} parent={}", self.id, self.parent_id);
                self.result.lock().unwrap().replace(rv);
                Runtime::current().scheduler.wake_by_id(self.parent_id);
                Poll::Ready(())
            },
            Poll::Pending => {
                debug!("[TaskWrapper] task not ready, tid={}", self.id);
                Poll::Pending
            },
        }
    }
}

pub struct JoinHandle<T>(Arc<Mutex<Option<T>>>);

impl <T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        //debug!("[JoinHandle] poll, tid={}", self.0);
        match self.0.lock().unwrap().take() {
            Some(rv) => {
                //debug!("[JoinHandle] ready, tid={}", self.0);
                //cx.waker().wake_by_ref();
                Poll::Ready(rv)
            },
            None => {
                //debug!("[JoinHandle] not ready, tid={}", self.0);
                //cx.waker().wake_by_ref();
                Poll::Pending
            },
        }
    }
}
