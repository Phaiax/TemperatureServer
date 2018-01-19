
extern crate bytes;
extern crate chrono;
extern crate dotenv;
extern crate env_logger;
#[macro_use]
extern crate failure;
extern crate futures;
extern crate futures_cpupool;
extern crate handlebars;
extern crate hyper;
#[macro_use]
extern crate lazy_static;
extern crate libc;
#[macro_use]
extern crate log;
extern crate regex;
extern crate rmp_serde as rmps;
extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
extern crate tokio_core;
extern crate tokio_inotify;
extern crate tokio_io;
extern crate tokio_serial;
extern crate tokio_signal;
extern crate file_db;

#[path="../filedb_old.rs"]
mod filedb;
#[path="../temp.rs"]
mod temp;

use futures::Future;

#[derive(Clone, Serialize, Deserialize)]
struct NewData {
    pub mean: [u16; 6],
    pub celsius: [i16; 6],
    pub plug_state: bool,
    pub reference_celsius: Option<i16>,
}


fn convert(d : filedb::Data) -> file_db::Timestamped<NewData> {
    file_db::Timestamped::at(d.time, NewData {
        mean: d.mean,
        celsius : d.celsius,
        plug_state : d.plug_state,
        reference_celsius: None
    })
}

fn main() {
    let olddb = filedb::FileDb::establish_connection().unwrap();
    let dates = olddb.get_dates().wait().unwrap();

    println!("IN: {:?}", dates);

    let mut all_data = vec![];
    for date in dates {
        let part = olddb.get_by_date(date).wait().unwrap();
        all_data.extend(part.iter().cloned());
    }




    let newdb : file_db::FileDb<file_db::Timestamped<NewData>> = file_db::FileDb::new_from_env("LOG_FOLDER_OUT", 1, "v2").unwrap();

    for i in all_data {
        newdb.insert_or_update_async(convert(i)).wait().unwrap();
    }

}