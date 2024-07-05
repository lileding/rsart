use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::collections::{VecDeque, HashMap};
use std::task::{Context, Wake, Poll};

use super::task::{Task, Waker};

use log::{debug, trace};

pub(crate) struct Scheduler {
    current_taskid: AtomicUsize,
    active_tasks: Mutex<VecDeque<Task>>,
    pending_tasks: Mutex<HashMap<usize, Arc<Waker>>>,
}

impl Scheduler {
    pub(crate) fn new() -> Self {
        Scheduler {
            current_taskid: AtomicUsize::new(0),
            active_tasks: Mutex::new(VecDeque::new()),
            pending_tasks: Mutex::new(HashMap::new()),
        }
    }

    // return true if both active and pending queue are empty
    pub(crate) fn schedule(&self) -> bool {
        trace!(
            "[Scheduler] enter, active_tasks={:?} pending_task={:?}",
            self.active_tasks.lock().unwrap().iter().map(|t| t.id()).collect::<Vec<usize>>(),
            self.pending_tasks.lock().unwrap().keys().collect::<Vec<&usize>>(),
        );
        loop {
            let mut lock = self.active_tasks.lock().unwrap();
            let mut task = match lock.pop_front() {
                Some(task) => task,
                None => break,
            };
            drop(lock);
            let task_waker = Waker::new();
            let waker = Arc::clone(&task_waker).into();
            let mut cx = Context::from_waker(&waker);
            let tid = task.id();
            trace!("[Scheduler] >>> enter task context <<<, tid={}", tid);
            self.current_taskid.store(task.id(), Ordering::Release);
            match task.poll(&mut cx) {
                Poll::Ready(_)=> {
                    trace!("[Scheduler] task done, tid={}", task.id());
                },
                Poll::Pending => {
                    trace!("[Scheduler] task pending, tid={}", task.id());
                    task_waker.park(task);
                }
            }
            self.current_taskid.store(0, Ordering::Release);
            trace!("[Scheduler] <<< exit task context >>>, tid={}", tid);
        }
        trace!(
            "[Scheduler] exit, active_tasks={:?} pending_task={:?}",
            self.active_tasks.lock().unwrap().iter().map(|t| t.id() ).collect::<Vec<usize>>(),
            self.pending_tasks.lock().unwrap().keys().collect::<Vec<&usize>>(),
        );
        self.pending_tasks.lock().unwrap().is_empty()
    }

    pub(crate) fn current_taskid(&self) -> usize {
        self.current_taskid.load(Ordering::Acquire)
    }

    pub(crate) fn activate(&self, task: Task) {
        debug!("[Scheduler] activate task, tid={}", task.id());
        self.pending_tasks.lock().unwrap().remove(&task.id());
        self.active_tasks.lock().unwrap().push_back(task);
    }

    pub(crate) fn wake_by_id(&self, taskid: usize) {
        debug!("[Scheduler] wake_by_id, tid={}", taskid);
        let mut lock = self.pending_tasks.lock().unwrap();
        let waker = lock.remove(&taskid).take();
        drop(lock);
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    pub(crate) fn park(&self, taskid: usize, waker: Arc<Waker>) {
        self.pending_tasks.lock().unwrap().insert(taskid, waker);
    }
}
