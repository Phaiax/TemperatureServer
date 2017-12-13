
use std::path::PathBuf;

use utils::FutureExt;

use env;

use tokio_inotify::AsyncINotify;

use failure::{Error, ResultExt};

use shared::Shared;

use hyper::server::{Http, NewService, Request, Response, Server, Service};
use hyper::header::ContentLength;

use futures::future::{self, Future};
use futures;
use futures::Stream;

use handlebars::Handlebars;

pub fn make_web_server(shared: &Shared) -> Result<Server<HelloWorldSpawner, ::hyper::Body>, Error> {
    let assets_folder: PathBuf = ::std::fs::canonicalize(env::var("WEBASSETS_FOLDER")
        .context("Environment variable WEBASSETS_FOLDER must be set.")?)?;

    if !assets_folder.is_dir() {
        bail!(
            "WEBASSETS_FOLDER not found ({})",
            assets_folder.to_string_lossy()
        );
    }


    let path_notify = AsyncINotify::init(&shared.handle())?;
    pub const IN_CLOSE_WRITE: u32 = 8;
    path_notify
        .add_watch(&assets_folder, IN_CLOSE_WRITE)
        .context("Web server can not watch the webassets folder for changes.")?;


    let webassets_updater = path_notify.for_each(|_event| {
        future::ok(())
    });
    shared.spawn(webassets_updater.print_and_forget_error());


    let addr = "0.0.0.0:12345".parse().unwrap();
    Ok(
        Http::new()
            .bind(
                &addr,
                HelloWorldSpawner {
                    shared: shared.clone(),
                },
            )
            .unwrap(),
    )
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
