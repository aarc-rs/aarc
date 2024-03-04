use aarc::{AtomicArc, AtomicWeak};
use rand::random;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::Arc;
use std::{array, thread};

#[test]
fn test_stack() {
    const THREADS_COUNT: usize = 5;
    const ITERS_PER_THREAD: usize = 10;

    struct StackNode {
        val: usize,
        next: Option<Arc<Self>>,
    }

    #[derive(Default)]
    struct Stack {
        top: AtomicArc<StackNode>,
    }

    impl Stack {
        fn push(&self, val: usize) {
            let mut top = self.top.load(SeqCst);
            loop {
                let new_node = Arc::new(StackNode { val, next: top });
                match self.top.compare_exchange(
                    new_node.next.as_ref(),
                    Some(&new_node),
                    SeqCst,
                    SeqCst,
                ) {
                    Ok(_) => break,
                    Err(before) => top = before,
                }
            }
        }
        fn pop(&self) -> Arc<StackNode> {
            let mut top = self.top.load(SeqCst).unwrap();
            loop {
                match self
                    .top
                    .compare_exchange(Some(&top), top.next.as_ref(), SeqCst, SeqCst)
                {
                    Ok(_) => return top,
                    Err(before) => top = before.unwrap(),
                }
            }
        }
    }

    let stack = Stack::default();

    thread::scope(|s| {
        for _ in 0..THREADS_COUNT {
            s.spawn(|| {
                for i in 0..ITERS_PER_THREAD {
                    stack.push(i)
                }
            });
        }
    });

    let val_counts: [AtomicUsize; ITERS_PER_THREAD] = array::from_fn(|_| AtomicUsize::default());
    thread::scope(|s| {
        for _ in 0..THREADS_COUNT {
            s.spawn(|| {
                for _ in 0..ITERS_PER_THREAD {
                    let node = stack.pop();
                    val_counts[node.val].fetch_add(1, SeqCst);
                }
            });
        }
    });

    // Verify that no nodes were lost.
    for count in val_counts.iter() {
        assert_eq!(count.load(SeqCst), THREADS_COUNT);
    }
}

#[test]
fn test_sorted_doubly_linked_list() {
    const THREADS_COUNT: usize = 5;
    const ITERS_PER_THREAD: usize = 10;

    #[derive(Default)]
    struct ListNode {
        val: usize,
        prev: AtomicWeak<Self>,
        next: AtomicArc<Self>,
    }

    struct LinkedList {
        head: Arc<ListNode>,
    }

    impl LinkedList {
        fn insert_sorted(&self, val: usize) {
            let mut curr_node = self.head.clone();
            let mut next = curr_node.next.load(SeqCst);
            loop {
                if next.is_none() || val < next.as_ref().unwrap().val {
                    let new = Arc::new(ListNode {
                        val,
                        prev: AtomicWeak::from(Arc::downgrade(&curr_node)),
                        next: AtomicArc::from(next.clone()),
                    });
                    match curr_node
                        .next
                        .compare_exchange(next.as_ref(), Some(&new), SeqCst, SeqCst)
                    {
                        Ok(_) => {
                            if let Some(next_node) = next {
                                // This is technically incorrect, as some other node could've
                                // been inserted, but we don't care for the purposes of this test.
                                next_node.prev.store(Some(&Arc::downgrade(&new)), SeqCst);
                            }
                            break;
                        }
                        Err(actual_next) => next = actual_next,
                    }
                } else {
                    curr_node = next.unwrap();
                    next = curr_node.next.load(SeqCst);
                }
            }
        }
    }

    let list = LinkedList {
        head: Arc::new(ListNode::default()),
    };

    thread::scope(|s| {
        for _ in 0..THREADS_COUNT {
            s.spawn(|| {
                for _ in 0..ITERS_PER_THREAD {
                    list.insert_sorted(random::<usize>());
                }
            });
        }
    });

    // Verify that no nodes were lost and that the list is in sorted order.
    let mut i = 0;
    let mut curr_node = list.head.clone();
    while let Some(next_node) = curr_node.next.load(SeqCst) {
        assert!(curr_node.val <= next_node.val);
        curr_node = next_node;
        i += 1;
    }
    assert_eq!(THREADS_COUNT * ITERS_PER_THREAD, i);
    // Iterate in reverse order using the weak ptrs (they may skip over some nodes but that's ok).
    while let Some(prev_weak) = curr_node.prev.load(SeqCst) {
        let prev_node = prev_weak.upgrade().unwrap();
        assert!(curr_node.val >= prev_node.val);
        curr_node = prev_node;
    }
}
