//! V8 backend GC cells backed by `rusty_v8::cppgc`.
//!
//! A [`V8GcCell`] is a strong cppgc root (`Persistent`) to a heap object
//! allocated on the isolate's `cppgc::Heap`. The wrapped value lives inside a
//! `rusty_v8::cppgc::GcCell` (an `UnsafeCell`), so mutation is only granted
//! with isolate-scoped proof — the execution context. A runtime borrow counter
//! restores the double-borrow checks `RefCell` provides on other engines.

use std::cell::Cell;
use std::ffi::CStr;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use rusty_v8 as v8;
use v8::cppgc::{self, GarbageCollected, GetRustObj};

use crate::v8::{V8Engine, V8Types};
use crate::ExecutionContext;

/// The cppgc heap object backing a [`V8GcCell`].
///
/// `value` uses rusty_v8's `cppgc::GcCell`, whose access methods require
/// isolate-scoped proof. `readers`/`writer` track active borrows so mutable
/// access stays exclusive, matching `RefCell` semantics on the other engines.
pub(crate) struct HeapCell<T> {
    value: cppgc::GcCell<T>,
    readers: Cell<u32>,
    writer: Cell<bool>,
}

// SAFETY: `HeapCell` holds no cppgc `Member`/`WeakMember`/`TracedReference`
// edges of its own: `T` is opaque to the engine and any JS references inside
// it are `v8::Global` handles, which are strong roots. There is nothing to
// visit during marking. `get_name` returns a static string.
unsafe impl<T: 'static> GarbageCollected for HeapCell<T> {
    fn trace(&self, _visitor: &mut cppgc::Visitor) {}

    fn get_name(&self) -> &'static CStr {
        c"js_engine::GcCell"
    }
}

/// A shared, cloneable GC-managed cell: a strong cppgc root to a heap cell.
///
/// The cell is kept alive while any clone exists and is reclaimed by the
/// isolate's cppgc heap once the last root drops.
pub struct V8GcCell<T: 'static>(cppgc::Persistent<HeapCell<T>>);

impl<T: 'static> Clone for V8GcCell<T> {
    fn clone(&self) -> Self {
        // SAFETY: `Persistent::new` creates a second strong root to the same
        // heap cell; the object stays alive while any root is live. Clones
        // are only created on the isolate thread, satisfying the C++ handle
        // construction/destruction thread rule.
        Self(cppgc::Persistent::new(&self.0))
    }
}

impl<T: 'static> V8GcCell<T> {
    /// Allocate a new cell on the engine's isolate cppgc heap.
    pub(crate) fn new(value: T, engine: &V8Engine) -> Self {
        let heap_cell = HeapCell {
            value: cppgc::GcCell::new(value),
            readers: Cell::new(0),
            writer: Cell::new(false),
        };
        Self(engine.allocate_gc_cell(heap_cell))
    }

    /// Immutably borrow the wrapped value.
    ///
    /// The returned guard's lifetime is tied to `&self`, not to the execution
    /// context, so other engine operations remain callable while a borrow is
    /// held. The heap cell is kept alive by this root (`Persistent`) for the
    /// whole borrow, and the borrow counter prevents mutable aliasing.
    pub(crate) fn borrow<'a, 'e>(
        &'a self,
        ec: &'e dyn ExecutionContext<V8Types>,
    ) -> V8GcRef<'a, T> {
        let engine = ec
            .as_any()
            .downcast_ref::<V8Engine>()
            .expect("V8 GcCell borrowed with a non-V8 execution context");
        let heap_cell = self.0.get().expect("V8 GcCell root holds no heap cell");
        if heap_cell.writer.get() {
            panic!("GcCell<T> already mutably borrowed");
        }
        heap_cell.readers.set(heap_cell.readers.get() + 1);
        let value = engine.with_isolate(|isolate| heap_cell.value.get(isolate) as *const T);
        V8GcRef {
            value,
            cell: heap_cell as *const HeapCell<T>,
            _marker: PhantomData,
        }
    }

    /// Mutably borrow the wrapped value.
    pub(crate) fn borrow_mut<'a, 'e>(
        &'a self,
        ec: &'e mut dyn ExecutionContext<V8Types>,
    ) -> V8GcRefMut<'a, T> {
        let engine = ec
            .as_any_mut()
            .downcast_mut::<V8Engine>()
            .expect("V8 GcCell mutably borrowed with a non-V8 execution context");
        let heap_cell = self.0.get().expect("V8 GcCell root holds no heap cell");
        if heap_cell.writer.get() || heap_cell.readers.get() > 0 {
            panic!("GcCell<T> already borrowed");
        }
        heap_cell.writer.set(true);
        let value = engine.with_isolate_mut(|isolate| heap_cell.value.get_mut(isolate) as *mut T);
        V8GcRefMut {
            value,
            cell: heap_cell as *const HeapCell<T>,
            _marker: PhantomData,
        }
    }

    /// Replace the wrapped value.
    pub(crate) fn set<'a, 'e>(&'a self, value: T, ec: &'e mut dyn ExecutionContext<V8Types>) {
        let engine = ec
            .as_any_mut()
            .downcast_mut::<V8Engine>()
            .expect("V8 GcCell set with a non-V8 execution context");
        let heap_cell = self.0.get().expect("V8 GcCell root holds no heap cell");
        if heap_cell.writer.get() || heap_cell.readers.get() > 0 {
            panic!("GcCell<T> already borrowed");
        }
        engine.with_isolate_mut(|isolate| {
            *heap_cell.value.get_mut(isolate) = value;
        });
    }

    /// Compare two cells for pointer equality.
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        self.0.get_rust_obj() == other.0.get_rust_obj()
    }
}

/// Immutable borrow guard for [`V8GcCell`].
pub struct V8GcRef<'a, T> {
    value: *const T,
    cell: *const HeapCell<T>,
    _marker: PhantomData<&'a T>,
}

impl<T> Deref for V8GcRef<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: The root held by the originating `V8GcCell` keeps the heap
        // cell alive for the lifetime of this guard (`'a`), and the borrow
        // counter guarantees no mutable borrow is active.
        unsafe { &*self.value }
    }
}

impl<T> Drop for V8GcRef<'_, T> {
    fn drop(&mut self) {
        // SAFETY: `cell` points into the same heap cell that supplied `value`;
        // it is kept alive by the root for the guard's lifetime.
        unsafe {
            (*self.cell).readers.set((*self.cell).readers.get() - 1);
        }
    }
}

/// Mutable borrow guard for [`V8GcCell`].
pub struct V8GcRefMut<'a, T> {
    value: *mut T,
    cell: *const HeapCell<T>,
    _marker: PhantomData<&'a mut T>,
}

impl<T> Deref for V8GcRefMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: See `V8GcRef::deref`; the borrow counter guarantees this is
        // the only active borrow.
        unsafe { &*self.value }
    }
}

impl<T> DerefMut for V8GcRefMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: The guard is the only mutable borrow (checked at creation);
        // no other accessor holds a reference into this cell.
        unsafe { &mut *self.value }
    }
}

impl<T> Drop for V8GcRefMut<'_, T> {
    fn drop(&mut self) {
        // SAFETY: Same liveness argument as `V8GcRef::drop`.
        unsafe {
            (*self.cell).writer.set(false);
        }
    }
}
