#![allow(dead_code, unused_variables)]

use std::env;
use std::thread::{spawn, Thread};
use std::fmt::Display;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Mutex, MutexGuard};
use std::sync::Arc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs::{self, read_dir, File};
use std::io::{BufReader, BufWriter};
use std::io::prelude::*;
use std::time::Duration;

use futures::{future, Future};
use futures_cpupool::{CpuFuture, CpuPool};

use failure::{err_msg, Error, ResultExt};

use temp::{TemperatureStats, Temperatures, raw2celsius};

use chrono::NaiveDateTime;
use chrono::prelude::*;

use rmps::{Deserializer, Serializer};
use serde::{Deserialize, Serialize};

use regex::Regex;

/// This is 50 bytes when serialized as messagepack
/// When written for each second a day, then the daily file has size 4MB
/// When written for each minute a day, then the daily file has size 72KB
#[derive(Serialize, Deserialize, Clone)]
pub struct Data {
    #[serde(serialize_with = "dataserde::naivedatetime_as_intint",
            deserialize_with = "dataserde::intint_as_naivedatetime")]
    pub time: NaiveDateTime,
    pub mean: [u16; 6],
    pub celsius: [i16; 6],
    pub plug_state: bool,
}

mod dataserde {
    use super::*;

    pub fn naivedatetime_as_intint<S>(data: &NaiveDateTime, ser: S) -> Result<S::Ok, S::Error>
    where
        S: ::serde::Serializer,
    {
        use serde::ser::SerializeTuple;
        let mut tup = ser.serialize_tuple(2)?;
        tup.serialize_element(&data.timestamp())?;
        tup.serialize_element(&data.timestamp_subsec_nanos())?;
        tup.end()
    }

    use serde::de::Error as SerdeError;

    struct MySerdeError(String);

    impl SerdeError for MySerdeError {
        fn custom<T>(msg: T) -> Self
        where
            T: Display,
        {
            MySerdeError(format!("{}", msg))
        }
    }

    impl Display for MySerdeError {
        fn fmt(&self, f: &mut ::std::fmt::Formatter) -> Result<(), ::std::fmt::Error> {
            Display::fmt(&self.0, f)
        }
    }

    impl ::std::fmt::Debug for MySerdeError {
        fn fmt(&self, f: &mut ::std::fmt::Formatter) -> Result<(), ::std::fmt::Error> {
            ::std::fmt::Debug::fmt(&self.0, f)
        }
    }

    impl ::std::error::Error for MySerdeError {
        fn description(&self) -> &str {
            &self.0
        }
    }

    struct IntIntVisitor;
    impl<'de> ::serde::de::Visitor<'de> for IntIntVisitor {
        type Value = NaiveDateTime;
        fn expecting(&self, formatter: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
            write!(formatter, "two integers")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: ::serde::de::SeqAccess<'de>,
        {
            let secs = seq.next_element::<i64>()?
                .ok_or_else(|| SerdeError::invalid_length(0, &"two integers"))?;
            let nanos = seq.next_element::<i64>()?
                .ok_or_else(|| SerdeError::invalid_length(1, &"one more integer"))?;
            Ok(NaiveDateTime::from_timestamp(secs, nanos as u32))
        }
    }

    pub fn intint_as_naivedatetime<'de, D>(deser: D) -> Result<NaiveDateTime, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        deser.deserialize_tuple(2, IntIntVisitor)
    }

}

// Chunks are days
struct Chunk {
    path: PathBuf,
    /// This data is always newer that the disk data, except if data is None
    /// also: Data is always sorted
    data: Option<Arc<Vec<Data>>>,
    /// If Some, then this is on par with data
    serialized: Option<Arc<Vec<u8>>>,
    disk_is_up_to_date: bool,
}

impl Chunk {
    fn new(p: PathBuf) -> Chunk {
        Chunk {
            path: p,
            data: Some(Arc::new(vec![])),
            serialized: None,
            disk_is_up_to_date: false,
        }
    }

    fn load(p: PathBuf) -> Chunk {
        Chunk {
            path: p,
            data: None,
            serialized: None,
            disk_is_up_to_date: true,
        }
    }

