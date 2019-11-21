
use std::future::Future;
//use futures01::Future as Future01;
use failure::Error;
use log::error;


use self::print_and_forget_error::PrintAndForgetError;


/// Helper function to `log` info about a `failure::Error`
pub fn print_error_and_causes<E>(err: E) where E: Into<Error> {
    let err = err.into();
    for (i, cause) in err.iter_chain().enumerate() {
        if i == 0 {
            error!("{}", cause);
        } else {
            error!(" > caused by: {}", cause);
        }
    }
    error!("{}", err.backtrace());
}

/// Extension methods for `Future`s
pub trait FutureExt<I> {

    /// Convert a `Future<Output=Result<I, E>>` to a `Future<Output=Option<I>>`
    /// If the Output is `Err`, then `log` the error before discarding.
    fn print_and_forget_error(self) -> PrintAndForgetError<Self>
    where
        Self: Sized + Future;

    /// Convert a `Future<Output=Result<I, E>>` to a `Future<Output=Option<I>>`.
    /// If the Output is `Err`, then wrap with a `failure::Context` and `log` the error before discarding.
    fn print_and_forget_error_with_context(self, context : &'static str) -> PrintAndForgetError<Self>
    where
        Self: Sized + Future;
}

// Impl for the FutureExt trait on Result-Futures
impl<F, I, E> FutureExt<F> for F
where
    F: Future<Output=Result<I, E>> + Sized,
    E: Into<Error>
{
    fn print_and_forget_error(self) -> PrintAndForgetError<Self>
    {
        PrintAndForgetError::new(self, None)
    }

    fn print_and_forget_error_with_context(self, context : &'static str) -> PrintAndForgetError<Self>
    {
        PrintAndForgetError::new(self, Some(context))
    }
}


// Impl for the FutureExt trait on Result-Futures
// impl<F, I, E> FutureExt<F> for F
// where
//     F: Future01<Item=I, Error=E> + Sized,
//     E: Into<Error>
// {
//     fn print_and_forget_error(self) -> PrintAndForgetError<Self>
//     {
//         PrintAndForgetError::new(self, None)
//     }

//     fn print_and_forget_error_with_context(self, context : &'static str) -> PrintAndForgetError<Self>
//     {
//         PrintAndForgetError::new(self, Some(context))
//     }
// }

// Contains the helper struct for the above extension methods that implements Future
/// (Like the struct `Map` for the combinator `.map()`)
mod print_and_forget_error {

    use futures::Future;
    use futures01::Future as Future01;

    use std::task::{Poll, Context};
    use std::pin::Pin;
    use failure::Error;
    use pin_utils::{unsafe_pinned};
    use super::print_error_and_causes;


    pub struct PrintAndForgetError<Fut> {
        inner: Fut,
        context: Option<&'static str>,
    }

    impl<Fut> PrintAndForgetError<Fut> {
        unsafe_pinned!(inner: Fut);

        pub fn new(future: Fut, context: Option<&'static str>) -> PrintAndForgetError<Fut>
        {
            PrintAndForgetError {
                inner: future,
                context,
            }
        }
    }


    impl<F, I, E> Future for PrintAndForgetError<F>
        where F: Future<Output=Result<I, E>>,
        E: Into<Error>
    {
        type Output = Option<I>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            match self.as_mut().inner().poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Ok(i)) => Poll::Ready(Some(i)),
                Poll::Ready(Err(e)) => {
                    match self.context {
                        Some(context) => print_error_and_causes(e.into().context(context)),
                        None => print_error_and_causes(e),
                    }
                    Poll::Ready(None)
                },
            }
        }
    }

    impl<F, I, E> Future01 for PrintAndForgetError<F>
        where F: Future01<Item=I, Error=E>,
        E: Into<Error>
    {
        type Item = ();
        type Error = ();

        fn poll(&mut self) -> futures01::Poll<(), ()> {
            use futures01::Async;
            match self.inner.poll() {
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Ok(Async::Ready(_e)) => Ok(Async::Ready(())),
                Err(e) => {
                    match self.context {
                        Some(context) => print_error_and_causes(e.into().context(context)),
                        None => print_error_and_causes(e),
                    }
                    Err(())
                },
            }
        }
    }

}

/// Extension methods for `Result`s
pub trait ResultExt<T, E> {
    /// Like `.ok()`, but log the error via `log`
    fn print_error_and_causes(self) -> Option<T> where E: Into<Error>;
}

impl<T, E> ResultExt<T, E> for Result<T, E> {
    fn print_error_and_causes(self) -> Option<T> where E: Into<Error> {
        match self {
            Ok(t) => Some(t),
            Err(e) => {
                print_error_and_causes(e);
                None
            }
        }
    }

}

