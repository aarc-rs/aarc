use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{Relaxed, SeqCst};

/// A counter that implements the wait-free "increment-if-not-zero" mechanism, as described in:
/// https://dl.acm.org/doi/10.1145/3519939.3523730, section 4.3.
/// For our purposes, we don't need to identify the exact operation which took the counter to zero,
/// so the "help bit" and associated logic is omitted.
///
/// Assumptions:
/// * overflow to the zero bit will never occur.
/// * fetch_sub will never be called after the counter hits zero.
pub(crate) struct StickyCounter(AtomicUsize);

impl StickyCounter {
    const ZERO_BIT: usize = 1 << (usize::BITS - 1);

    pub(crate) fn try_increment(&self) -> Result<usize, ()> {
        let before = self.0.fetch_add(1, SeqCst);
        if Self::is_zero(before) {
            self.0.fetch_sub(1, Relaxed);
            Err(())
        } else if before == 0 {
            // The stored value is zero, but the zero bit is not set, so a decrement is occurring.
            // They will act as if our increment occurred before their decrement, so we should too.
            Ok(1)
        } else {
            Ok(before)
        }
    }

    // Returns the value of the counter before the decrement (like fetch_sub).
    pub(crate) fn decrement(&self) -> usize {
        let before = self.0.fetch_sub(1, SeqCst);
        if before == 1 {
            match self.0.compare_exchange(0, Self::ZERO_BIT, SeqCst, SeqCst) {
                Ok(_) => 1,
                Err(val) => {
                    // Someone else set the zero bit or an increment occurred first.
                    // In the latter case, we simply act as if we never hit zero.
                    if Self::is_zero(val) {
                        0
                    } else {
                        val + 1
                    }
                }
            }
        } else {
            before
        }
    }

    pub(crate) fn load(&self) -> usize {
        let mut val = self.0.load(SeqCst);
        if val == 0 {
            // We are racing with a decrement, so we try to help set the zero bit.
            val = self
                .0
                .compare_exchange(0, Self::ZERO_BIT, SeqCst, SeqCst)
                .unwrap_or_else(|x| x)
        }
        if Self::is_zero(val) {
            0
        } else {
            val
        }
    }

    fn is_zero(val: usize) -> bool {
        (val & Self::ZERO_BIT) != 0
    }
}

impl Default for StickyCounter {
    fn default() -> Self {
        Self(AtomicUsize::new(1))
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::sticky_counter::StickyCounter;
    use std::thread;

    #[test]
    fn test_counter_is_sticky() {
        let x = StickyCounter::default();
        assert!(x.try_increment().is_ok());
        assert_eq!(x.decrement(), 2);
        assert_eq!(x.decrement(), 1); // this decrement should've taken us to zero.
        assert!(x.try_increment().is_err()); // further increments should fail.
        assert!(x.try_increment().is_err());
        assert_eq!(x.load(), 0);
    }

    #[test]
    fn test_counter_near_zero() {
        const TRIALS: usize = 20;
        for i in 0..TRIALS {
            let x = StickyCounter::default();
            let f1 = || {
                let before_sub = x.decrement();
                assert!(before_sub == 1 || before_sub == 2);
                let val = x.load();
                assert!(val == 0 || val == 1 || val == 2);
            };
            let f2 = || {
                if let Ok(before_inc) = x.try_increment() {
                    assert_eq!(before_inc, 1);
                    let before_sub = x.decrement();
                    assert!(before_sub == 1 || before_sub == 2);
                }
            };
            thread::scope(|s| {
                if i > TRIALS / 2 {
                    s.spawn(f1);
                    s.spawn(f2);
                } else {
                    s.spawn(f2);
                    s.spawn(f1);
                }
            });
            assert_eq!(x.load(), 0);
        }
    }
}
