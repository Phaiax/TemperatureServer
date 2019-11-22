
use std::path::PathBuf;
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::prelude::*;
use std::ffi::OsStr;
use std::sync::Arc;
use std::time::{Duration as SDuration, Instant};

use crate::utils::{print_error_and_causes, FutureExt as _, ResultExt as ResultExt2};


use crate::HeaterControlMode;

use failure::{Error, ResultExt, bail};

use crate::Shared;
use crate::DataLogEntry;
use crate::TSDataLogEntry;
use file_db::{create_intervall_filtermap, TimestampedMethods};

use hyper::StatusCode;
use hyper::server::{Http, NewService, Request, Response, Server, Service};
use hyper::header;

use futures::future::{FutureExt as _, TryFutureExt}; // for conversion

use futures01::future::{self, Future};
use futures01;
use futures01::Stream;

use handlebars::Handlebars;

use tokio_inotify::AsyncINotify;

use rmp_serde::{Deserializer, Serializer};
use serde::{Deserialize, Serialize};
use serde_derive::{Deserialize, Serialize};

use chrono::NaiveDate;
use chrono::NaiveDateTime;
use chrono::{Duration, Timelike};

pub fn make_web_server(shared: &Shared) -> Result<Server<HelloWorldSpawner, ::hyper::Body>, Error> {
    let assets_folder: PathBuf = ::std::fs::canonicalize(std::env::var("WEBASSETS_FOLDER")
        .context("Environment variable WEBASSETS_FOLDER must be set.")?)?;

    if !assets_folder.is_dir() {
        bail!(
            "WEBASSETS_FOLDER not found ({})",
            assets_folder.to_string_lossy()
        );
    }

    let index_html = assets_folder.join("index.html");

    if !index_html.is_file() {
        bail!("Missing index.html in WEBASSETS_FOLDER.");
    }

    let template_registry = Rc::new(RefCell::new(Handlebars::new()));

    let addr = "0.0.0.0:12345".parse().unwrap();
    let server = Http::new()
        .bind(
            &addr,
            HelloWorldSpawner {
                shared: shared.clone(),
                template_registry: template_registry.clone(),
                assets_folder: Rc::new(assets_folder.clone()),
            },
        )
        .unwrap();


    // handlebars template
    template_registry
        .borrow_mut()
        .register_template_file("index.html", &index_html)
        .with_context(|_e| {
            format!("Cannot compile {}", &index_html.to_string_lossy())
        })?;
    // todo find all other .html files in the folder

    // React live on asset changes

    let path_notify = AsyncINotify::init(&server.handle())?;
    const IN_CLOSE_WRITE: u32 = 8;
    path_notify
        .add_watch(&assets_folder, IN_CLOSE_WRITE)
        .context("Web server can not watch the webassets folder for changes.")?;

    let template_registry1 = Rc::clone(&template_registry);
    let webassets_updater = path_notify.for_each(move |_event| {
        if _event.name.extension().unwrap_or(OsStr::new("")) == "html" {
            template_registry1
                .try_borrow_mut()
                .map(|mut registry| {
                    registry
                        .register_template_file(
                            &_event.name.to_string_lossy(),
                            assets_folder.join(&_event.name),
                        )
                        .with_context(|_e| {
                            format!("Cannot compile {}", &_event.name.to_string_lossy())
                        })
                        .print_error_and_causes();
                })
                .print_error_and_causes();
        }
        future::ok(())
    });
    server
        .handle()
        .spawn(webassets_updater.map_err(|e| print_error_and_causes(e) ));

    Ok(server)
}

pub struct HelloWorldSpawner {
    shared: Shared,
    template_registry: Rc<RefCell<Handlebars>>,
    assets_folder: Rc<PathBuf>,
}

impl NewService for HelloWorldSpawner {
    type Request = Request;
    type Response = Response;
    type Error = ::hyper::Error;
    type Instance = HelloWorld;
    fn new_service(&self) -> Result<Self::Instance, ::std::io::Error> {
        Ok(HelloWorld {
            shared: async_std::sync::Arc::clone(&self.shared),
            template_registry: Rc::clone(&self.template_registry),
            assets_folder: Rc::clone(&self.assets_folder),
        })
    }
}

pub struct HelloWorld {
    shared: Shared,
    template_registry: Rc<RefCell<Handlebars>>,
    assets_folder: Rc<PathBuf>,
}

type HandlerResult = Box<dyn Future<Item = Response, Error = ::hyper::Error>>;

