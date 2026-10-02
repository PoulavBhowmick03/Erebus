use std::sync::Mutex;

use erebus_journal::{Boundary, FaultHook, Step};

pub struct SweepHook {
    fail_at: usize,
    steps: Mutex<Vec<Step>>,
}

impl SweepHook {
    pub fn new(fail_at: usize) -> Self {
        Self {
            fail_at,
            steps: Mutex::new(Vec::new()),
        }
    }

    pub fn count(&self) -> usize {
        self.steps.lock().unwrap().len()
    }
}

impl FaultHook for SweepHook {
    fn after(&self, boundary: Boundary<'_>) -> std::io::Result<()> {
        let mut steps = self.steps.lock().unwrap();
        steps.push(boundary.step);
        if steps.len() == self.fail_at {
            Err(std::io::Error::other("injected durable boundary failure"))
        } else {
            Ok(())
        }
    }
}
