use std::future::{Future, poll_fn};
use std::task::Poll;

pub(crate) async fn assert_pending_once<F: Future + ?Sized>(future: std::pin::Pin<&mut F>) {
    let mut future = Some(future);
    poll_fn(|context| {
        let result = future
            .take()
            .expect("single-poll helper polled more than once")
            .poll(context);
        assert!(result.is_pending(), "future completed on its first poll");
        Poll::Ready(())
    })
    .await;
}
