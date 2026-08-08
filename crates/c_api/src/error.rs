//! ABI error mapping and panic containment.

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::ContinuityEngineStatus;

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

pub(crate) struct AbiError {
    pub(crate) status: ContinuityEngineStatus,
    pub(crate) message: String,
}

impl AbiError {
    pub(crate) fn new(status: ContinuityEngineStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

pub(crate) fn set_last_error(message: impl Into<String>) {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = message.into());
}

pub(crate) fn last_error() -> String {
    LAST_ERROR.with(|slot| slot.borrow().clone())
}

pub(crate) fn boundary(operation: impl FnOnce() -> Result<(), AbiError>) -> ContinuityEngineStatus {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(())) => {
            set_last_error("");
            ContinuityEngineStatus::Ok
        }
        Ok(Err(error)) => {
            set_last_error(error.message);
            error.status
        }
        Err(_) => {
            set_last_error("Rust panic contained at Continuity C ABI boundary");
            ContinuityEngineStatus::Panic
        }
    }
}
