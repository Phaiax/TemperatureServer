
use std::env;
use std::thread::{spawn, Thread};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Mutex;

use diesel;
use diesel_infer_schema;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;

use futures::{future, Future};

use futures_cpupool::CpuPool;

use failure::{Error, ResultExt};

use temp::{Temperatures, TemperatureStats};

use chrono::NaiveDateTime;
use chrono::prelude::*;

pub struct Db {
    conn: Mutex<SqliteConnection>,
    pool: CpuPool,
}


impl Db {
    pub fn establish_connection() -> Result<Db, Error> {

        let database_url =
            env::var("DATABASE_URL").context("Environment variable DATABASE_URL must be set.")?;

        let conn = SqliteConnection::establish(&database_url)
            .context(&format!("Error connecting to {}", database_url))?;

        let pool = CpuPool::new(1); // no multi threading with sqlite!

        Ok( Db { conn: Mutex::new(conn), pool } )
    }

    fn log(&self, data : TemperatureStats) {
        let conn = self.conn.lock().expect("Connection mutex already in use.");
        self.pool.spawn_fn(|| {
            use self::schema::temperatures; //::dsl::*;
            use self::models::{Temperature, NewTemperatures};

            let time = Local::now().naive_utc();

            let in_celsius = ::temp::raw2celsius(&data.mean);

            let new_log = NewTemperatures {
                r0: data.mean[0] as i32,
                r1: data.mean[1] as i32,
                r2: data.mean[2] as i32,
                r3: data.mean[3] as i32,
                r4: data.mean[4] as i32,
                r5: data.mean[5] as i32,
                t0: (in_celsius[0] * 10.) as i32,
                t1: (in_celsius[1] * 10.) as i32,
                t2: (in_celsius[2] * 10.) as i32,
                t3: (in_celsius[3] * 10.) as i32,
                t4: (in_celsius[4] * 10.) as i32,
                t5: (in_celsius[5] * 10.) as i32,
                timestamp: time,
            };

            match diesel::insert_into(temperatures::table)
                .values(&new_log)
                .execute(&*conn) {
                Ok(_) => return future::ok(()),
                Err(e) => {
                    error!("Error saving new log line ({})", e);
                    return future::err(())
                }
            }

        }).forget();
        // forget() keeps the pool running but forgets the CpuFuture
    }
}


pub mod schema {
    infer_schema!("dotenv:DATABASE_URL");
    /*
    table! {
        temperatures (id) {
            id -> Integer,
            r0 -> Integer,
            r1 -> Integer,
            r2 -> Integer,
            r3 -> Integer,
            r4 -> Integer,
            r5 -> Integer,
            t0 -> Integer,
            t1 -> Integer,
            t2 -> Integer,
            t3 -> Integer,
            t4 -> Integer,
            t5 -> Integer,
            timestamp -> Timestamp,
        }
    }*/
}

pub mod models {
    use super::schema::temperatures;
    use chrono::NaiveDateTime;

    #[derive(Queryable)]
    pub struct Temperature {
        pub id: i32,
        pub r0: i32,
        pub r1: i32,
        pub r2: i32,
        pub r3: i32,
        pub r4: i32,
        pub r5: i32,
        pub t0: i32,
        pub t1: i32,
        pub t2: i32,
        pub t3: i32,
        pub t4: i32,
        pub t5: i32,
        pub timestamp: NaiveDateTime,
    }

    #[derive(Insertable)]
    #[table_name="temperatures"]
    pub struct NewTemperatures {
        pub r0: i32,
        pub r1: i32,
        pub r2: i32,
        pub r3: i32,
        pub r4: i32,
        pub r5: i32,
        pub t0: i32,
        pub t1: i32,
        pub t2: i32,
        pub t3: i32,
        pub t4: i32,
        pub t5: i32,
        pub timestamp: NaiveDateTime,
    }
}
