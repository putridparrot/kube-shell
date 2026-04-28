use std::sync::atomic::{AtomicBool, Ordering};

static FOREGROUND_COMMAND_ACTIVE: AtomicBool = AtomicBool::new(false);
static PENDING_INTERRUPT: AtomicBool = AtomicBool::new(false);

pub fn install_ctrlc_handler() -> Result<(), String> {
	ctrlc::set_handler(|| {
		if FOREGROUND_COMMAND_ACTIVE.load(Ordering::SeqCst) {
			PENDING_INTERRUPT.store(true, Ordering::SeqCst);
		}
	})
	.map_err(|err| format!("Failed to install Ctrl+C handler: {err}"))
}

pub struct ForegroundCommandGuard;

impl ForegroundCommandGuard {
	pub fn new() -> Self {
		FOREGROUND_COMMAND_ACTIVE.store(true, Ordering::SeqCst);
		PENDING_INTERRUPT.store(false, Ordering::SeqCst);
		Self
	}
}

impl Drop for ForegroundCommandGuard {
	fn drop(&mut self) {
		FOREGROUND_COMMAND_ACTIVE.store(false, Ordering::SeqCst);
	}
}

pub fn consume_pending_interrupt() -> bool {
	PENDING_INTERRUPT.swap(false, Ordering::SeqCst)
}