    fn force_to_memory(&mut self) -> Result<&mut Arc<Vec<Data>>, Error> {
        if let Some(ref mut data) = self.data {
            Ok(data)
        } else {
            if self.disk_is_up_to_date {
                let file = File::open(&self.path)?;
                let size = file.metadata().map(|m| m.len()).unwrap_or(100000);
                let mut buf_reader = BufReader::new(file);
                let mut contents = Vec::with_capacity(size as usize);
                buf_reader.read_to_end(&mut contents)?;

                let mut de = Deserializer::new(&contents[..]);
                let desered: Vec<Data> = Deserialize::deserialize(&mut de)?;

                self.data = Some(Arc::new(desered));
                Ok(self.data.as_mut().unwrap())
            } else {
                bail!(
                    "No data, but disk not up to date!? ({})",
                    self.path.to_string_lossy()
                );
            }
        }
    }

    fn sync_to_disk(&mut self) -> Result<(), Error> {
        if !self.disk_is_up_to_date {
            if self.data.is_none() {
                bail!("No data to write to {}", self.path.to_string_lossy());
            }

            {
                let mut file = File::create(&self.path)?;
                let serialized = self.sync_serialized()?;
                file.write_all(&serialized[..])?;
            }

            self.disk_is_up_to_date = true;
        }
        Ok(())
    }

    fn sync_serialized(&mut self) -> Result<&Arc<Vec<u8>>, Error> {
        if let Some(ref serialized) = self.serialized {
            Ok(serialized)
        } else {
            let mut buf = Vec::with_capacity(0);
            let serialized = self.force_to_memory()?
                .serialize(&mut Serializer::new(&mut buf))?;
            self.serialized = Some(Arc::new(buf));
            Ok(&self.serialized.as_ref().unwrap())
        }
    }

    pub fn update<F, R>(&mut self, f: F) -> Result<R, Error>
    where
        F: FnOnce(&mut Vec<Data>) -> R,
    {
        self.update_no_sort(|mut data| {
            let r = f(&mut data);
            data.sort_by_key(|data| data.time);
            r
        })
    }

    fn update_no_sort<F, R>(&mut self, f: F) -> Result<R, Error>
    where
        F: FnOnce(&mut Vec<Data>) -> R,
    {
        let r = f(Arc::make_mut(self.force_to_memory()?));
        self.serialized = None;
        self.disk_is_up_to_date = false;
        Ok(r)
    }

    pub fn get_shared_vec(&mut self) -> Result<Arc<Vec<Data>>, Error> {
        Ok(self.force_to_memory()?.clone())
    }

    pub fn get_serialized(&mut self) -> Result<Arc<Vec<u8>>, Error> {
        Ok(self.sync_serialized()?.clone())
    }

    pub fn get_by_key(&mut self, key: NaiveDateTime) -> Result<Option<Data>, Error> {
        let data = self.force_to_memory()?;
        match data.binary_search_by_key(&key, |data| data.time) {
            Ok(index) => Ok(data.get(index).cloned()),
            Err(_would_be_insert) => Ok(None),
        }
    }

    pub fn insert_or_update(&mut self, data: Data) -> Result<(), Error> {
        self.update_no_sort(|vec| {
            let pos = vec.binary_search_by_key(&data.time, |data| data.time);
            match pos {
                Ok(index) => vec[index] = data,
                Err(would_be_insert) => vec.insert(would_be_insert, data),
            }
        })
    }
}



type Chunks = HashMap<NaiveDate, Arc<Mutex<Chunk>>>;

// Filedb stores chunked data into a folder addressable by time and date
pub struct FileDb {
    chunks: Arc<Mutex<Chunks>>,
    path_base: PathBuf,
    pool: CpuPool,
    lockfile: (File, PathBuf),
}


impl FileDb {
    pub fn establish_connection() -> Result<FileDb, Error> {
        let database_url: PathBuf = env::var("LOG_FOLDER")
            .context("Environment variable LOG_FOLDER must be set.")?
            .into();


        let lockfilepath = database_url.join("pid.dblock");

        let mut lockfile = if lockfilepath.is_file() {
            File::open(&lockfilepath)?
        } else {
            File::create(&lockfilepath)?
        };
        let pid = unsafe { ::libc::getpid() } as i32;
        write!(lockfile, "{}", pid)?;
        lockfile.sync_all()?;

        use std::os::unix::io::AsRawFd;
        let fd = lockfile.as_raw_fd() as i32 as ::libc::c_int;
        if unsafe { ::libc::flock(fd, ::libc::LOCK_EX) } as i32 == 0 {
            // success
        } else {
            bail!("Could not set database lock.")
        }

        let pool = CpuPool::new(2);

        let mut chunks = HashMap::new();
        for (path, existing_date) in Self::get_existing_files(Path::new(&database_url))? {
            chunks.insert(existing_date, Arc::new(Mutex::new(Chunk::load(path))));
        }

        Ok(FileDb {
            pool,
            chunks: Arc::new(Mutex::new(chunks)),
            path_base: database_url,
            lockfile: (lockfile, lockfilepath),
        })
    }

