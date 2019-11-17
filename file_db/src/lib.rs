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
//! * Run tests with `cargo test -- --test-threads=1`
//!
//! # Internal Data Structure
//!
//! ```txt
//! Arc<Mutex<                                                      >>
//!            HashMap<ChunkKey,                                   >
//!                              Arc<Mutex<                      >>
//!                                        |--------CHUNK-------|
//!                                        Option<              >
//!                                               Arc<Vec<    >>
//!                                                       Data
//!
//!
//! ```

#![allow(dead_code, unused_variables)]

mod filedb;
pub mod timestamped;
pub mod lock;

pub use filedb::{FileDb, ToFilenamePart, ChunkableData};
pub use timestamped::{Timestamped, TimestampedMethods, create_intervall_filtermap};
pub use typemap::Key;

#[cfg(test)]
mod tests {

    //use futures_cpupool::CpuPool;
    use std::path::{Path, PathBuf};
    use std::fs::create_dir;
    use std::ops::Deref;
    use crate::filedb::{ChunkableData, FileDb};
    use crate::timestamped::Timestamped;
    use serde_derive::{Serialize, Deserialize};

    #[derive(Serialize, Deserialize, Clone)]
    struct TestData {
        a: usize,
        b: Vec<u16>,
    }

    type MyDb = FileDb<Timestamped<TestData>>;

    async fn clear_db(database_url: &PathBuf) {
        let db = MyDb::new(&database_url, 2, "v1").await.unwrap();
        db.clear_all_data_ondisk_and_in_memory().await.unwrap();
    }

    #[async_std::test]
    async fn test_db() {
        let database_url: PathBuf = "/tmp/file_db_test".into();
        create_dir(&database_url).ok();

        clear_db(&database_url).await;

        let db = MyDb::new(&database_url, 2, "v1").await.unwrap();


        let testdata = Timestamped::now(TestData {
            a: 2,
            b: vec![1, 3, 5, 235],
        });
        let date = testdata.chunk_key();

        let pooloperation = db.insert_or_update_async(testdata.clone());
        let testdata_returned = pooloperation.await.unwrap();
        assert_eq!(testdata.a, testdata_returned.a);

        // Add entry while holding the vec (so the vec must be cloned)
        let vec = db.get_by_chunk_key_async(date).await.unwrap();
        assert_eq!(vec[0].a, testdata.a);
        db.insert_or_update_async(Timestamped::now(testdata.deref().clone()))
            .await
            .unwrap();
        assert_eq!(vec.len(), 1);
        let vec = db.get_by_chunk_key_async(date).await.unwrap();
        assert_eq!(vec.len(), 2);


        let and_back = db.get_by_key_async(testdata.key()).await.unwrap().unwrap();
        assert!(and_back.a == testdata.a);


        // lock file exists
        let lockfile = database_url.join("pid.dblock");
        assert!(lockfile.is_file());

        // data-file
        let data_file = database_url.join(Path::new(
            &format!("chunk-{}.db.v1", date.0.format("%Y-%m-%d")),
        ));

        // write to disk on drop and delete pid.dblock
        assert!(!data_file.is_file());
        drop(db);
        assert!(!lockfile.is_file());
        assert!(data_file.is_file());
        let date_created = data_file.metadata().unwrap().modified().unwrap();

        // reopen and reread data
        let db = MyDb::new(&database_url, 2, "v1").await.unwrap();
        let and_back = db.get_by_key_async(testdata.key()).await.unwrap().unwrap();
        assert!(and_back.b == testdata.b);
        drop(db);
        // we had no changes, so the file should not have been saved again
        let date_no_modification = data_file.metadata().unwrap().modified().unwrap();
        assert_eq!(date_created, date_no_modification);

        // clear db
        clear_db(&database_url).await;
        assert!(!data_file.is_file());
        assert!(!lockfile.is_file());

        // reopen and reread data should fail
        let db = MyDb::new(&database_url, 2, "v1").await.unwrap();
        assert!(
            db.get_by_key_async(testdata.key())
                .await
                .unwrap()
                .is_none()
        );
    }

    use typemap::Key;
    use std::sync::Arc;
    use chrono::{Duration, NaiveDateTime};
    use crate::timestamped::create_intervall_filtermap;

    struct CachedSerializationType;
    impl Key for CachedSerializationType {
        type Value = Vec<usize>;
    }


    #[async_std::test]
    async fn test_caching() {
        let database_url: PathBuf = "/tmp/file_db_test".into();
        create_dir(&database_url).ok();

        clear_db(&database_url).await;

        let db = MyDb::new(&database_url, 2, "v1").await.unwrap();

        for i in 1..1000 {
            let testdata = Timestamped::at(
                NaiveDateTime::from_timestamp(3600 + i * 15, 0),
                TestData {
                    a: i as usize,
                    b: vec![1, 3, 5, 235],
                },
            );
            db.insert_or_update_async(testdata.clone()).await.unwrap();
        }

        let date = db.get_non_empty_chunk_keys_async().await.unwrap()[0];

        // filter every minute
        let filter_one_per_minute =
            create_intervall_filtermap(Duration::minutes(1), |data : &Timestamped<TestData>| data.a as usize, 0.25);


        let filtered_cache: Arc<Vec<usize>> =
            db.custom_cached_by_chunk_key_async::<CachedSerializationType>(date, filter_one_per_minute)
                .await
                .unwrap();

        // So with 4 entries per minute, and 1000 entries, there should be around 250 left.
        // For each entry, we pushed one usize into the Vec.

        assert_eq!(filtered_cache.len(), 250);

        // Test that internal cache is cleared
        for i in 1000..2000 {
            let testdata = Timestamped::at(
                NaiveDateTime::from_timestamp(3600 + i * 15, 0),
                TestData {
                    a: i as usize,
                    b: vec![1, 3, 5, 235],
                },
            );
            db.insert_or_update_async(testdata.clone()).await.unwrap();
        }

        let filter_one_per_minute =
            create_intervall_filtermap(Duration::minutes(1), |data : &Timestamped<TestData>| data.a as usize, 0.25);

        let filtered_cache2: Arc<Vec<usize>> =
            db.custom_cached_by_chunk_key_async::<CachedSerializationType>(date, filter_one_per_minute)
                .await
                .unwrap();

        assert_eq!(filtered_cache.len(), 250);
        assert_eq!(filtered_cache2.len(), 500);

        drop(db);
        clear_db(&database_url).await;
    }

}
