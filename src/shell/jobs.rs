use std::sync::{Arc, Mutex};
use std::thread;

use delegate_shell::interpreter::value::Value;
use delegate_shell::Runtime;

#[derive(Clone)]
pub struct Job {
    pub id: usize,
    pub name: String,
    pub status: Arc<Mutex<JobStatus>>,
    pub result: Arc<Mutex<Option<Result<Value, String>>>>,
}

#[derive(Clone, PartialEq)]
pub enum JobStatus {
    Running,
    Done,
    Failed,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Running => write!(f, "running"),
            JobStatus::Done => write!(f, "done"),
            JobStatus::Failed => write!(f, "failed"),
        }
    }
}

pub struct JobManager {
    jobs: Vec<Job>,
    next_id: usize,
}

impl JobManager {
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            next_id: 1,
        }
    }

    /// Spawn a background job that runs a dgsh source string.
    pub fn spawn(&mut self, name: String, source: String) -> usize {
        let id = self.next_id;
        self.next_id += 1;

        let status = Arc::new(Mutex::new(JobStatus::Running));
        let result: Arc<Mutex<Option<Result<Value, String>>>> = Arc::new(Mutex::new(None));

        let status_clone = status.clone();
        let result_clone = result.clone();

        thread::spawn(move || {
            let mut engine = match Runtime::new() {
                Ok(e) => e,
                Err(e) => {
                    *result_clone.lock().unwrap_or_else(|p| p.into_inner()) = Some(Err(e));
                    *status_clone.lock().unwrap_or_else(|p| p.into_inner()) = JobStatus::Failed;
                    return;
                }
            };

            match engine.run_source(&source) {
                Ok(()) => {
                    *result_clone.lock().unwrap_or_else(|p| p.into_inner()) = Some(Ok(Value::void()));
                    *status_clone.lock().unwrap_or_else(|p| p.into_inner()) = JobStatus::Done;
                }
                Err(e) => {
                    *result_clone.lock().unwrap_or_else(|p| p.into_inner()) = Some(Err(e));
                    *status_clone.lock().unwrap_or_else(|p| p.into_inner()) = JobStatus::Failed;
                }
            }
        });

        self.jobs.push(Job {
            id,
            name,
            status,
            result,
        });

        id
    }

    /// List all jobs with their status.
    pub fn list(&self) -> Vec<(usize, String, JobStatus)> {
        self.jobs
            .iter()
            .map(|j| {
                let status = j.status.lock().unwrap_or_else(|p| p.into_inner()).clone();
                (j.id, j.name.clone(), status)
            })
            .collect()
    }

    /// Remove completed jobs and return them.
    pub fn collect_done(&mut self) -> Vec<Job> {
        let mut done = Vec::new();
        self.jobs.retain(|j| {
            let status = j.status.lock().unwrap_or_else(|p| p.into_inner()).clone();
            if status != JobStatus::Running {
                done.push(j.clone());
                false
            } else {
                true
            }
        });
        done
    }
}
