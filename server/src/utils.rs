
use futures::Future;
use failure::Error;


mod print_and_forget_error {

    use futures::Future;
    use futures::{Poll, Async};
    use failure::Error;
    use super::print_error_and_causes;


    pub struct PrintAndForgetError<F> where F: Future {
        future: F,
        context: Option<&'static str>,
    }

    pub fn new<F>(future: F, context: Option<&'static str>) -> PrintAndForgetError<F>
        where F: Future,
    {
        PrintAndForgetError {
            future: future,
            context,
        }
    }

    impl<F, I, E> Future for PrintAndForgetError<F>
        where F: Future<Item=I, Error=E>,
        E: Into<Error>
    {
        type Item = ();
        type Error = ();

        fn poll(&mut self) -> Poll<(), ()> {
            match self.future.poll() {
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
    F: Future<Item = I, Error = E>,
    E: Into<Error>
{
    fn print_and_forget_error(self) -> PrintAndForgetError<Self>
    where
        Self: Sized,
    {
        print_and_forget_error::new(self, None)
    }

    fn print_and_forget_error_with_context(self, context : &'static str) -> PrintAndForgetError<Self>
    where
        Self: Sized,
    {
        print_and_forget_error::new(self, Some(context))
    }
}

pub trait ResultExt<T, E> {
    fn print_error_and_causes(self) where E: Into<Error>;

//    fn compat(self) -> Result<T, Compat<E>>;
//    fn context<D>(self, context: D) -> Result<T, Context<D>>
//    where
//        D: Display + Send + Sync + 'static;
//    fn with_context<F, D>(self, f: F) -> Result<T, Context<D>>
//    where
//        F: FnOnce(&E) -> D,
//        D: Display + Send + Sync + 'static;
}

impl<T, E> ResultExt<T, E> for Result<T, E> {
    fn print_error_and_causes(self) where E: Into<Error> {
        match self {
            Ok(_t) => (),
            Err(e) => {
                print_error_and_causes(e);
            }
        }
    }

}


pub fn print_error_and_causes<E>(err: E) where E: Into<Error> {
    let err = err.into();
    for (i, cause) in err.causes().enumerate() {
        if i == 0 {
            error!("{}", cause);
        } else {
            error!(" > caused by: {}", cause);
        }
    }
    error!("{}", err.backtrace());
}


pub struct OnDrop(Option<Box<FnMut() -> ()>>);

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