impl Service for HelloWorld {
    // boilerplate hooking up hyper's server types
    type Request = Request;
    type Response = Response;
    type Error = ::hyper::Error;
    // The future representing the eventual Response your call will
    // resolve to. This can change to whatever Future you need.
    type Future = HandlerResult;

    fn call(&self, _req: Request) -> Self::Future {
        let mut path_segments = _req.path().split("/").skip(1);
        let response_body = match path_segments.next() {
            Some("") | Some("index.html") => self.indexhtml(),
            Some("assets") => self.serve_asset(path_segments),
            Some("history") => self.serve_history(path_segments.next()),
            Some("dates") => self.serve_available_dates(),
            Some("current") => self.serve_current_temperatures(),
            Some("set_heater_control_strategy") if _req.query().is_some() => {
                self.set_heater_control_strategy(_req.query().unwrap())
            }
            _ => make404(),
        };
        response_body
    }
}

impl HelloWorld {
    fn indexhtml(&self) -> HandlerResult {
        let template_registry = Rc::clone(&self.template_registry);
        box_and_convert_error(future::lazy(move || {
            let data: BTreeMap<String, String> = BTreeMap::new();
            let resp = template_registry
                .borrow()
                .render("index.html", &data)
                .map_err(|err| ::failure::Context::new(format!("{}", err)))?;
            Ok(resp).map(str_to_response)
        }))
    }

    fn serve_asset<'a, I: Iterator<Item = &'a str>>(&self, mut path_segments: I) -> HandlerResult {
        match path_segments.next() {
            Some(filename) => {
                let path = self.assets_folder.join(filename);
                box_and_convert_error(future::lazy(move || {
                    if path.is_file() {
                        let mut f = File::open(path).unwrap();
                        let mut buffer = String::new();
                        f.read_to_string(&mut buffer).unwrap();
                        Ok(buffer).map(str_to_response)
                    } else {
                        Err(::failure::err_msg("Unknown asset"))
                    }
                }))
            }
            None => make404(),
        }
    }




    fn serve_history<'a>(&self, date: Option<&'a str>) -> HandlerResult {
        match NaiveDate::parse_from_str(date.unwrap_or("nodate"), "%Y-%m-%d") {
            Ok(date) => {
                let shared = self.shared.clone();

                let every_3_minutes = create_intervall_filtermap(
                    Duration::minutes(3),
                    |data: &TSDataLogEntry| JsData::from(data),
                    0.25,
                );


                use file_db::Key;
                struct CachedAndFilteredMarker;
                impl Key for CachedAndFilteredMarker {
                    type Value = Vec<u8>;
                }

                let fut = async move {
                    let serialized = shared
                        .db
                        .custom_cached_by_chunk_key_async::<CachedAndFilteredMarker>(
                            date.into(),
                            Box::new(move |data: &[::file_db::Timestamped<DataLogEntry>]| {
                                let as_vec: Vec<_> = every_3_minutes(data);
                                let mut buf = Vec::with_capacity(0);
                                as_vec
                                    .serialize(&mut Serializer::new(&mut buf))
                                    .print_error_and_causes();
                                buf
                            }),
                        ).await?;

                    let resp = Response::new()
                        .with_header(header::ContentLength(serialized.len() as u64))
                        .with_header(header::ContentType(::hyper::mime::APPLICATION_MSGPACK))
                        // TODO: Performance by using stream and without copy
                        .with_body((*serialized).clone());

                    Ok(resp)
                };
                box_and_convert_error(fut.boxed().compat())
            }
            Err(_err) => make404(),
        }
    }

    fn serve_available_dates(&self) -> HandlerResult {
        let shared = async_std::sync::Arc::clone(&self.shared);
        let fut = async move {
            let datesvec = shared.db.get_non_empty_chunk_keys_async().await?;
            let json_str = serde_json::to_string(&datesvec)?;

            let resp = Response::new()
                .with_header(header::ContentLength(json_str.len() as u64))
                .with_header(header::ContentType(::hyper::mime::APPLICATION_JSON))
                .with_body(json_str);

            Ok(resp)
        };
        box_and_convert_error(fut.boxed().compat())
    }

    fn serve_current_temperatures(&self) -> HandlerResult {
        let shared = async_std::sync::Arc::clone(&self.shared);

        let fut = async move {
            let data = DataLogEntry::new_from_current(&shared).await;

            #[derive(Serialize)]
            struct Current {
                block : JsData,
                control_strategy : String,
            }

            let data = Current {
                block : (&data).into(), // do better
                control_strategy : format!("{:?}", shared.control_strategy.load()),
            };

            let json_str = serde_json::to_string(&data)?;

            let resp = Response::new()
                .with_header(header::ContentLength(json_str.len() as u64))
                .with_header(header::ContentType(::hyper::mime::APPLICATION_JSON))
                .with_body(json_str);

            Ok(resp)
        };
        box_and_convert_error(fut.boxed().compat())
    }

    fn set_heater_control_strategy(&self, query: &str) -> HandlerResult {
        let shared = async_std::sync::Arc::clone(&self.shared);
        let mut action = None;
        for k_v in query.split('&') {
            if !k_v.contains("=") {
                continue;
            }
            let mut k_v = k_v.split("=");
            if k_v.next() == Some("action") {
                action = k_v.next();
            }
        }
        let answer = match action {
            Some("on") => {
                shared.control_strategy.store(HeaterControlMode::ForceOn{ until: Instant::now() + SDuration::from_secs(3600 * 12) });
                "set: on"
            }
            Some("off") => {
                shared.control_strategy.store(HeaterControlMode::ForceOff{ until: Instant::now() + SDuration::from_secs(3600 * 12) });
                "set: off"
            }
            Some("auto") => {
                shared.control_strategy.store(HeaterControlMode::Auto);
                "set: auto"
            }
            _ => {
                "do nothing"
            }
        };
        box_and_convert_error(future::lazy(move || Ok(answer.to_string()).map(str_to_response)))
    }
}

