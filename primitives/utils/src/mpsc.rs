#[cfg(not(features = "metered"))]
mod inner {
    // just aliased, non performance implications
    use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
    pub type TracingUnboundedSender<T> = UnboundedSender<T>;
    pub type TracingUnboundedReceiver<T> = UnboundedReceiver<T>;

    /// Alias `mpsc::unbounded`
    pub fn tracing_unbounded<T>(_key: &'static str) ->(TracingUnboundedSender<T>, TracingUnboundedReceiver<T>) {
        mpsc::unbounded()
    }
}


#[cfg(features = "metered")]
mod inner {
    //tracing implementation
    use futures::channel::mpsc::{self,
        UnboundedReceiver, UnboundedSender,
        TryRecvError, TrySendError, SendError
    };
    use futures::{sink::Sink, task::{Poll, Context}, stream::Stream};
    use std::pin::Pin;
    use crate::metrics::UNBOUNDED_CHANNELS_COUNTER;

    /// Wrapper Type around `UnboundedSender` that increases the global
    /// measure when a message is added
    #[derive(Debug)]
    pub struct TracingUnboundedSender<T>(&'static str, UnboundedSender<T>);

    /// Wrapper Type around `UnboundedReceiver` that decreases the global
    /// measure when a message is polled
    #[derive(Debug)]
    pub struct TracingUnboundedReceiver<T>(&'static str, UnboundedReceiver<T>);

    /// Wrapper around `mpsc::unbounded` that tracks the in- and outflow via
    /// `UNBOUNDED_CHANNELS_COUNTER`
    pub fn tracing_unbounded<T>(key: &'static str) ->(TracingUnboundedSender<T>, TracingUnboundedReceiver<T>) {
        let (s, r) = mpsc::unbounded();
        (TracingUnboundedSender(key.clone(), s), TracingUnboundedReceiver(key,r))
    }

    impl<T> TracingUnboundedSender<T> {
        /// Proxy function to mpsc::UnboundedSender
        pub fn poll_ready(&self, ctx: &mut Context) -> Poll<Result<(), SendError>> {
            self.1.poll_ready(ctx)
        }

        /// Proxy function to mpsc::UnboundedSender
        pub fn is_closed(&self) -> bool {
            self.1.is_closed()
        }

        /// Proxy function to mpsc::UnboundedSender
        pub fn close_channel(&self) {
            self.1.close_channel()
        }

        /// Proxy function to mpsc::UnboundedSender
        pub fn disconnect(&mut self) {
            self.1.disconnect()
        }

        /// Proxy function to mpsc::UnboundedSender
        pub fn start_send(&mut self, msg: T) -> Result<(), SendError> {
            self.1.start_send(msg)
        }

        /// Proxy function to mpsc::UnboundedSender
        pub fn unbounded_send(&self, msg: T) -> Result<(), TrySendError<T>> {
            self.1.unbounded_send(msg).map(|s|{
                UNBOUNDED_CHANNELS_COUNTER.with_label_values(self.0).incr();
                s
            })
        }

        /// Proxy function to mpsc::UnboundedSender
        pub fn same_receiver(&self, other: &UnboundedSender<T>) -> bool {
            self.1.same_receiver(other)
        }
    }

    impl<T> Clone for TracingUnboundedSender<T> {
        fn clone(&self) -> Self {
            Self(self.0.clone(), self.1.clone())
        }
    }

    impl<T> TracingUnboundedReceiver<T> {

        fn consume(&mut self) {
            // consume all items, make sure to reflect the updated count
            let mut count = 0;
            while let Ok(Some(..)) = self.try_next() {
                count += 1;
            }

            // and discount the messages
            if count > 0 {
                UNBOUNDED_CHANNELS_COUNTER.with_label_values(self.0).decr_by(count);
            }

        }

        /// Proxy function to mpsc::UnboundedReceiver
        /// that consumes all messages first and updates the counter
        pub fn close(&mut self) {
            self.consume();
            self.1.close()
        }

        /// Proxy function to mpsc::UnboundedReceiver
        /// that discounts the messages taken out
        pub fn try_next(&mut self) -> Result<Option<T>, TryRecvError> {
            self.1.try_next().map(|s| {
                if s.is_some() {
                    UNBOUNDED_CHANNELS_COUNTER.with_label_values(self.0).decr();
                }
                s
            })
        }
    }

    impl<T> Drop for TracingUnboundedReceiver<T> {
        fn drop(&mut self) {
            self.consume();
        }
    }

    impl<T> Unpin for TracingUnboundedReceiver<T> {}

    impl<T> Stream for TracingUnboundedReceiver<T> {
        type Item = T;

        fn poll_next(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<T>> {
            let s = self.get_mut();
            match Pin::new(&mut s.1).poll_next(cx) {
                Poll::Ready(msg) => {
                    if msg.is_some() {
                        UNBOUNDED_CHANNELS_COUNTER.with_label_values(self.0).decr();
                   }
                    Poll::Ready(msg)
                }
                Poll::Pending => {
                    Poll::Pending
                }
            }
        }
    }


    impl<T> Sink<T> for TracingUnboundedSender<T> {
        type Error = SendError;

        fn poll_ready(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            TracingUnboundedSender::poll_ready(&*self, cx)
        }

        fn start_send(
            mut self: Pin<&mut Self>,
            msg: T,
        ) -> Result<(), Self::Error> {
            TracingUnboundedSender::start_send(&mut *self, msg)
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            self.disconnect();
            Poll::Ready(Ok(()))
        }
    }

    impl<T> Sink<T> for &TracingUnboundedSender<T> {
        type Error = SendError;

        fn poll_ready(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            TracingUnboundedSender::poll_ready(*self, cx)
        }

        fn start_send(self: Pin<&mut Self>, msg: T) -> Result<(), Self::Error> {
            self.unbounded_send(msg)
                .map_err(TrySendError::into_send_error)
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            self.close_channel();
            Poll::Ready(Ok(()))
        }
    }
}

pub use inner::{tracing_unbounded, TracingUnboundedSender, TracingUnboundedReceiver};