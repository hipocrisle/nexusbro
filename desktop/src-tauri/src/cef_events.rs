//! Blocking command relay and coalescing primitive, independent of CEF.
use std::sync::{atomic::{AtomicBool, Ordering}, mpsc::{channel, Receiver, Sender}, Arc};

#[derive(Default)]
pub(crate) struct WakeGate(AtomicBool);
impl WakeGate {
    pub(crate) const fn new() -> Self { Self(AtomicBool::new(false)) }
    /// Claim one UI task. Queue the event before calling this method.
    pub(crate) fn claim(&self) -> bool { !self.0.swap(true, Ordering::AcqRel) }
    /// Clear before draining, or after a failed post, so arrivals can retry.
    pub(crate) fn release(&self) { self.0.store(false, Ordering::Release); }
}

pub(crate) struct Relay<T> {
    stop: Arc<AtomicBool>,
    unblock: Sender<T>,
    stop_event: Option<T>,
}
impl<T> Drop for Relay<T> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // No join on the UI thread, and no periodic timeout wakes on the worker.
        if let Some(event) = self.stop_event.take() { let _ = self.unblock.send(event); }
    }
}

pub(crate) fn relay<T: Send + 'static>(
    tx: &Sender<T>, incoming: Receiver<T>, stop_event: T, notify: fn(),
) -> std::io::Result<(Receiver<T>, Relay<T>)> {
    let (ready_tx, ready_rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    std::thread::Builder::new().name("cef-events".into()).stack_size(256 * 1024).spawn(move || {
        'relay: while let Ok(first) = incoming.recv() {
            // Move a bounded burst before posting, rather than posting once for
            // every title/loading event of a page opening many resources.
            for event in std::iter::once(first).chain(incoming.try_iter().take(127)) {
                if worker_stop.load(Ordering::Acquire) || ready_tx.send(event).is_err() { break 'relay; }
            }
            notify();
        }
    })?;
    Ok((ready_rx, Relay { stop, unblock: tx.clone(), stop_event: Some(stop_event) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Barrier, thread, time::Duration};
    fn notify() {}

    #[test]
    fn burst_preserves_fifo_without_a_periodic_poll() {
        let (tx, incoming) = channel();
        let (ready, guard) = relay(&tx, incoming, usize::MAX, notify).unwrap();
        for value in 0..1000 { tx.send(value).unwrap(); }
        for expected in 0..1000 {
            assert_eq!(ready.recv_timeout(Duration::from_secs(3)).unwrap(), expected);
        }
        drop(guard);
        assert!(ready.recv_timeout(Duration::from_secs(3)).is_err());
    }

    #[test]
    fn dropping_owner_wakes_blocked_worker_without_forwarding_stop_event() {
        let (tx, incoming) = channel();
        let (ready, guard) = relay(&tx, incoming, "stop", notify).unwrap();
        // Keep a sender alive: channel disconnection alone cannot stop recv.
        drop(guard);
        assert!(ready.recv_timeout(Duration::from_secs(3)).is_err());
        // Receiver captures may be dropped just after ready_tx; wait for the
        // worker's input to disconnect rather than depending on drop order.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while tx.send("after-stop").is_ok() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(tx.send("after-exit").is_err());
    }

    #[test]
    fn destroyed_ui_receiver_causes_worker_to_exit() {
        let (tx, incoming) = channel();
        let (ready, guard) = relay(&tx, incoming, "stop", notify).unwrap();
        drop(ready);
        tx.send("wake").unwrap();
        // On failure ready_tx is dropped; worker consumes no further events.
        // Disconnection proves the worker dropped incoming.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while tx.send("probe").is_ok() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(tx.send("final").is_err());
        drop(guard);
    }

    #[test]
    fn concurrent_producers_claim_exactly_one_wake_until_release() {
        let gate = Arc::new(WakeGate::default());
        let barrier = Arc::new(Barrier::new(8));
        let workers: Vec<_> = (0..8).map(|_| {
            let gate = gate.clone(); let barrier = barrier.clone();
            thread::spawn(move || { barrier.wait(); gate.claim() as usize })
        }).collect();
        assert_eq!(workers.into_iter().map(|w| w.join().unwrap()).sum::<usize>(), 1);
        // Failure to post or nested UI borrow retries must not leave it stuck.
        gate.release();
        assert!(gate.claim());
        gate.release();
        assert!(gate.claim());
    }

    #[test]
    fn clear_before_drain_allows_arrival_during_tick_to_schedule_next_tick() {
        let gate = WakeGate::default();
        assert!(gate.claim()); // first queued event
        assert!(!gate.claim()); // same pending task covers the next arrival
        gate.release(); // UI task begins, before examining any receiver
        assert!(gate.claim()); // event arrives while that task is draining
        assert!(!gate.claim()); // further arrivals coalesce onto its successor
        gate.release();
        assert!(gate.claim());
    }
}
