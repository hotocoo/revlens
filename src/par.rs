//! Order-preserving parallel map over a slice with scoped threads (no thread pool dependency).

use std::num::NonZeroUsize;

/// Worker count for `threads` (0 means one per available core).
pub fn workers(threads: usize) -> usize {
    if threads > 0 {
        return threads;
    }
    std::thread::available_parallelism().map_or(1, NonZeroUsize::get)
}

/// `items.iter().map(f).collect()`, split into contiguous chunks across up to `threads` workers.
pub fn map<T: Sync, R: Send>(items: &[T], threads: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let n = workers(threads).min(items.len());
    if n <= 1 {
        return items.iter().map(f).collect();
    }
    let size = items.len().div_ceil(n);
    let f = &f;
    std::thread::scope(|s| {
        let handles: Vec<_> = items
            .chunks(size)
            .map(|chunk| s.spawn(move || chunk.iter().map(f).collect::<Vec<R>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| match h.join() {
                Ok(v) => v,
                Err(panic) => std::panic::resume_unwind(panic),
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_preserves_order_for_any_worker_count() {
        let xs: Vec<u32> = (0..1000).collect();
        let want: Vec<u32> = xs.iter().map(|x| x * 2).collect();
        for t in [0, 1, 3, 64, 5000] {
            assert_eq!(map(&xs, t, |x| x * 2), want, "threads={t}");
        }
        assert!(map(&[] as &[u32], 4, |x| *x).is_empty());
        assert_eq!(workers(5), 5);
        assert!(workers(0) >= 1);
    }
}
