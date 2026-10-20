use std::{
    collections::VecDeque,
    sync::{self, Arc, Mutex},
};
pub struct TaskQueue<T> {
    /// The tasks in the queue
    tasks: VecDeque<T>,
    /// Stop execution
    stop: bool,
    shutdown_if_empty: bool,
}
impl<T> std::default::Default for TaskQueue<T> {
    fn default() -> Self {
        Self::empty()
    }
}
impl<T> TaskQueue<T> {
    const fn empty() -> Self {
        Self {
            tasks: VecDeque::new(),
            stop: false,
            shutdown_if_empty: false,
        }
    }
    fn push_back(&mut self, task: T) {
        self.tasks.push_back(task);
    }
    fn pop_front(&mut self) -> Option<T> {
        self.tasks.pop_front()
    }
    fn has_data(&self) -> bool {
        !self.tasks.is_empty()
            || self.stop
            || (self.tasks.is_empty() && self.shutdown_if_empty)
    }
}
pub struct ThreadData<T> {
    task: Mutex<TaskQueue<T>>,
    condvar: sync::Condvar,
}
impl<T> std::default::Default for ThreadData<T> {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{error:?}")]
pub struct MyPoisonError<'a, T> {
    error: std::sync::PoisonError<std::sync::MutexGuard<'a, TaskQueue<T>>>,
    data: T,
}
impl<T> ThreadData<T> {
    const fn empty() -> Self {
        Self {
            task: Mutex::new(TaskQueue::empty()),
            condvar: sync::Condvar::new(),
        }
    }
    fn push_task(&self, task: T) -> Result<(), MyPoisonError<'_, T>> {
        let mut task_queue = match self.task.lock() {
            Ok(guard) => guard,
            Err(e) => {
                std::hint::cold_path();
                return Err(MyPoisonError {
                    data: task,
                    error: e,
                });
            }
        };
        task_queue.push_back(task);
        self.set_condvar();
        Ok(())
    }
    fn is_poisoned(&self) -> bool {
        self.task.is_poisoned()
    }
    fn clear_poison_by_clearing_tasks(&self) {
        let Err(mut x) = self.task.lock() else {
            return;
        };
        **x.get_mut() = TaskQueue::empty();
        self.task.clear_poison();
    }
    fn stop(&self) {
        match self.task.lock() {
            Err(x) => {
                tracing::error!("ThreadData encountered poisoned mutex {x:?}");
                return;
            }
            Ok(guard) => guard,
        }
        .stop = true;
        self.set_condvar();
    }
    fn shutdown_if_empty(&self) {
        match self.task.lock() {
            Err(x) => {
                tracing::error!("ThreadData encountered poisoned mutex {x:?}");
                return;
            }
            Ok(guard) => guard,
        }
        .shutdown_if_empty = true;
        self.set_condvar();
    }
    fn set_condvar(&self) {
        self.condvar.notify_one();
    }

    /// Return `None` if the thread is requested to stop.
    /// # Panics
    /// if the inner mutex is poisoned. Since that likely means the main thread panic'd panicing
    /// here is acceptable.
    fn wait_for_value(&self) -> Option<T> {
        let guard = match self.task.lock() {
            Err(x) => {
                panic!("Render thread encountered poisoned mutex {x:?}");
            }
            Ok(guard) => guard,
        };
        match self
            .condvar
            .wait_while(guard, |queue| !std::hint::likely(queue.has_data()))
        {
            Ok(mut guard) => {
                if guard.stop {
                    return None;
                }
                if guard.tasks.is_empty() && guard.shutdown_if_empty {
                    return None;
                }
                // If task has data, we can safely unwrap here.
                Some(unsafe { guard.tasks.pop_front().unwrap_unchecked() })
            }
            Err(x) => {
                panic!(
                    "Render thread encountered poisoned mutex during condvar wait {x:?}"
                );
            }
        }
    }
}

