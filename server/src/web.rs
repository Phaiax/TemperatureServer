
use failure::Error;

use shared::SharedDataRc;

use hyper::server::{Http, Request, Response, Service, Server, NewService};
use hyper::header::ContentLength;

use futures::future::Future;
use futures;


pub fn make_web_server(shared : &SharedDataRc) -> Server<HelloWorldSpawner, ::hyper::Body> {
    let addr = "0.0.0.0:12345".parse().unwrap();
    Http::new().bind(&addr, HelloWorldSpawner { shared : shared.clone() }).unwrap()
}

pub struct HelloWorldSpawner {
    shared : SharedDataRc,
}

impl NewService for HelloWorldSpawner {
    type Request = Request;
    type Response = Response;
    type Error = ::hyper::Error;
    type Instance = HelloWorld;
    fn new_service(&self) -> Result<Self::Instance, ::std::io::Error> {
        Ok(
            HelloWorld { shared: self.shared.clone() }
        )
    }
}

pub struct HelloWorld {
    shared : SharedDataRc,
}

impl Service for HelloWorld {
    // boilerplate hooking up hyper's server types
    type Request = Request;
    type Response = Response;
    type Error = ::hyper::Error;
    // The future representing the eventual Response your call will
    // resolve to. This can change to whatever Future you need.
    type Future = Box<Future<Item=Self::Response, Error=Self::Error>>;

    fn call(&self, _req: Request) -> Self::Future {
        // We're currently ignoring the Request
        // And returning an 'ok' Future, which means it's ready
        // immediately, and build a Response with the 'PHRASE' body.
        let shared = self.shared.temperatures.get();
        let formatted = format!("{:?}", shared);

        Box::new(futures::future::ok(
            Response::new()
                .with_header(ContentLength(formatted.len() as u64))
                .with_body(formatted)
        ))
    }
}
