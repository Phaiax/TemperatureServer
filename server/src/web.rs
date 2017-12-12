
use tokio_inotify::AsyncINotify;

use failure::Error;

use shared::Shared;

use hyper::server::{Http, NewService, Request, Response, Server, Service};
use hyper::header::ContentLength;

use futures::future::Future;
use futures;

use handlebars::Handlebars;

pub fn make_web_server(shared: &Shared) -> Server<HelloWorldSpawner, ::hyper::Body> {

//    let database_url: PathBuf = env::var("TEMPLATE_FOLDER")
//        .context("Environment variable TEMPLATE_FOLDER must be set.")?
//        .into();
//
//    let path_notify = AsyncINotify::new(&shared.handle());

    let addr = "0.0.0.0:12345".parse().unwrap();
    Http::new()
        .bind(
            &addr,
            HelloWorldSpawner {
                shared: shared.clone(),
            },
        )
        .unwrap()

}

pub struct HelloWorldSpawner {
    shared: Shared,
}

impl NewService for HelloWorldSpawner {
    type Request = Request;
    type Response = Response;
    type Error = ::hyper::Error;
    type Instance = HelloWorld;
    fn new_service(&self) -> Result<Self::Instance, ::std::io::Error> {
        Ok(HelloWorld {
            shared: self.shared.clone(),
        })
    }
}

pub struct HelloWorld {
    shared: Shared,
}

impl Service for HelloWorld {
    // boilerplate hooking up hyper's server types
    type Request = Request;
    type Response = Response;
    type Error = ::hyper::Error;
    // The future representing the eventual Response your call will
    // resolve to. This can change to whatever Future you need.
    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, _req: Request) -> Self::Future {
        // We're currently ignoring the Request
        // And returning an 'ok' Future, which means it's ready
        // immediately, and build a Response with the 'PHRASE' body.
        let shared = self.shared.temperatures.get();
        let formatted = format!("{:?}", shared);

        Box::new(futures::future::ok(
            Response::new()
                .with_header(ContentLength(formatted.len() as u64))
                .with_body(formatted),
        ))
    }
}
