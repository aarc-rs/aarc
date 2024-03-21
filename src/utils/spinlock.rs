use crate::utils::helpers::{alloc_box_ptr, dealloc_box_ptr};
use std::ptr::null_mut;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::SeqCst;
use std::{hint, ptr};

const SPIN_FLAG: *mut u8 = usize::MAX as *mut u8;

pub struct Spinlock<T>(AtomicPtr<T>);

impl<T> Spinlock<T> {
    pub const fn new() -> Self {
        Self(AtomicPtr::new(null_mut()))
    }
}

impl<T: Default> Spinlock<T> {
    pub fn with_take_mut<R, F: Fn(&mut T) -> R>(&self, f: F) -> R {
        loop {
            let mut ptr = self.0.swap(SPIN_FLAG as *mut T, SeqCst);
            if !ptr::eq(ptr, SPIN_FLAG as *mut T) {
                if ptr.is_null() {
                    ptr = alloc_box_ptr(T::default());
                }
                let result = unsafe { f(&mut *ptr) };
                self.0.store(ptr, SeqCst);
                return result;
            } else {
                hint::spin_loop();
            }
        }
    }
}

impl<T> Drop for Spinlock<T> {
    fn drop(&mut self) {
        unsafe {
            dealloc_box_ptr(self.0.load(SeqCst));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::spinlock::Spinlock;
    use std::cell::Cell;
    use std::thread;

    #[test]
    fn test_with_take_mut() {
        const THREADS_COUNT: usize = 3;

        let v = Spinlock::<Cell<usize>>::new();
        thread::scope(|s| {
            for _ in 0..THREADS_COUNT {
                s.spawn(|| {
                    v.with_take_mut(|x| {
                        x.set(x.get() + 1);
                    })
                });
            }
        });
        v.with_take_mut(|x| {
            assert_eq!(x.get(), THREADS_COUNT);
        });
    }
}
