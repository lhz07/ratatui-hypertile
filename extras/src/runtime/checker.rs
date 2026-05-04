use std::time::Duration;

use tokio::time::Interval;

/// Checker is an activation-gated periodic trigger designed for use as a select branch.
/// It coalesces arbitrarily frequent activations into a bounded-rate tick stream:
///
/// You can configure it with a fixed interval (based on tokio::time::Interval).
///
/// Calling activate() arms the checker (it is level-triggered and does not accumulate counts).
///
/// wait() behaves as a gated tick:
///
/// - If the checker is armed, wait() awaits the next interval tick, then
///   automatically disarms itself.
/// - If it is not armed, wait() never completes.
///
/// As a result, repeated activations faster than the interval are filtered:
/// they are collapsed into a single pending check and wait() can complete
/// at most once per interval.
/// Conversely, if activations are sparse (or wait() has not been polled
/// for longer than one interval), the next wait() will complete
/// immediately (if the checker is armed), making this branch very
/// competitive in select.
///
/// This makes Checker useful for “periodic maintenance” logic: once activated,
/// it guarantees a tick within at most one interval when no other branches
/// keep winning, while preventing overly frequent activations from causing
/// overly frequent checks.
pub struct Checker {
    interval: Interval,
    need_check: bool,
}

impl Checker {
    pub fn new(period: Duration) -> Self {
        let mut interval = tokio::time::interval(period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Self {
            interval,
            need_check: false,
        }
    }

    #[inline]
    pub fn activate(&mut self) {
        if !self.need_check {
            self.need_check = true;
        }
    }

    pub async fn wait(&mut self) {
        if self.need_check {
            self.interval.tick().await;
            self.need_check = false;
        } else {
            Pending.await
        }
    }
}

struct Pending;

impl Future for Pending {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::task::Poll::Pending
    }
}

/// | 50  | 50 | 50 | 50 |         2000   |   3000  | 50 | 100 | 200 | 700|
///
/// |50|tick|       1000      |tick|1150|tick|3000|tick|             1000|tick
#[tokio::test]
async fn checker_test() {
    let mut checker = Checker::new(Duration::from_secs(1));
    let times = [50, 50, 50, 50, 2000, 3000, 50, 100, 200, 700]
        .into_iter()
        .map(Duration::from_millis);
    let mut expected_times = [50, 1050, 2200, 5200, 6200]
        .into_iter()
        .map(Duration::from_millis);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    tokio::time::pause();
    let ins = tokio::time::Instant::now();

    tokio::task::spawn_blocking(move || {
        for t in times {
            let _ = tokio::time::advance(t);
            std::thread::sleep(Duration::from_millis(1));
            let _ = tx.send(());
        }
    });

    loop {
        tokio::select! {
            res = rx.recv() => {
                if res.is_none(){
                    break;
                }
                checker.activate();
            }
            _ = checker.wait() => {
                let elaspsed = ins.elapsed();
                println!("{:?}: check!", elaspsed);
                assert_eq!(elaspsed, expected_times.next().unwrap());
            }
        }
    }
}
