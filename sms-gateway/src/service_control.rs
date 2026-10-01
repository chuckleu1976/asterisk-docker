use std::sync::atomic::{AtomicBool, Ordering};

static SERVICE_RUNNING: AtomicBool = AtomicBool::new(true);

pub fn is_running() -> bool {
    SERVICE_RUNNING.load(Ordering::SeqCst)
}

pub fn start() -> bool {
    SERVICE_RUNNING.swap(true, Ordering::SeqCst)
}

pub fn stop() -> bool {
    SERVICE_RUNNING.swap(false, Ordering::SeqCst)
}
