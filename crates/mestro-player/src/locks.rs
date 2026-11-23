//! Poison-tolerant lock helper, the same discipline as
//! mestro-os's `locks` (this crate depends on no sibling
//! crate): the guarded values are written wholesale, never
//! incrementally, so a mutex left poisoned by a panicking thread
//! still holds usable data.

use std::sync::{Mutex, MutexGuard, PoisonError};

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
