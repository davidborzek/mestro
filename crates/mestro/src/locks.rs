//! Lock helpers for the shared-state mutexes. The guarded values
//! are written wholesale, never incrementally, so a mutex left
//! poisoned by a panicking thread still holds usable data —
//! poisoning must not take the whole pipeline down with it.
//! (unwrap/expect are denied crate-wide, see [lints] in
//! Cargo.toml.)

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn read<T>(rwlock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    rwlock.read().unwrap_or_else(PoisonError::into_inner)
}

pub fn write<T>(rwlock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    rwlock.write().unwrap_or_else(PoisonError::into_inner)
}
