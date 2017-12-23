//!
//! # What is the purpose of file_db
//!
//! * Provide an easy to use log target for exactly one type of structured data.
//! * Intended e.g for usage on Raspberry Pi's SD card: Don't write the same file over and over,
//!   instead create new files every now and then. This can increase the sd card lifetime.
//! * Async writes/updates by using a Thread Pool.
//! * Caching of filtered/serialized data and retrieval of that data as Arc<Vec<u8>>.
//!
//! # Usage
//!
//! * Define your Datatype and implement `?`. This allows to split the data into chunks.
//! * For timestamped data, wrap into `Timestamped<D>` which chunks by day.
//!
//! # More
//!
//! * Uses messagepack by default to save data to disk.
//!
//!
//!
//!
//!
//!
//!

#![allow(dead_code, unused_variables)]

extern crate futures;
extern crate futures_cpupool;
#[macro_use]
extern crate failure;
#[macro_use]
extern crate log;
extern crate env_logger;
extern crate chrono;
extern crate dotenv;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate rmp_serde as rmps;
extern crate libc;

mod filedb;
mod timestamped;
mod lock;

#[cfg(test)]
mod test {

    use futures_cpupool::CpuPool;
    use std::path::{Path, PathBuf};
    use std::fs::create_dir;
    use std::ops::Deref;
    use filedb::{FileDb, ChunkableData};
    use timestamped::Timestamped;
    use futures::Future;

    #[derive(Serialize, Deserialize, Clone)]
    struct TestData {
        a : usize,
        b : Vec<u16>,
    }

    type MyDb = FileDb<Timestamped<TestData>>;

    fn clear_db(database_url: &PathBuf) {
        let db = MyDb::new(&database_url, CpuPool::new(2)).unwrap();
        db.clear_all_data_ondisk_and_in_memory().unwrap();
    }

    #[test]
    fn test_db() {

        let database_url: PathBuf = "/tmp/file_db_test".into();
        create_dir(&database_url).ok();

        clear_db(&database_url);

        let db = MyDb::new(&database_url, CpuPool::new(2)).unwrap();


        let testdata = Timestamped::now(TestData { a : 2, b : vec![1,3,5,235] });
        let date = testdata.chunk_key();

        let pooloperation = db.insert_or_update_async(testdata.clone());
        let testdata_returned = pooloperation.wait().unwrap();
        assert_eq!(testdata.a, testdata_returned.a);

        // Add entry while holding the vec (so the vec must be cloned)
        let vec = db.get_by_chunk_key_async(date).wait().unwrap();
        assert_eq!(vec[0].a, testdata.a);
        db.insert_or_update_async(Timestamped::now(testdata.deref().clone())).wait().unwrap();
        assert_eq!(vec.len(), 1);
        let vec = db.get_by_chunk_key_async(date).wait().unwrap();
        assert_eq!(vec.len(), 2);


        let and_back = db.get_by_key_async(testdata.key()).wait().unwrap().unwrap();
        assert!(and_back.a == testdata.a);


        // lock file exists
        let lockfile = database_url.join("pid.dblock");
        assert!(lockfile.is_file());

        // data-file
        let data_file =
            database_url.join(Path::new(&format!("chunk-{}.db", date.0.format("%Y-%m-%d"))));

        // write to disk on drop and delete pid.dblock
        assert!(!data_file.is_file());
        drop(db);
        assert!(!lockfile.is_file());
        assert!(data_file.is_file());
        let date_created = data_file.metadata().unwrap().modified().unwrap();

        // reopen and reread data
        let db = MyDb::new(&database_url, CpuPool::new(2)).unwrap();
        let and_back = db.get_by_key_async(testdata.key()).wait().unwrap().unwrap();
        assert!(and_back.b == testdata.b);
        drop(db);
        // we had no changes, so the file should not have been saved again
        let date_no_modification = data_file.metadata().unwrap().modified().unwrap();
        assert_eq!(date_created, date_no_modification);

        // clear db
        clear_db(&database_url);
        assert!(!data_file.is_file());
        assert!(!lockfile.is_file());

        // reopen and reread data should fail
        let db = MyDb::new(&database_url, CpuPool::new(2)).unwrap();
        assert!(db.get_by_key_async(testdata.key()).wait().unwrap().is_none());
    }


}



#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
