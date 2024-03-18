use crate::utils::helpers::{alloc_box_ptr, dealloc_box_ptr};
use std::array;
use std::ptr::null_mut;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

/// A specialized linked list; each node contains an array of N items.
pub(crate) struct UnrolledLinkedList<T: Default, const N: usize> {
    head: ULLNode<T, N>,
    nodes_count: AtomicUsize,
}

impl<T: Default, const N: usize> UnrolledLinkedList<T, N> {
    pub(crate) fn iter(&self, order: Ordering) -> impl Iterator<Item = &'_ T> {
        self.head.iter(order)
    }

    pub(crate) fn get_nodes_count(&self) -> usize {
        self.nodes_count.load(SeqCst)
    }

    #[allow(dead_code)]
    pub(crate) fn get_at_index(&self, index: usize) -> &T {
        let mut curr = &self.head;
        unsafe {
            let mut i = index;
            while i >= N {
                curr = &*curr.next.load(SeqCst);
                i -= N;
            }
            &curr.items[i]
        }
    }

    pub(crate) fn try_for_each_with_append<F: Fn(&T) -> bool>(&self, f: F) -> &T {
        let mut curr = &self.head;
        loop {
            for item in curr.items.iter() {
                if f(item) {
                    return item;
                }
            }
            let mut next = curr.next.load(SeqCst);
            if next.is_null() {
                let new_node = alloc_box_ptr(ULLNode::default());
                match curr
                    .next
                    .compare_exchange(null_mut(), new_node, SeqCst, SeqCst)
                {
                    Ok(_) => {
                        next = new_node;
                        self.nodes_count.fetch_add(1, SeqCst);
                    }
                    Err(actual) => unsafe {
                        dealloc_box_ptr(new_node);
                        next = actual;
                    },
                }
            }
            unsafe {
                curr = &*next;
            }
        }
    }
}

impl<T: Default, const N: usize> Default for UnrolledLinkedList<T, N> {
    fn default() -> Self {
        Self {
            head: ULLNode::default(),
            nodes_count: AtomicUsize::new(1),
        }
    }
}
unsafe impl<T: Default + Send + Sync, const N: usize> Send for UnrolledLinkedList<T, N> {}

unsafe impl<T: Default + Send + Sync, const N: usize> Sync for UnrolledLinkedList<T, N> {}

struct ULLNode<T, const N: usize> {
    items: [T; N],
    next: AtomicPtr<ULLNode<T, N>>,
}

impl<T, const N: usize> ULLNode<T, N> {
    fn iter(&self, order: Ordering) -> impl Iterator<Item = &'_ T> {
        let mut iters = vec![self.items.iter()];
        let mut curr = self.next.load(order);
        while !curr.is_null() {
            unsafe {
                iters.push((*curr).items.iter());
                curr = (*curr).next.load(order);
            }
        }
        iters.into_iter().flatten()
    }
}

impl<T: Default, const N: usize> Default for ULLNode<T, N> {
    fn default() -> Self {
        Self {
            items: array::from_fn(|_| T::default()),
            next: AtomicPtr::default(),
        }
    }
}

impl<T, const N: usize> Drop for ULLNode<T, N> {
    fn drop(&mut self) {
        let next = self.next.load(SeqCst);
        if !next.is_null() {
            unsafe {
                dealloc_box_ptr(next);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::unrolled_linked_list::UnrolledLinkedList;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering::SeqCst;
    use std::thread;

    #[test]
    fn test_concurrent_iter_and_append() {
        const ITEMS_PER_NODE: usize = 2;
        const THREADS_COUNT: usize = ITEMS_PER_NODE * 2 + 1;

        let ull: UnrolledLinkedList<AtomicBool, ITEMS_PER_NODE> = UnrolledLinkedList::default();
        thread::scope(|s| {
            for _ in 0..THREADS_COUNT {
                s.spawn(|| {
                    let result = ull.try_for_each_with_append(|b| {
                        b.compare_exchange(false, true, SeqCst, SeqCst).is_ok()
                    });
                    assert!(result.load(SeqCst));
                });
            }
        });
        for i in 0..THREADS_COUNT {
            assert_eq!(ull.get_at_index(i).load(SeqCst), i < THREADS_COUNT);
        }
    }
}
