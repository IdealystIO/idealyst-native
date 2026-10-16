//! The per-thread table behind Android's [`crate::thread_local!`].
//!
//! Each [`Key`] gets a process-wide index the first time any thread
//! touches it; each thread keeps a vector of boxed values by index. std
//! stores the vector behind two thread-locals of its own — so two pthread
//! keys for the whole process:
//!
//! - `TABLE`, a destructor-free pointer to this thread's table, which
//!   stays readable while values are being destroyed (a value's `Drop`
//!   may touch another key, as with std thread-locals);
//! - `GUARD`, whose destructor runs at thread exit and drops the values,
//!   newest first, then frees the table.
//!
//! A boxed value never moves, so the `&T` handed to [`Key::with`] stays
//! valid while other keys initialise and the vector grows.

use std::cell::{Cell, RefCell, UnsafeCell};
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};

/// No index assigned yet.
const UNASSIGNED: usize = usize::MAX;

/// The next index a key takes.
static NEXT_INDEX: AtomicUsize = AtomicUsize::new(0);

/// A slot whose value is being initialised: touching the key again from
/// its own initialiser is a bug, as it is for std.
const INITIALISING: *mut () = 1 as *mut ();

#[derive(Clone, Copy)]
struct Slot {
    ptr: *mut (),
    drop: unsafe fn(*mut ()),
}

#[derive(Default)]
struct Table {
    slots: UnsafeCell<Vec<Option<Slot>>>,
}

unsafe fn drop_box<T>(ptr: *mut ()) {
    drop(unsafe { Box::from_raw(ptr as *mut T) });
}

std::thread_local! {
    static TABLE: Cell<*mut Table> = const { Cell::new(std::ptr::null_mut()) };
    static GUARD: Guard = const { Guard };
}

/// Drops this thread's values at thread exit.
struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        let Ok(table) = TABLE.try_with(Cell::get) else { return };
        if table.is_null() {
            return;
        }
        // A value's `Drop` may initialise a key again (as std's os-key
        // storage allows); drop those in further rounds, as pthread runs
        // key destructors up to PTHREAD_DESTRUCTOR_ITERATIONS times.
        for _ in 0..4 {
            let slots = std::mem::take(unsafe { &mut *(*table).slots.get() });
            if slots.iter().all(Option::is_none) {
                break;
            }
            for slot in slots.into_iter().rev().flatten() {
                if slot.ptr != INITIALISING {
                    unsafe { (slot.drop)(slot.ptr) };
                }
            }
        }
        let _ = TABLE.try_with(|t| t.set(std::ptr::null_mut()));
        drop(unsafe { Box::from_raw(table) });
    }
}

/// This thread's table, created on first use. `None` once the thread is
/// past the point where std thread-locals can be reached.
fn table() -> Option<*mut Table> {
    TABLE
        .try_with(|t| {
            let mut p = t.get();
            if p.is_null() {
                p = Box::into_raw(Box::<Table>::default());
                t.set(p);
                // Registers the destructor. Already destroyed (a key first
                // touched during thread exit): the table leaks with the
                // thread, as a value std re-initialises then would.
                let _ = GUARD.try_with(|_| ());
            }
            p
        })
        .ok()
}

/// An Android thread-local: what [`crate::thread_local!`] declares there.
/// The same methods as [`std::thread::LocalKey`].
pub struct Key<T: 'static> {
    init: fn() -> T,
    index: AtomicUsize,
}

/// The thread is being torn down past the point where thread-locals can be
/// reached — [`std::thread::AccessError`]'s meaning (std's can't be
/// constructed outside std).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccessError;

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("already destroyed")
    }
}

impl std::error::Error for AccessError {}

/// Puts a slot back to empty if its initialiser panics, so the next
/// access tries again instead of reporting recursion.
struct InitGuard {
    table: *mut Table,
    index: usize,
    armed: bool,
}

impl Drop for InitGuard {
    fn drop(&mut self) {
        if self.armed {
            unsafe { (&mut *(*self.table).slots.get())[self.index] = None };
        }
    }
}

impl<T: 'static> Key<T> {
    #[doc(hidden)]
    pub const fn new(init: fn() -> T) -> Self {
        Key { init, index: AtomicUsize::new(UNASSIGNED) }
    }

    fn index(&self) -> usize {
        let i = self.index.load(Ordering::Acquire);
        if i != UNASSIGNED {
            return i;
        }
        let fresh = NEXT_INDEX.fetch_add(1, Ordering::Relaxed);
        match self.index.compare_exchange(UNASSIGNED, fresh, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => fresh,
            Err(won) => won,
        }
    }

    /// This thread's value, initialised on first use.
    fn value(&'static self) -> Result<*const T, AccessError> {
        let table = table().ok_or(AccessError)?;
        let i = self.index();
        // SAFETY: the table is this thread's; no reference into the vector
        // outlives these short borrows (values live in their own boxes).
        unsafe {
            let slots = &mut *(*table).slots.get();
            if let Some(Some(slot)) = slots.get(i) {
                assert!(
                    slot.ptr != INITIALISING,
                    "a thread-local was accessed from its own initialiser"
                );
                return Ok(slot.ptr as *const T);
            }
            if slots.len() <= i {
                slots.resize(i + 1, None);
            }
            slots[i] = Some(Slot { ptr: INITIALISING, drop: drop_box::<T> });
        }
        let mut guard = InitGuard { table, index: i, armed: true };
        // The initialiser may touch other keys, which may grow the vector.
        let ptr = Box::into_raw(Box::new((self.init)()));
        guard.armed = false;
        unsafe { (&mut *(*table).slots.get())[i] = Some(Slot { ptr: ptr as *mut (), drop: drop_box::<T> }) };
        Ok(ptr)
    }

    /// Run `f` with this thread's value. Panics if the thread is past the
    /// point where thread-locals can be reached, as std's does.
    pub fn with<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.try_with(f)
            .expect("cannot access a Thread Local Storage value during or after destruction")
    }

    /// [`with`](Self::with), or [`AccessError`] instead of panicking.
    pub fn try_with<F, R>(&'static self, f: F) -> Result<R, AccessError>
    where
        F: FnOnce(&T) -> R,
    {
        let p = self.value()?;
        // SAFETY: boxed values are only freed by the guard at thread exit.
        Ok(f(unsafe { &*p }))
    }
}

impl<T: 'static> Key<Cell<T>> {
    pub fn set(&'static self, value: T) {
        self.with(|c| c.set(value))
    }

    pub fn get(&'static self) -> T
    where
        T: Copy,
    {
        self.with(Cell::get)
    }

    pub fn take(&'static self) -> T
    where
        T: Default,
    {
        self.with(Cell::take)
    }

    pub fn replace(&'static self, value: T) -> T {
        self.with(|c| c.replace(value))
    }
}

impl<T: 'static> Key<RefCell<T>> {
    pub fn with_borrow<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.with(|c| f(&c.borrow()))
    }

    pub fn with_borrow_mut<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        self.with(|c| f(&mut c.borrow_mut()))
    }

    pub fn set(&'static self, value: T) {
        self.with(|c| *c.borrow_mut() = value)
    }

    pub fn take(&'static self) -> T
    where
        T: Default,
    {
        self.with(RefCell::take)
    }

    pub fn replace(&'static self, value: T) -> T {
        self.with(|c| c.replace(value))
    }
}

impl<T: 'static> fmt::Debug for Key<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Key").finish_non_exhaustive()
    }
}