fn str_to_response(body: String) -> Response {
    Response::new()
        .with_header(header::ContentLength(body.len() as u64))
        .with_body(body)
}

fn box_and_convert_error<F>(result: F) -> HandlerResult
where
    F: Future<Item = Response, Error = Error> + Sized + 'static,
{
    Box::new(result.then(|result| {
        let f = match result {
            Ok(response) => response,
            Err(err) => {
                use std::fmt::Write;
                let mut buf = String::with_capacity(1000);
                for (i, cause) in err.iter_chain().enumerate() {
                    if i == 0 {
                        write!(buf, "<p>{}</p>", cause).unwrap();
                    } else {
                        write!(buf, "<p> &gt; caused by: {} </p>", cause).unwrap();
                    }
                }
                write!(buf, "<pre>{}</pre>", err.backtrace()).unwrap();
                let body = format!(
                    r#"<!doctype html>
                    <html lang="en"><head>
                      <meta charset="utf-8">
                      <title>505 Internal Server Error</title>
                    </head>
                    <body>
                        <h1>505 Internal Server Error</h1>
                        {}
                    </body></html>"#,
                    buf
                );
                print_error_and_causes(err);
                Response::new()
                    .with_status(StatusCode::InternalServerError)
                    .with_header(header::ContentLength(body.len() as u64))
                    .with_body(body)
            }
        };
        Ok(f)
    }))
}

fn make404() -> HandlerResult {
    Box::new(future::lazy(|| {
        let body = format!(
            r#"<!doctype html>
                <html lang="en"><head>
                  <meta charset="utf-8">
                  <title>404 Not Found</title>
                </head>
                <body>
                    <h1>404 Not Found</h1>
                </body></html>"#
        );
        Ok(
            Response::new()
                .with_status(StatusCode::NotFound)
                .with_header(header::ContentLength(body.len() as u64))
                .with_body(body),
        )
    }))
}


#[derive(Serialize, Deserialize, Clone)]
pub struct JsData {
    pub time: String,
    pub high: f64,
    pub highmid: f64,
    pub mid: f64,
    pub midlow: f64,
    pub low: f64,
    pub outside: f64,
    pub heater_state: u8,
    pub reference: f64,
}

impl<'a> From<&'a TSDataLogEntry> for JsData {
    fn from(d: &TSDataLogEntry) -> JsData {
        JsData {
            time: d.time().format("%Y-%m-%dT%H:%M:%S+0000").to_string(),
            high: d.celsius[0] as f64 / 100.0,
            highmid: d.celsius[1] as f64 / 100.0,
            mid: d.celsius[2] as f64 / 100.0,
            midlow: d.celsius[3] as f64 / 100.0,
            low: d.celsius[4] as f64 / 100.0,
            outside: d.celsius[5] as f64 / 100.0,
            heater_state: if d.heater_state { 1 } else { 0 },
            reference: d.reference_celsius.unwrap_or(0) as f64 / 100.0,
        }
    }
}