    fn get_filepath<P: AsRef<Path>>(date: NaiveDate, path_base: P) -> PathBuf {
        path_base
            .as_ref()
            .join(Path::new(&format!("chunk-{}.db", date.format("%Y-%m-%d"))))
    }

    fn get_existing_files<P: AsRef<Path>>(
        path_base: P,
    ) -> Result<Vec<(PathBuf, NaiveDate)>, Error> {
        lazy_static! {
            static ref CHUNK_FILENAME_REGEX : Regex =
                Regex::new(r"chunk-([0-9]{4}-[0-9]{2}-[0-9]{2}).db").unwrap();
        }
        let mut files = vec![];
        for entry in read_dir(&path_base).context("LOG_FOLDER is not a directory")? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let filename = path.file_name()
                    .expect("File has no filename?!")
                    .to_string_lossy();

                if let Some(caps) = CHUNK_FILENAME_REGEX.captures(&filename) {
                    let date = caps.get(1).unwrap().as_str();
                    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
                    let control = Self::get_filepath(date, &path_base);
                    if control != path {
                        bail!("Extracted {}, but path was {:?}", date, path);
                    }
                    files.push((control, date));
                }
            }
        }
        Ok(files)
    }

    fn get_or_create_by_date<P: AsRef<Path>>(
        this: &Arc<Mutex<Chunks>>,
        date: NaiveDate,
        path_base: P,
    ) -> Arc<Mutex<Chunk>> {
        // chunks is a Hasmap
        // chunk is a vector
        let mut chunks: MutexGuard<Chunks> = this.lock().unwrap();
        if chunks.contains_key(&date) {
            chunks.get(&date).unwrap().clone()
        } else {
            let new_chunk = Arc::new(Mutex::new(Chunk::new(Self::get_filepath(date, &path_base))));
            chunks.insert(date, Arc::clone(&new_chunk));
            new_chunk
        }
    }

    pub fn save_all(&self) -> CpuFuture<(), Error> {
        let chunks = self.chunks.clone();

        let f: Box<Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let chunks = chunks.lock().unwrap();
            for (ref date, ref chunk) in (*chunks).iter() {
                chunk.lock().unwrap().sync_to_disk()?;
            }
            Ok(())
        }));

        self.pool.spawn(f)
    }


    fn result_to_future<T, E: Into<Error>>(res: Result<T, E>) -> future::FutureResult<T, Error> {
        match res {
            Ok(d) => future::ok::<T, Error>(d),
            Err(e) => future::err::<T, Error>(e.into().context("Filedb error").into()),
        }
    }

    fn result_to_future_with_context<T, E: Into<Error>>(
        res: Result<T, E>,
        context: &'static str,
    ) -> future::FutureResult<T, Error> {
        match res {
            Ok(d) => future::ok::<T, Error>(d),
            Err(e) => {
                let e: Error = e.into().context(context).into();
                future::err::<T, Error>(e.context("Filedb error").into())
            }
        }
    }

    pub fn insert_or_update_async(
        &self,
        data0: TemperatureStats,
        plug_state: bool,
    ) -> CpuFuture<Data, Error> {
        let chunks = self.chunks.clone();
        let path_base = self.path_base.clone();
        let mut data = Data {
            time: Local::now().naive_local(),
            mean: [0; 6],
            celsius: ::temp::raw2celsius100(&data0.mean),
            plug_state,
        };
        data.mean
            .iter_mut()
            .zip(data0.mean.iter())
            .for_each(|(i, m)| *i = *m as u16);

        let f: Box<Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let date = data.time.date();
            let chunk = Self::get_or_create_by_date(&chunks, date, path_base);
            let r = chunk
                .lock()
                .unwrap()
                .insert_or_update(data.clone())
                .map(|_| data);
            Self::result_to_future_with_context(r, "Could not insert or update")
        }));

        self.pool.spawn(f)
        // forget() keeps the pool running but forgets the CpuFuture
    }

    pub fn get_serialized_by_date(&self, date: NaiveDate) -> CpuFuture<Arc<Vec<u8>>, Error> {
        let chunks = self.chunks.clone();
        let path_base = self.path_base.clone();

        let f: Box<Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let chunk = Self::get_or_create_by_date(&chunks, date, path_base);
            let r = chunk.lock().unwrap().get_serialized();
            Self::result_to_future_with_context(r, "Could not serialize")
        }));

        self.pool.spawn(f)
    }


    pub fn get_by_date(&self, date: NaiveDate) -> CpuFuture<Arc<Vec<Data>>, Error> {
        let chunks = self.chunks.clone();
        let path_base = self.path_base.clone();

        let f: Box<Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let chunk = Self::get_or_create_by_date(&chunks, date, path_base);
            let r = chunk.lock().unwrap().get_shared_vec();
            Self::result_to_future_with_context(r, "Could not get by date")
        }));

        self.pool.spawn(f)
    }

    pub fn get_by_datetime(&self, time: NaiveDateTime) -> CpuFuture<Option<Data>, Error> {
        let chunks = self.chunks.clone();
        let path_base = self.path_base.clone();

        let f: Box<Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let chunk = Self::get_or_create_by_date(&chunks, time.date(), path_base);
            let r = chunk.lock().unwrap().get_by_key(time);
            Self::result_to_future_with_context(r, "Could not get bu datetime")
        }));

        self.pool.spawn(f)
    }

    pub fn clear_all_data_ondisk_and_in_memory(self) -> Result<(), Error> {
        let mut chunks = self.chunks.lock().unwrap();
        for (date, chunk) in chunks.drain() {
            fs::remove_file(Self::get_filepath(date, &self.path_base))?;
        }
        Ok(())
    }
}

