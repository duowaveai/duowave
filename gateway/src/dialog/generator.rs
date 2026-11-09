//! A stream written as an async block, the way the Node gateway wrote its async generators: the block runs only
//! while the stream is polled, stops at each `yielder.send(item).await` until the reader asks for the next item,
//! and is dropped with the stream (an answer nobody reads any more stops where it is).

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};

use futures::stream::{BoxStream, Stream};
use futures::StreamExt;

/// The block's side of the stream.
pub struct Yielder<T> {
    slot: Arc<Mutex<Option<T>>>,
}

impl<T> Yielder<T> {
    /// Hands `item` to the reader and returns once the reader asks for the next one, as `yield` did.
    pub async fn send(&self, item: T) {
        let mut item = Some(item);
        poll_fn(|_| {
            let mut slot = lock(&self.slot);
            if let Some(item) = item.take() {
                *slot = Some(item);
                // No waker: the stream polls this block again exactly when it is asked for the next item.
                return Poll::Pending;
            }
            if slot.is_some() {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await
    }
}

struct Generator<T> {
    slot: Arc<Mutex<Option<T>>>,
    body: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

impl<T> Stream for Generator<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let this = self.get_mut();
        let Some(body) = this.body.as_mut() else { return Poll::Ready(None) };
        let done = body.as_mut().poll(cx).is_ready();
        if done {
            this.body = None;
        }
        match lock(&this.slot).take() {
            Some(item) => Poll::Ready(Some(item)),
            None if done => Poll::Ready(None),
            None => Poll::Pending,
        }
    }
}

fn lock<T>(slot: &Mutex<Option<T>>) -> MutexGuard<'_, Option<T>> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The items `body` sends, in order; the stream ends when the block returns.
pub fn generate<T, F, Fut>(body: F) -> BoxStream<'static, T>
where
    T: Send + 'static,
    F: FnOnce(Yielder<T>) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let slot = Arc::new(Mutex::new(None));
    let future = body(Yielder { slot: slot.clone() });
    Generator { slot, body: Some(Box::pin(future)) }.boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn yields_in_order_and_runs_only_as_far_as_it_is_read() {
        let steps = Arc::new(AtomicUsize::new(0));
        let counted = steps.clone();
        let mut stream = generate(move |y| async move {
            for n in 1..=3 {
                counted.fetch_add(1, Ordering::SeqCst);
                y.send(n).await;
            }
            counted.fetch_add(10, Ordering::SeqCst);
        });
        assert_eq!(steps.load(Ordering::SeqCst), 0, "nothing runs before the first read");
        assert_eq!(stream.next().await, Some(1));
        assert_eq!(steps.load(Ordering::SeqCst), 1, "it stops at the yield");
        assert_eq!(stream.next().await, Some(2));
        assert_eq!(stream.next().await, Some(3));
        assert_eq!(steps.load(Ordering::SeqCst), 3);
        assert_eq!(stream.next().await, None);
        assert_eq!(steps.load(Ordering::SeqCst), 13);
        assert_eq!(stream.next().await, None);
    }

    #[tokio::test]
    async fn waits_inside_and_stops_when_dropped() {
        let after = Arc::new(AtomicUsize::new(0));
        let seen = after.clone();
        let mut stream = generate(move |y| async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            y.send("first").await;
            seen.fetch_add(1, Ordering::SeqCst);
            y.send("second").await;
        });
        assert_eq!(stream.next().await, Some("first"));
        drop(stream);
        assert_eq!(after.load(Ordering::SeqCst), 0, "a dropped stream does not go on");
    }
}
