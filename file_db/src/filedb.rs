use std::str::FromStr;
use std::hash::Hash;
use std::env;
use std::fmt::Display;
use std::sync::{Mutex, MutexGuard};
use std::sync::Arc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs::{self, read_dir, File};
use std::io::{BufReader, BufWriter};
use std::io::prelude::*;
use std::marker::PhantomData;

use futures::{future, Future};
use futures_cpupool::{CpuFuture, CpuPool};

use failure::{Error, ResultExt};

use crate::lock::ExclusiveFilesystembasedLock;

use rmp_serde::{Deserializer as MsgPackDeserializer, Serializer as MsgPackSerializer};
use serde::Serialize;
use serde::de::{Deserialize, DeserializeOwned};

use typemap::{Key as TypeMapKey, ShareMap, TypeMap};


pub trait ToFilenamePart {
    fn to_filename_part(&self) -> String;
}

/// Helper to wrap the `::Value` of a user defined TypeMap key with an `Arc<>`
struct ArcedCacheType<InnerKey: TypeMapKey>(PhantomData<InnerKey>);

impl<InnerKey: TypeMapKey> TypeMapKey for ArcedCacheType<InnerKey> {
    type Value = Arc<InnerKey::Value>;
}

///
/// Data that will be saved into the file_db must implement this trait.
///
/// For example timestamped data can use its timestamp as key and
/// its date as chunkid.
///
pub trait ChunkableData
    : Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    type Key: Ord + Send + Sync + Copy;
    type ChunkKey: Eq
        + Ord
        + ToFilenamePart
        + Hash
        + Display
        + FromStr
        + From<Self::Key>
        + Send
        + Copy
        + Clone;
    fn chunk_key(&self) -> Self::ChunkKey;
    fn key(&self) -> Self::Key;
    /// A hint for `Vec::with_capacity()`
    fn estimate_keys_per_chunk() -> usize {
        10000
    }
    /// A hint for `Vec::with_capacity()`
    fn estimate_serialized_bytes_per_chunk() -> usize {
        1_000_000
    }
}

/// Chunks are groups of data. (Grouped by `ChunkabeData::chunk_key()`)
/// Each chunk is saved as a seperate file.
struct Chunk<CData: ChunkableData> {
    /// The absolute path to this chunk's file.
    path: PathBuf,
    /// True, if the file `self.path` is up to date with `self.data`.
    disk_is_up_to_date: bool,
    /// If `data.is_some()`, then `data` is always newer
    /// than the correspondent file `self.path` on disk.
    data: Option<Arc<Vec<CData>>>,

    /// User defined data that is derived from `self.data`.
    /// Any data in `self.cache` is always on par with data.
    /// (In other words: On data update, the cache is cleared.)
    cache: ShareMap,
}

/// The Hashmap from ChunkKeys to chunks.
/// Each chunk is mutexed.
type Chunks<CData/*: ChunkableData [lint: not enforced anyway]*/>
     = HashMap<<CData as ChunkableData>::ChunkKey, Arc<Mutex<Chunk<CData>>>>;

/// The database type.
/// Filedb stores chunked data into a folder addressable by key and chunk key.
pub struct FileDb<CData: ChunkableData> {
    chunks: Arc<Mutex<Chunks<CData>>>,
    path_base: PathBuf,
    pool: CpuPool,
    lock: ExclusiveFilesystembasedLock,
    version_postfix: &'static str,
}



impl<CData: ChunkableData> Chunk<CData> {
    /// Creates a new unsaved and empty chunk of data.
    fn new(p: PathBuf) -> Chunk<CData> {
        Chunk {
            path: p,
            disk_is_up_to_date: false,
            data: Some(Arc::new(vec![])),
            cache: TypeMap::custom(),
        }
    }

    /// Loads this chunk lazily. Disk is accessed when the first read or write is requested.
    fn load(p: PathBuf) -> Chunk<CData> {
        Chunk {
            path: p,
            disk_is_up_to_date: true,
            data: None,
            cache: TypeMap::custom(),
        }
    }

    /// Loads the data from the file to memory, if it is not already there.
    fn force_to_memory(&mut self) -> Result<&mut Arc<Vec<CData>>, Error> {
        if let Some(ref mut data) = self.data {
            Ok(data)
        } else {
            if self.disk_is_up_to_date {
                let file = File::open(&self.path)?;
                let size = file.metadata()
                    .map(|m| m.len())
                    .unwrap_or(CData::estimate_serialized_bytes_per_chunk() as u64);
                let mut buffered_file = BufReader::new(file);
                let mut contents = Vec::with_capacity(size as usize);
                buffered_file.read_to_end(&mut contents)?;

                let mut de = MsgPackDeserializer::new(&contents[..]);
                let desered: Vec<CData> = Deserialize::deserialize(&mut de)?;

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
                let file = File::create(&self.path)?;
                let mut buffered_file = BufWriter::new(file);
                self.force_to_memory()?
                    .serialize(&mut MsgPackSerializer::new(&mut buffered_file))?;
            }

            self.disk_is_up_to_date = true;
        }
        Ok(())
    }


