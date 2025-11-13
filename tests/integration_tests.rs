use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;
use std::thread;

use rand::random;

use aarc::{Arc, AtomicArc, CompareExchange, Guard};

fn test_stack(threads_count: usize, iters_per_thread: usize) {
    #[derive(Default)]
    struct StackNode {
        val: usize,
        next: Option<Arc<Self>>,
    }

    #[derive(Default)]
    struct Stack {
        top: AtomicArc<StackNode>,
    }

    unsafe impl Send for Stack {}
    unsafe impl Sync for Stack {}

    impl Stack {
        fn push(&self, val: usize) {
            let mut top = self.top.load();
            loop {
                let next = top.as_ref().map(Arc::from);
                let new_node = Arc::new(StackNode { val, next });
                match self.top.compare_exchange(top.as_ref(), Some(&new_node)) {
                    Ok(_) => break,
                    Err(before) => top = before,
                }
            }
        }
        fn pop(&self) -> Option<Guard<'_, StackNode>> {
            let mut top = self.top.load();
            while let Some(top_node) = top.as_ref() {
                let next = top_node.next.as_ref();
                match self.top.compare_exchange(top.as_ref(), next) {
                    Ok(_) => return top,
                    Err(actual_top) => top = actual_top,
                }
            }
            None
        }
    }

    let stack = Stack::default();

    thread::scope(|s| {
        for _ in 0..threads_count {
            s.spawn(|| {
                for i in 0..iters_per_thread {
                    stack.push(i);
                }
            });
        }
    });

    let val_counts: Vec<AtomicUsize> = (0..iters_per_thread)
        .map(|_| AtomicUsize::default())
        .collect();
    thread::scope(|s| {
        for _ in 0..threads_count {
            s.spawn(|| {
                for _ in 0..iters_per_thread {
                    let node = stack.pop().unwrap();
                    val_counts[node.val].fetch_add(1, Relaxed);
                }
            });
        }
    });

    // Verify that no nodes were lost.
    for count in &val_counts {
        assert_eq!(count.load(Relaxed), threads_count);
    }
}

#[test]
fn test_stack_small() {
    test_stack(5, 25);
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_stack_full() {
    test_stack(8, 500);
}

fn test_sorted_linked_list(threads_count: usize, iters_per_thread: usize) {
    #[derive(Default)]
    struct ListNode {
        val: usize,
        next: AtomicArc<Self>,
    }

    struct LinkedList {
        head: AtomicArc<ListNode>,
    }

    impl LinkedList {
        fn insert_sorted(&self, val: usize) {
            let mut curr_node = self.head.load().unwrap();
            let mut next_node = curr_node.next.load();
            loop {
                if next_node.is_none() || val < next_node.as_ref().unwrap().val {
                    let next = next_node
                        .as_ref()
                        .map_or_else(AtomicArc::default, AtomicArc::from);
                    let new = Arc::new(ListNode { val, next });
                    // let g = Guard::from(&new);
                    match curr_node
                        .next
                        .compare_exchange(next_node.as_ref(), Some(&new))
                    {
                        Ok(()) => break,
                        Err(actual_next) => next_node = actual_next,
                    }
                } else {
                    curr_node = next_node.unwrap();
                    next_node = curr_node.next.load();
                }
            }
        }
    }

    let list = LinkedList {
        head: AtomicArc::new(Some(ListNode::default())),
    };

    thread::scope(|s| {
        for _ in 0..threads_count {
            s.spawn(|| {
                for _ in 0..iters_per_thread {
                    list.insert_sorted(random::<usize>());
                }
            });
        }
    });

    // Verify that no nodes were lost and that the list is in sorted order.
    let mut i = 0;
    let mut curr_node = list.head.load().unwrap();
    loop {
        let next = curr_node.next.load();
        if let Some(next_node) = next {
            assert!(curr_node.val <= next_node.val);
            curr_node = next_node;
            i += 1;
        } else {
            break;
        }
    }
    assert_eq!(threads_count * iters_per_thread, i);
}

#[test]
fn test_sorted_linked_list_small() {
    test_sorted_linked_list(5, 25);
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_sorted_linked_list_full() {
    test_sorted_linked_list(8, 500);
}