impl Drop for FileDb {
    fn drop(&mut self) {
        self.save_all().wait().map_err(|e| error!("{}", e)).ok();

        use std::os::unix::io::AsRawFd;
        let fd = self.lockfile.0.as_raw_fd() as i32 as ::libc::c_int;
        if unsafe { ::libc::flock(fd, ::libc::LOCK_UN) } as i32 == 0 {
            // success
        } else {
            error!("Could not remove database lock.");
        }

        fs::remove_file(&self.lockfile.1)
            .map_err(|e| error!("{}", e))
            .ok();
    }
}


#[cfg(test)]
mod test {
    use super::*;

    fn clear_db() {
        ::dotenv::dotenv().ok();
        let db = FileDb::establish_connection().unwrap();
        db.clear_all_data_ondisk_and_in_memory().unwrap();
    }

    #[test]
    fn test_db() {
        ::dotenv::dotenv().ok();
        ::init_logger();

        clear_db();

        let db = FileDb::establish_connection().unwrap();

        let date = Local::now().naive_local().date();

        let temp = TemperatureStats {
            mean: [500., 600., 500., 400., 500., 700.],
            std_dev: [0., 0., 0., 0., 0., 0.],
        };
        // as u16 for comparision
        let mean_u = [500, 600, 500, 400, 500, 700];

        let pooloperation = db.insert_or_update_async(temp, true);
        let data = pooloperation.wait().unwrap();
        assert!(data.mean == mean_u);

        // Add entry while holding the vec (so the vec must be cloned)
        let vec = db.get_by_date(date).wait().unwrap();
        assert!(vec[0].mean == mean_u);
        db.insert_or_update_async(temp, false).wait().unwrap();
        assert!(vec.len() == 1);
        let vec = db.get_by_date(date).wait().unwrap();
        assert!(vec.len() == 2);


        let and_back = db.get_by_datetime(data.time).wait().unwrap().unwrap();
        assert!(and_back.mean == mean_u);


        // lock file exists
        let database_url: PathBuf = env::var("LOG_FOLDER").unwrap().into();
        let lockfile = database_url.join("pid.dblock");
        assert!(lockfile.is_file());

        // data-file
        let data_file =
            database_url.join(Path::new(&format!("chunk-{}.db", date.format("%Y-%m-%d"))));

        // write to disk on drop
        assert!(!data_file.is_file());
        db.save_all().wait().unwrap();

        drop(db);
        assert!(!lockfile.is_file());
        assert!(data_file.is_file());
        let date_created = data_file.metadata().unwrap().modified().unwrap();

        // reopen and reread data
        let db = FileDb::establish_connection().unwrap();
        let and_back = db.get_by_datetime(data.time).wait().unwrap().unwrap();
        assert!(and_back.mean == mean_u);
        drop(db);
        // we had no changes, so the file should not have been saved again
        let date_no_modification = data_file.metadata().unwrap().modified().unwrap();
        assert_eq!(date_created, date_no_modification);

        // clear db
        clear_db();
        assert!(!data_file.is_file());
        assert!(!lockfile.is_file());

        // reopen and reread data should fail
        let db = FileDb::establish_connection().unwrap();
        assert!(db.get_by_datetime(data.time).wait().unwrap().is_none());
    }


}