    pub fn update<F, R>(&mut self, f: F) -> Result<R, Error>
    where
        F: FnOnce(&mut Vec<CData>) -> R,
    {
        self.update_no_sort(|mut data| {
            let r = f(&mut data);
            data.sort_by_key(|element| element.key());
            r
        })
    }

    fn update_no_sort<F, R>(&mut self, f: F) -> Result<R, Error>
    where
        F: FnOnce(&mut Vec<CData>) -> R,
    {
        let r = f(Arc::make_mut(self.force_to_memory()?));
        // self.serialized = None;
        self.disk_is_up_to_date = false;
        self.cache.clear();
        Ok(r)
    }

    pub fn get_shared_vec(&mut self) -> Result<Arc<Vec<CData>>, Error> {
        Ok(self.force_to_memory()?.clone())
    }


    pub fn custom_cached<K: TypeMapKey>(
        &mut self,
        f: Box<dyn Fn(&[CData]) -> K::Value>,
    ) -> Result<Arc<K::Value>, Error>
    where
        K::Value: Send + Sync,
    {
        if let Some(val) = self.cache.get::<ArcedCacheType<K>>() {
            return Ok(Arc::clone(val));
        }

        let new_val = Arc::new(f(&self.force_to_memory()?[..]));
        self.cache.insert::<ArcedCacheType<K>>(new_val);

        Ok(Arc::clone(self.cache.get::<ArcedCacheType<K>>().unwrap()))
    }

    pub fn get_by_key(&mut self, key: CData::Key) -> Result<Option<CData>, Error> {
        let data = self.force_to_memory()?;
        match data.binary_search_by_key(&key, |data| data.key()) {
            Ok(index) => Ok(data.get(index).cloned()),
            Err(_would_be_insert) => Ok(None),
        }
    }

    pub fn insert_or_update(&mut self, data: CData) -> Result<(), Error> {
        self.update_no_sort(|vec| {
            let pos = vec.binary_search_by_key(&data.key(), |data| data.key());
            match pos {
                Ok(index) => vec[index] = data,
                Err(would_be_insert) => vec.insert(would_be_insert, data),
            }
        })
    }

    pub fn len(&mut self) -> Result<usize, Error> {
        self.force_to_memory().map(|c| c.len())
    }
}





impl<CData: ChunkableData> FileDb<CData> {
    pub fn new_from_env(
        env_var_name: &str,
        num_threads: usize,
        version_postfix: &'static str,
    ) -> Result<FileDb<CData>, Error> {
        let database_url: PathBuf = env::var(env_var_name)
            .with_context(|_e| {
                format!("Environment variable {} must be set.", env_var_name)
            })?
            .into();

        Self::new(database_url, num_threads, version_postfix)
    }


    pub fn new<P: AsRef<Path>>(
        database_url: P,
        num_threads: usize,
        version_postfix: &'static str,
    ) -> Result<FileDb<CData>, Error> {
        let lock =
            ExclusiveFilesystembasedLock::try_set_lock(database_url.as_ref().join("pid.dblock"))?;

        let mut chunks = HashMap::new();
        for (path, existing_date) in Self::get_existing_files(&database_url, version_postfix)? {
            chunks.insert(existing_date, Arc::new(Mutex::new(Chunk::load(path))));
        }

        Ok(FileDb {
            pool: CpuPool::new(num_threads),
            chunks: Arc::new(Mutex::new(chunks)),
            path_base: database_url.as_ref().to_path_buf(),
            lock,
            version_postfix,
        })
    }

    fn get_filepath<P: AsRef<Path>>(
        chunk_key: CData::ChunkKey,
        path_base: P,
        version_postfix: &'static str,
    ) -> PathBuf {
        path_base.as_ref().join(Path::new(&format!(
            "chunk-{}.db.{}",
            &chunk_key.to_filename_part(),
            version_postfix
        )))
    }

