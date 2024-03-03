use aarc::atomics::AtomicArc;
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

    let stack = Stack {
        top: AtomicArc::default(),
    };

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
fn test_sorted_linked_list() {
    const THREADS_COUNT: usize = 5;
    const ITERS_PER_THREAD: usize = 10;

    struct ListNode {
        val: usize,
        next: AtomicArc<ListNode>,
    }

    let head = AtomicArc::new(Some(ListNode {
        val: 0,
        next: AtomicArc::new(None),
    }));

    thread::scope(|s| {
        for _ in 0..THREADS_COUNT {
            s.spawn(|| {
                for _ in 0..ITERS_PER_THREAD {
                    let val = random::<usize>();
                    let mut curr_arc = head.load(SeqCst).unwrap();
                    let mut next = curr_arc.next.load(SeqCst);
                    'inner: loop {
                        if next.is_none() || val < next.as_ref().unwrap().val {
                            let new = Arc::new(ListNode {
                                val,
                                next: AtomicArc::from(next.clone()),
                            });
                            match curr_arc.next.compare_exchange(
                                next.as_ref(),
                                Some(&new),
                                SeqCst,
                                SeqCst,
                            ) {
                                Ok(_) => break 'inner,
                                Err(actual_next) => next = actual_next,
                            }
                        } else {
                            curr_arc = next.unwrap();
                            next = curr_arc.next.load(SeqCst);
                        }
                    }
                }
            });
        }
    });

    // Verify that no nodes were lost and that the list is in sorted order.
    let mut i = 0;
    let mut curr_arc = head.load(SeqCst).unwrap();
    while let Some(next_node) = curr_arc.next.load(SeqCst) {
        assert!(curr_arc.val <= next_node.val);
        curr_arc = next_node;
        i += 1;
    }
    assert_eq!(THREADS_COUNT * ITERS_PER_THREAD, i);
}
