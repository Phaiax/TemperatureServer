
use std::future::Future;
use failure::Error;
use log::{log, error};

mod print_and_forget_error {

    use futures::Future;
    use std::task::{Poll, Context};
    use std::pin::Pin;
    use failure::Error;
    use pin_utils::{unsafe_pinned, unsafe_unpinned};
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

}

use self::print_and_forget_error::PrintAndForgetError;

pub trait FutureExt<I> {
    fn print_and_forget_error(self) -> PrintAndForgetError<Self>
    where
        Self: Sized + Future;

    fn print_and_forget_error_with_context(self, context : &'static str) -> PrintAndForgetError<Self>
    where
        Self: Sized + Future;
}


impl<F, I, E> FutureExt<F> for F
where
    F: Future<Output=Result<I, E>> + Sized,
    E: Into<Error>
{
    fn print_and_forget_error(self) -> PrintAndForgetError<Self>
//    where
//        Self: Sized,
    {
        PrintAndForgetError::new(self, None)
    }

    fn print_and_forget_error_with_context(self, context : &'static str) -> PrintAndForgetError<Self>
//    where
//        Self: Sized,
    {
        PrintAndForgetError::new(self, Some(context))
    }
}


pub trait ResultExt<T, E> {
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


pub struct OnDrop(Option<Box<dyn FnMut() -> ()>>);

impl OnDrop {
    pub fn execute_on_drop<F>(closure: F) -> OnDrop where F : FnMut() -> () + 'static {
        OnDrop(Some(Box::new(closure)))
    }
}

impl Drop for OnDrop {
    fn drop(&mut self) {
        let mut box_ = self.0.take().unwrap();
        box_();
    }

}