    fn get_existing_files<P: AsRef<Path>>(
        path_base: P,
        version_postfix: &'static str,
    ) -> Result<Vec<(PathBuf, CData::ChunkKey)>, Error> {
        let filename_end = format!(".db.{}", version_postfix);

        let mut files = vec![];
        for entry in read_dir(&path_base).context("Database path is not a directory")? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let filename = path.file_name()
                    .expect("File has no filename?!")
                    .to_string_lossy();

                // Do not use regex for something this simple

                if filename.starts_with("chunk-") && filename.ends_with(&filename_end[..]) {
                    if filename.len() > 6 + filename_end.len() {
                        let end = filename.len() - filename_end.len();
                        let chunk_key_str = &filename[6..end];

                        if let Ok(chunk_key) = CData::ChunkKey::from_str(&chunk_key_str) {
                            let control = Self::get_filepath(chunk_key, &path_base, version_postfix);
                            if control != path {
                                bail!("Extracted {}, but path was {:?}", chunk_key, path);
                            }
                            files.push((control, chunk_key));
                        }
                    }
                }
            }
        }
        Ok(files)
    }

    fn get_or_create_by_chunk_key<P: AsRef<Path>>(
        this: &Arc<Mutex<Chunks<CData>>>,
        chunk_key: CData::ChunkKey,
        path_base: P,
        version_postfix: &'static str,
    ) -> Arc<Mutex<Chunk<CData>>> {
        // chunks is a Hasmap
        // chunk is a vector
        let mut chunks: MutexGuard<Chunks<CData>> = this.lock().unwrap();
        if chunks.contains_key(&chunk_key) {
            chunks.get(&chunk_key).unwrap().clone()
        } else {
            let new_chunk = Arc::new(Mutex::new(Chunk::new(
                Self::get_filepath(chunk_key, &path_base, version_postfix),
            )));
            chunks.insert(chunk_key, Arc::clone(&new_chunk));
            new_chunk
        }
    }

    pub fn save_all(&self) -> CpuFuture<(), Error> {
        let chunks = self.chunks.clone();

        let f: Box<dyn Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let chunks = chunks.lock().unwrap();
            for (ref _chunk_key, ref chunk) in (*chunks).iter() {
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

    pub fn insert_or_update_async(&self, data: CData) -> CpuFuture<CData, Error> {
        let chunks = self.chunks.clone();
        let path_base = self.path_base.clone();
        let version_postfix = self.version_postfix;

        let f: Box<dyn Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let chunk = Self::get_or_create_by_chunk_key(
                &chunks,
                data.chunk_key(),
                path_base,
                version_postfix,
            );
            let r = chunk
                .lock()
                .unwrap()
                .insert_or_update(data.clone())
                .map(|_| data);
            Self::result_to_future_with_context(r, "Could not insert or update")
        }));

        self.pool.spawn(f)
    }


    pub fn custom_cached_by_chunk_key_async<K: TypeMapKey>(
        &self,
        chunk_key: CData::ChunkKey,
        filter_function: Box<dyn Fn(&[CData]) -> K::Value + Send + Sync + 'static>,
    ) -> CpuFuture<Arc<K::Value>, Error>
    where
        K::Value: Send + Sync,
    {
        let chunks = self.chunks.clone();
        let path_base = self.path_base.clone();
        let version_postfix = self.version_postfix;

        let f: Box<dyn Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let chunk = Self::get_or_create_by_chunk_key(&chunks, chunk_key, path_base, version_postfix);
            let r = chunk.lock().unwrap().custom_cached::<K>(filter_function);
            Self::result_to_future_with_context(r, "Could not serialize special")
        }));

        self.pool.spawn(f)
    }


    pub fn get_by_chunk_key_async(
        &self,
        chunk_key: CData::ChunkKey,
    ) -> CpuFuture<Arc<Vec<CData>>, Error> {
        let chunks = self.chunks.clone();
        let path_base = self.path_base.clone();
        let version_postfix = self.version_postfix;

        let f: Box<dyn Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let chunk = Self::get_or_create_by_chunk_key(&chunks, chunk_key, path_base, version_postfix);
            let r = chunk.lock().unwrap().get_shared_vec();
            Self::result_to_future_with_context(r, "Could not get by chunk_key")
        }));

        self.pool.spawn(f)
    }

    pub fn get_by_key_async(&self, key: CData::Key) -> CpuFuture<Option<CData>, Error> {
        let chunks = self.chunks.clone();
        let path_base = self.path_base.clone();
        let version_postfix = self.version_postfix;

        let f: Box<dyn Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let chunk = Self::get_or_create_by_chunk_key(&chunks, key.into(), path_base, version_postfix);
            let r = chunk.lock().unwrap().get_by_key(key);
            Self::result_to_future_with_context(r, "Could not get by datetime")
        }));

        self.pool.spawn(f)
    }

    pub fn get_non_empty_chunk_keys_async(&self) -> CpuFuture<Vec<CData::ChunkKey>, Error> {
        let chunks = self.chunks.clone();

        let f: Box<dyn Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let chunks = chunks.lock().unwrap();
            let mut chunk_keys = Vec::with_capacity(chunks.len());

            for (chunk_key, chunk) in chunks.iter() {
                // Test: skip len() check because this would normally force all chunks to memory
                // let len = match chunk.lock().unwrap().len() {
                //     Ok(len) => len,
                //     Err(e) => {
                //         return future::err(
                //             Error::from(e)
                //                 .context("Could not get data len of chunk.")
                //                 .into(),
                //         )
                //     }
                // };
                //if len > 0 {
                    chunk_keys.push(*chunk_key);
                //}
            }
            chunk_keys.sort();
            future::ok(chunk_keys)
        }));

        self.pool.spawn(f)
    }

    pub fn clear_all_data_ondisk_and_in_memory(self) -> Result<(), Error> {
        let mut chunks = self.chunks.lock().unwrap();
        for (chunk_key, chunk) in chunks.drain() {
            fs::remove_file(Self::get_filepath(
                chunk_key,
                &self.path_base,
                self.version_postfix,
            ))?;
        }
        Ok(())
    }
}

impl<CData: ChunkableData> Drop for FileDb<CData> {
    fn drop(&mut self) {
        self.save_all().wait().map_err(|e| error!("{}", e)).ok();
        // Lock will be released afterwards.
    }
}