pub struct RenderThread<T>
where
    T: Sync + Send + 'static,
{
    tasks: Arc<ThreadData<T>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl<T> std::default::Default for RenderThread<T>
where
    T: Sync + Send + 'static,
{
    fn default() -> Self {
        Self::empty()
    }
}

impl<T> RenderThread<T>
where
    T: Sync + Send + 'static,
{
    #[must_use]
    pub fn empty() -> Self {
        Self {
            tasks: Arc::new(ThreadData::empty()),
            thread: None,
        }
    }
    /// # Errors
    /// Mutex error
    pub fn push_task(&self, task: T) -> Result<(), MyPoisonError<'_, T>> {
        self.tasks.push_task(task)
    }
    /// Returns the panic payload.
    pub fn clear_poison_by_clearing_tasks(
        &mut self,
    ) -> Option<Box<dyn std::any::Any + Send + 'static>> {
        let poisoned = self.tasks.is_poisoned();
        let Some(thread) = self.thread.take() else {
            if poisoned {
                self.tasks.clear_poison_by_clearing_tasks();
            }
            return None;
        };
        if !poisoned {
            //make sure the thread is stopped
            self.tasks.stop();
        }
        thread.join().err()
    }
    pub fn start<F: FnMut(T) + Send + 'static>(&mut self, f: F) {
        let tasks = Arc::downgrade(&self.tasks);
        self.thread = Some(std::thread::spawn(move || {
            Self::run_loop(f, &tasks);
        }));
    }
    fn run_loop<F: FnMut(T) + Send + 'static>(
        mut f: F,
        tasks: &sync::Weak<ThreadData<T>>,
    ) {
        'l: loop {
            let Some(tasks) = tasks.upgrade() else {
                break 'l;
            };
            let Some(task) = tasks.wait_for_value() else {
                break 'l;
            };
            f(task);
        }
    }
    fn shutdown_wait_for_all(
        &mut self,
    ) -> Result<(), Box<dyn std::any::Any + Send + 'static>> {
        if let Some(thread) = self.thread.take() {
            let is_poisoned = self.tasks.is_poisoned();
            if !is_poisoned {
                self.tasks.shutdown_if_empty();
            }
            thread.join()
        } else {
            Ok(())
        }
    }
    fn shutdown_once_current_task_is_done(
        &mut self,
    ) -> Result<(), Box<dyn std::any::Any + Send + 'static>> {
        if let Some(thread) = self.thread.take() {
            let is_poisoned = self.tasks.is_poisoned();
            if !is_poisoned {
                self.tasks.stop();
            }
            thread.join()
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn basic_functionality() {
        let mut thread = RenderThread::default();
        let v = vec![15, 24, 33, 42, 51];
        let v_out = Arc::new(Mutex::new(Vec::new()));
        let r = v_out.clone();
        thread.start(move |x: i32| {
            r.lock().unwrap().push(x);
        });
        for &i in &v {
            thread.push_task(i).unwrap();
        }
        thread.shutdown_wait_for_all().unwrap();

        let v_out = v_out.lock().unwrap().clone();
        assert_eq!(v, v_out);
    }

    #[test]
    fn shutdown_wait_for_all() {
        let mut thread = RenderThread::default();
        let v_out = Arc::new(Mutex::new(Vec::new()));
        let r = v_out.clone();
        thread.start(move |x: i32| {
            r.lock().unwrap().push(x);
        });
        for i in 0..10000 {
            let _ = thread.push_task(i);
        }
        thread.shutdown_wait_for_all().unwrap();

        let v_out = v_out.lock().unwrap().clone();
        assert_eq!(v_out.len(), 10000);
    }
    #[test]
    // If this starts failing raise the TASK_COUNT or increase the sleep duration.
    fn shutdown_immediate() {
        const TASK_COUNT: usize = 100_000;
        let mut thread = RenderThread::default();
        let v_out = Arc::new(Mutex::new(Vec::new()));
        let r = v_out.clone();
        thread.start(move |x| {
            r.lock().unwrap().push(x);
            std::thread::sleep(std::time::Duration::from_millis(100));
        });

        for i in 0..TASK_COUNT {
            let _ = thread.push_task(i);
        }
        thread.shutdown_once_current_task_is_done().unwrap();

        let v_out = v_out.lock().unwrap().clone();
        assert!(v_out.len() < TASK_COUNT);
    }
}
