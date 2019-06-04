// Copyright 2018 TiKV Project Authors. Licensed under Apache-2.0.

//! Raw related functionality.
//!
//! Using the [`raw::Client`](raw::Client) you can utilize TiKV's raw interface.
//!
//! This interface offers optimal performance as it does not require coordination with a timestamp
//! oracle, while the transactional interface does.
//!
//! **Warning:** It is not advisable to use both raw and transactional functionality in the same keyspace.
//!
use crate::{rpc::RpcClient, Config, Error, Key, KeyRange, KvPair, Result, Value};
use futures::{future, task::Context, Future, Poll};
use std::{fmt, ops::Bound, pin::Pin, sync::Arc, u32};

const MAX_RAW_KV_SCAN_LIMIT: u32 = 10240;

/// The TiKV raw [`Client`](Client) is used to issue requests to the TiKV server and PD cluster.
pub struct Client {
    rpc: Arc<RpcClient>,
}

impl Client {
    /// Create a new [`Client`](Client) once the [`Connect`](Connect) resolves.
    ///
    /// ```rust,no_run
    /// # #![feature(async_await)]
    /// # use tikv_client::{Config, raw::Client};
    /// # use futures::prelude::*;
    /// # futures::executor::block_on(async {
    /// let connect = Client::connect(Config::default());
    /// let client = connect.await.unwrap();
    /// # });
    /// ```
    pub fn connect(config: Config) -> Connect {
        Connect::new(config)
    }

    #[inline]
    fn rpc(&self) -> Arc<RpcClient> {
        Arc::clone(&self.rpc)
    }

    /// Create a new [`Get`](Get) request.
    ///
    /// Once resolved this request will result in the fetching of the value associated with the
    /// given key.
    ///
    /// ```rust,no_run
    /// # #![feature(async_await)]
    /// # use tikv_client::{Value, Config, raw::Client};
    /// # use futures::prelude::*;
    /// # futures::executor::block_on(async {
    /// # let connecting_client = Client::connect(Config::new(vec!["192.168.0.100", "192.168.0.101"]));
    /// # let connected_client = connecting_client.await.unwrap();
    /// let key = "TiKV";
    /// let req = connected_client.get(key);
    /// let result: Option<Value> = req.await.unwrap();
    /// # });
    /// ```
    pub fn get(&self, key: impl Into<Key>) -> Get {
        Get::new(self.rpc(), GetInner::new(key.into()))
    }

    /// Create a new [`BatchGet`](BatchGet) request.
    ///
    /// Once resolved this request will result in the fetching of the values associated with the
    /// given keys.
    ///
    /// ```rust,no_run
    /// # #![feature(async_await)]
    /// # use tikv_client::{KvPair, Config, raw::Client};
    /// # use futures::prelude::*;
    /// # futures::executor::block_on(async {
    /// # let connecting_client = Client::connect(Config::new(vec!["192.168.0.100", "192.168.0.101"]));
    /// # let connected_client = connecting_client.await.unwrap();
    /// let keys = vec!["TiKV", "TiDB"];
    /// let req = connected_client.batch_get(keys);
    /// let result: Vec<KvPair> = req.await.unwrap();
    /// # });
    /// ```
    pub fn batch_get(&self, keys: impl IntoIterator<Item = impl Into<Key>>) -> BatchGet {
        BatchGet::new(
            self.rpc(),
            BatchGetInner::new(keys.into_iter().map(Into::into).collect()),
        )
    }

    /// Create a new [`Put`](Put) request.
    ///
    /// Once resolved this request will result in the setting of the value associated with the given key.
    ///
    /// ```rust,no_run
    /// # #![feature(async_await)]
    /// # use tikv_client::{Key, Value, Config, raw::Client};
    /// # use futures::prelude::*;
    /// # futures::executor::block_on(async {
    /// # let connecting_client = Client::connect(Config::new(vec!["192.168.0.100", "192.168.0.101"]));
    /// # let connected_client = connecting_client.await.unwrap();
    /// let key = "TiKV";
    /// let val = "TiKV";
    /// let req = connected_client.put(key, val);
    /// let result: () = req.await.unwrap();
    /// # });
    /// ```
    pub fn put(&self, key: impl Into<Key>, value: impl Into<Value>) -> Put {
        Put::new(self.rpc(), PutInner::new(key.into(), value.into()))
    }

    /// Create a new [`BatchPut`](BatchPut) request.
    ///
    /// Once resolved this request will result in the setting of the value associated with the given key.
    ///
    /// ```rust,no_run
    /// # #![feature(async_await)]
    /// # use tikv_client::{Error, Result, KvPair, Key, Value, Config, raw::Client};
    /// # use futures::prelude::*;
    /// # futures::executor::block_on(async {
    /// # let connecting_client = Client::connect(Config::new(vec!["192.168.0.100", "192.168.0.101"]));
    /// # let connected_client = connecting_client.await.unwrap();
    /// let kvpair1 = ("PD", "Go");
    /// let kvpair2 = ("TiKV", "Rust");
    /// let iterable = vec![kvpair1, kvpair2];
    /// let req = connected_client.batch_put(iterable);
    /// let result: () = req.await.unwrap();
    /// # });
    /// ```
    pub fn batch_put(&self, pairs: impl IntoIterator<Item = impl Into<KvPair>>) -> BatchPut {
        BatchPut::new(
            self.rpc(),
            BatchPutInner::new(pairs.into_iter().map(Into::into).collect()),
        )
    }

    /// Create a new [`Delete`](Delete) request.
    ///
    /// Once resolved this request will result in the deletion of the given key.
    ///
    /// ```rust,no_run
    /// # #![feature(async_await)]
    /// # use tikv_client::{Key, Config, raw::Client};
    /// # use futures::prelude::*;
    /// # futures::executor::block_on(async {
    /// # let connecting_client = Client::connect(Config::new(vec!["192.168.0.100", "192.168.0.101"]));
    /// # let connected_client = connecting_client.await.unwrap();
    /// let key = "TiKV";
    /// let req = connected_client.delete(key);
    /// let result: () = req.await.unwrap();
    /// # });
    /// ```
    pub fn delete(&self, key: impl Into<Key>) -> Delete {
        Delete::new(self.rpc(), DeleteInner::new(key.into()))
    }

    /// Create a new [`BatchDelete`](BatchDelete) request.
    ///
    /// Once resolved this request will result in the deletion of the given keys.
    ///
    /// ```rust,no_run
    /// # #![feature(async_await)]
    /// # use tikv_client::{Config, raw::Client};
    /// # use futures::prelude::*;
    /// # futures::executor::block_on(async {
    /// # let connecting_client = Client::connect(Config::new(vec!["192.168.0.100", "192.168.0.101"]));
    /// # let connected_client = connecting_client.await.unwrap();
    /// let keys = vec!["TiKV", "TiDB"];
    /// let req = connected_client.batch_delete(keys);
    /// let result: () = req.await.unwrap();
    /// # });
    /// ```
    pub fn batch_delete(&self, keys: impl IntoIterator<Item = impl Into<Key>>) -> BatchDelete {
        BatchDelete::new(
            self.rpc(),
            BatchDeleteInner::new(keys.into_iter().map(Into::into).collect()),
        )
    }

    /// Create a new [`Scan`](Scan) request.
    ///
    /// Once resolved this request will result in a scanner over the given keys.
    ///
    /// ```rust,no_run
    /// # #![feature(async_await)]
    /// # use tikv_client::{KvPair, Config, raw::Client};
    /// # use futures::prelude::*;
    /// # futures::executor::block_on(async {
    /// # let connecting_client = Client::connect(Config::new(vec!["192.168.0.100", "192.168.0.101"]));
    /// # let connected_client = connecting_client.await.unwrap();
    /// let inclusive_range = "TiKV"..="TiDB";
    /// let req = connected_client.scan(inclusive_range, 2);
    /// let result: Vec<KvPair> = req.await.unwrap();
    /// # });
    /// ```
    pub fn scan(&self, range: impl KeyRange, limit: u32) -> Scan {
        Scan::new(self.rpc(), ScanInner::new(range.into_bounds(), limit))
    }

    /// Create a new [`BatchScan`](BatchScan) request.
    ///
    /// Once resolved this request will result in a set of scanners over the given keys.
    ///
    /// ```rust,no_run
    /// # #![feature(async_await)]
    /// # use tikv_client::{Key, Config, raw::Client};
    /// # use futures::prelude::*;
    /// # futures::executor::block_on(async {
    /// # let connecting_client = Client::connect(Config::new(vec!["192.168.0.100", "192.168.0.101"]));
    /// # let connected_client = connecting_client.await.unwrap();
    /// let inclusive_range1 = "TiDB"..="TiKV";
    /// let inclusive_range2 = "TiKV"..="TiSpark";
    /// let iterable = vec![inclusive_range1, inclusive_range2];
    /// let req = connected_client.batch_scan(iterable, 2);
    /// let result = req.await;
    /// # });
    /// ```
    pub fn batch_scan(
        &self,
        ranges: impl IntoIterator<Item = impl KeyRange>,
        each_limit: u32,
    ) -> BatchScan {
        BatchScan::new(
            self.rpc(),
            BatchScanInner::new(
                ranges.into_iter().map(KeyRange::into_keys).collect(),
                each_limit,
            ),
        )
    }

    /// Create a new [`DeleteRange`](DeleteRange) request.
    ///
    /// Once resolved this request will result in the deletion of all keys over the given range.
    ///
    /// ```rust,no_run
    /// # #![feature(async_await)]
    /// # use tikv_client::{Key, Config, raw::Client};
    /// # use futures::prelude::*;
    /// # futures::executor::block_on(async {
    /// # let connecting_client = Client::connect(Config::new(vec!["192.168.0.100", "192.168.0.101"]));
    /// # let connected_client = connecting_client.await.unwrap();
    /// let inclusive_range = "TiKV"..="TiDB";
    /// let req = connected_client.delete_range(inclusive_range);
    /// let result: () = req.await.unwrap();
    /// # });
    /// ```
    pub fn delete_range(&self, range: impl KeyRange) -> DeleteRange {
        DeleteRange::new(self.rpc(), DeleteRangeInner::new(range.into_keys()))
    }
}

/// An unresolved [`Client`](Client) connection to a TiKV cluster.
///
/// Once resolved it will result in a connected [`Client`](Client).
///
/// ```rust,no_run
/// # #![feature(async_await)]
/// use tikv_client::{Config, raw::{Client, Connect}};
/// use futures::prelude::*;
///
/// # futures::executor::block_on(async {
/// let connect: Connect = Client::connect(Config::default());
/// let client: Client = connect.await.unwrap();
/// # });
/// ```
pub struct Connect {
    config: Config,
}

impl Connect {
    fn new(config: Config) -> Self {
        Connect { config }
    }
}

impl Future for Connect {
    type Output = Result<Client>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Self::Output> {
        let config = &self.config;
        let rpc = Arc::new(RpcClient::connect(config)?);
        Poll::Ready(Ok(Client { rpc }))
    }
}

/// A [`ColumnFamily`](ColumnFamily) is an optional parameter for [`raw::Client`](Client) requests.
///
/// TiKV uses RocksDB's `ColumnFamily` support. You can learn more about RocksDB's `ColumnFamily`s [on their wiki](https://github.com/facebook/rocksdb/wiki/Column-Families).
///
/// By default in TiKV data is stored in three different `ColumnFamily` values, configurable in the TiKV server's configuration:
///
/// * Default: Where real user data is stored. Set by `[rocksdb.defaultcf]`.
/// * Write: Where MVCC and index related data are stored. Set by `[rocksdb.writecf]`.
/// * Lock: Where lock information is stored. Set by `[rocksdb.lockcf]`.
///
/// Not providing a call a `ColumnFamily` means it will use the default value of `default`.
///
/// The best (and only) way to create a [`ColumnFamily`](ColumnFamily) is via the `From` implementation:
///
/// ```rust
/// # use tikv_client::raw::ColumnFamily;
///
/// let cf = ColumnFamily::from("write");
/// let cf = ColumnFamily::from(String::from("write"));
/// let cf = ColumnFamily::from(&String::from("write"));
/// ```
///
/// **But, you should not need to worry about all this:** Many functions which accept a
/// `ColumnFamily` accept an `Into<ColumnFamily>`, which means all of the above types can be passed
/// directly to those functions.
#[derive(Default, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct ColumnFamily(String);

impl<T: Into<String>> From<T> for ColumnFamily {
    fn from(i: T) -> ColumnFamily {
        ColumnFamily(i.into())
    }
}

impl fmt::Display for ColumnFamily {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

type BoxTryFuture<Resp> = Box<dyn Future<Output = Result<Resp>> + Send>;

trait RequestInner: Sized {
    type Resp;

    fn execute(self, client: Arc<RpcClient>, cf: Option<ColumnFamily>) -> BoxTryFuture<Self::Resp>;
}

enum RequestState<Inner>
where
    Inner: RequestInner,
{
    Uninitiated(Option<(Arc<RpcClient>, Inner, Option<ColumnFamily>)>),
    Initiated(BoxTryFuture<Inner::Resp>),
}

impl<Inner> RequestState<Inner>
where
    Inner: RequestInner,
{
    fn new(client: Arc<RpcClient>, inner: Inner) -> Self {
        RequestState::Uninitiated(Some((client, inner, None)))
    }

    fn cf(&mut self, new_cf: impl Into<ColumnFamily>) {
        if let RequestState::Uninitiated(Some((_, _, ref mut cf))) = self {
            cf.replace(new_cf.into());
        }
    }

    fn inner_mut(&mut self) -> Option<&mut Inner> {
        match self {
            RequestState::Uninitiated(Some((_, ref mut inner, _))) => Some(inner),
            _ => None,
        }
    }

    fn assure_initialized<'a>(self: Pin<&'a mut Self>) -> Pin<&'a mut Self> {
        unsafe {
            let mut this = Pin::get_unchecked_mut(self);
            if let RequestState::Uninitiated(state) = &mut this {
                let (client, inner, cf) = state.take().unwrap();
                *this = RequestState::Initiated(inner.execute(client, cf));
            }
            Pin::new_unchecked(this)
        }
    }

    fn assert_init_pin_mut<'a>(
        self: Pin<&'a mut Self>,
    ) -> Pin<&'a mut dyn Future<Output = Result<Inner::Resp>>> {
        unsafe {
            match Pin::get_unchecked_mut(self) {
                RequestState::Initiated(future) => Pin::new_unchecked(&mut **future),
                _ => unreachable!(),
            }
        }
    }

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<Inner::Resp>> {
        self = self.assure_initialized();
        self.assert_init_pin_mut().poll(cx)
    }
}

/// An unresolved [`Client::get`](Client::get) request.
///
/// Once resolved this request will result in the fetching of the value associated with the given
/// key.
pub struct Get {
    state: RequestState<GetInner>,
}

impl Get {
    fn new(client: Arc<RpcClient>, inner: GetInner) -> Self {
        Self {
            state: RequestState::new(client, inner),
        }
    }

    /// Set the (optional) [`ColumnFamily`](ColumnFamily).
    pub fn cf(mut self, cf: impl Into<ColumnFamily>) -> Self {
        self.state.cf(cf);
        self
    }
}

impl Future for Get {
    type Output = Result<Option<Value>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        unsafe { Pin::new_unchecked(&mut Pin::get_unchecked_mut(self).state).poll(cx) }
    }
}

pub(crate) struct GetInner {
    key: Key,
}

impl GetInner {
    fn new(key: Key) -> Self {
        GetInner { key }
    }
}

impl RequestInner for GetInner {
    type Resp = Option<Value>;

    fn execute(
        self,
        client: Arc<RpcClient>,
        cf: Option<ColumnFamily>,
    ) -> BoxTryFuture<Option<Value>> {
        Box::new(client.raw_get(self.key, cf))
    }
}

/// An unresolved [`Client::batch_get`](Client::batch_get) request.
///
/// Once resolved this request will result in the fetching of the values associated with the given
/// keys.
pub struct BatchGet {
    state: RequestState<BatchGetInner>,
}

impl BatchGet {
    fn new(client: Arc<RpcClient>, inner: BatchGetInner) -> Self {
        Self {
            state: RequestState::new(client, inner),
        }
    }

    /// Set the (optional) [`ColumnFamily`](ColumnFamily).
    pub fn cf(mut self, cf: impl Into<ColumnFamily>) -> Self {
        self.state.cf(cf);
        self
    }
}

impl Future for BatchGet {
    type Output = Result<Vec<KvPair>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        unsafe { Pin::new_unchecked(&mut Pin::get_unchecked_mut(self).state).poll(cx) }
    }
}

pub(crate) struct BatchGetInner {
    keys: Vec<Key>,
}

impl RequestInner for BatchGetInner {
    type Resp = Vec<KvPair>;

    fn execute(
        self,
        client: Arc<RpcClient>,
        cf: Option<ColumnFamily>,
    ) -> BoxTryFuture<Vec<KvPair>> {
        Box::new(client.raw_batch_get(self.keys, cf))
    }
}

impl BatchGetInner {
    fn new(keys: Vec<Key>) -> Self {
        BatchGetInner { keys }
    }
}

/// An unresolved [`Client::put`](Client::put) request.
///
/// Once resolved this request will result in the putting of the value associated with the given
/// key.
pub struct Put {
    state: RequestState<PutInner>,
}

impl Put {
    fn new(client: Arc<RpcClient>, inner: PutInner) -> Self {
        Self {
            state: RequestState::new(client, inner),
        }
    }

    /// Set the (optional) [`ColumnFamily`](ColumnFamily).
    pub fn cf(mut self, cf: impl Into<ColumnFamily>) -> Self {
        self.state.cf(cf);
        self
    }
}

impl Future for Put {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        unsafe { Pin::new_unchecked(&mut Pin::get_unchecked_mut(self).state).poll(cx) }
    }
}

pub(crate) struct PutInner {
    key: Key,
    value: Value,
}

impl PutInner {
    fn new(key: Key, value: Value) -> Self {
        PutInner { key, value }
    }
}

impl RequestInner for PutInner {
    type Resp = ();

    fn execute(self, client: Arc<RpcClient>, cf: Option<ColumnFamily>) -> BoxTryFuture<()> {
        let (key, value) = (self.key, self.value);
        Box::new(client.raw_put(key, value, cf))
    }
}

/// An unresolved [`Client::batch_put`](Client::batch_put) request.
///
/// Once resolved this request will result in the setting of the value associated with the given key.
pub struct BatchPut {
    state: RequestState<BatchPutInner>,
}

impl BatchPut {
    fn new(client: Arc<RpcClient>, inner: BatchPutInner) -> Self {
        Self {
            state: RequestState::new(client, inner),
        }
    }

    /// Set the (optional) [`ColumnFamily`](ColumnFamily).
    pub fn cf(mut self, cf: impl Into<ColumnFamily>) -> Self {
        self.state.cf(cf);
        self
    }
}

impl Future for BatchPut {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        unsafe { Pin::new_unchecked(&mut Pin::get_unchecked_mut(self).state).poll(cx) }
    }
}

pub(crate) struct BatchPutInner {
    pairs: Vec<KvPair>,
}

impl BatchPutInner {
    fn new(pairs: Vec<KvPair>) -> Self {
        BatchPutInner { pairs }
    }
}

impl RequestInner for BatchPutInner {
    type Resp = ();

    fn execute(self, client: Arc<RpcClient>, cf: Option<ColumnFamily>) -> BoxTryFuture<()> {
        Box::new(client.raw_batch_put(self.pairs, cf))
    }
}

/// An unresolved [`Client::delete`](Client::delete) request.
///
/// Once resolved this request will result in the deletion of the given key.
pub struct Delete {
    state: RequestState<DeleteInner>,
}

impl Delete {
    fn new(client: Arc<RpcClient>, inner: DeleteInner) -> Self {
        Self {
            state: RequestState::new(client, inner),
        }
    }

    /// Set the (optional) [`ColumnFamily`](ColumnFamily).
    pub fn cf(mut self, cf: impl Into<ColumnFamily>) -> Self {
        self.state.cf(cf);
        self
    }
}

impl Future for Delete {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        unsafe { Pin::new_unchecked(&mut Pin::get_unchecked_mut(self).state).poll(cx) }
    }
}

pub(crate) struct DeleteInner {
    key: Key,
}

impl DeleteInner {
    fn new(key: Key) -> Self {
        DeleteInner { key }
    }
}

impl RequestInner for DeleteInner {
    type Resp = ();

    fn execute(self, client: Arc<RpcClient>, cf: Option<ColumnFamily>) -> BoxTryFuture<()> {
        Box::new(client.raw_delete(self.key, cf))
    }
}

/// An unresolved [`Client::batch_delete`](Client::batch_delete) request.
///
/// Once resolved this request will result in the deletion of the given keys.
pub struct BatchDelete {
    state: RequestState<BatchDeleteInner>,
}

impl BatchDelete {
    fn new(client: Arc<RpcClient>, inner: BatchDeleteInner) -> Self {
        Self {
            state: RequestState::new(client, inner),
        }
    }

    /// Set the (optional) [`ColumnFamily`](ColumnFamily).
    pub fn cf(mut self, cf: impl Into<ColumnFamily>) -> Self {
        self.state.cf(cf);
        self
    }
}

impl Future for BatchDelete {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        unsafe { Pin::new_unchecked(&mut Pin::get_unchecked_mut(self).state).poll(cx) }
    }
}

pub(crate) struct BatchDeleteInner {
    keys: Vec<Key>,
}

impl BatchDeleteInner {
    fn new(keys: Vec<Key>) -> Self {
        BatchDeleteInner { keys }
    }
}

impl RequestInner for BatchDeleteInner {
    type Resp = ();

    fn execute(self, client: Arc<RpcClient>, cf: Option<ColumnFamily>) -> BoxTryFuture<()> {
        Box::new(client.raw_batch_delete(self.keys, cf))
    }
}

pub(crate) struct ScanInner {
    range: (Bound<Key>, Bound<Key>),
    limit: u32,
    key_only: bool,
}

impl ScanInner {
    fn new(range: (Bound<Key>, Bound<Key>), limit: u32) -> Self {
        ScanInner {
            range,
            limit,
            key_only: false,
        }
    }
}

impl RequestInner for ScanInner {
    type Resp = Vec<KvPair>;

    fn execute(
        self,
        client: Arc<RpcClient>,
        cf: Option<ColumnFamily>,
    ) -> BoxTryFuture<Vec<KvPair>> {
        if self.limit > MAX_RAW_KV_SCAN_LIMIT {
            Box::new(future::err(Error::max_scan_limit_exceeded(
                self.limit,
                MAX_RAW_KV_SCAN_LIMIT,
            )))
        } else {
            let keys = match self.range.into_keys() {
                Err(e) => return Box::new(future::err(e)),
                Ok(v) => v,
            };
            Box::new(client.raw_scan(keys, self.limit, self.key_only, cf))
        }
    }
}

/// An unresolved [`Client::scan`](Client::scan) request.
///
/// Once resolved this request will result in a scanner over the given range.
pub struct Scan {
    state: RequestState<ScanInner>,
}

impl Scan {
    fn new(client: Arc<RpcClient>, inner: ScanInner) -> Self {
        Scan {
            state: RequestState::new(client, inner),
        }
    }

    /// Set the (optional) [`ColumnFamily`](ColumnFamily).
    pub fn cf(mut self, cf: impl Into<ColumnFamily>) -> Self {
        self.state.cf(cf);
        self
    }

    pub fn key_only(mut self) -> Self {
        if let Some(x) = self.state.inner_mut() {
            x.key_only = true;
        };
        self
    }
}

impl Future for Scan {
    type Output = Result<Vec<KvPair>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        unsafe { Pin::new_unchecked(&mut Pin::get_unchecked_mut(self).state).poll(cx) }
    }
}

pub(crate) struct BatchScanInner {
    ranges: Vec<Result<(Key, Option<Key>)>>,
    each_limit: u32,
    key_only: bool,
}

impl BatchScanInner {
    fn new(ranges: Vec<Result<(Key, Option<Key>)>>, each_limit: u32) -> Self {
        BatchScanInner {
            ranges,
            each_limit,
            key_only: false,
        }
    }
}

impl RequestInner for BatchScanInner {
    type Resp = Vec<KvPair>;

    fn execute(
        self,
        client: Arc<RpcClient>,
        cf: Option<ColumnFamily>,
    ) -> BoxTryFuture<Vec<KvPair>> {
        if self.each_limit > MAX_RAW_KV_SCAN_LIMIT {
            Box::new(future::err(Error::max_scan_limit_exceeded(
                self.each_limit,
                MAX_RAW_KV_SCAN_LIMIT,
            )))
        } else if self.ranges.iter().any(Result::is_err) {
            // All errors must be InvalidKeyRange so we can simply return a new InvalidKeyRange
            Box::new(future::err(Error::invalid_key_range()))
        } else {
            Box::new(client.raw_batch_scan(
                self.ranges.into_iter().map(Result::unwrap).collect(),
                self.each_limit,
                self.key_only,
                cf,
            ))
        }
    }
}

/// An unresolved [`Client::batch_scan`](Client::batch_scan) request.
///
/// Once resolved this request will result in a scanner over the given ranges.
pub struct BatchScan {
    state: RequestState<BatchScanInner>,
}

impl BatchScan {
    fn new(client: Arc<RpcClient>, inner: BatchScanInner) -> Self {
        BatchScan {
            state: RequestState::new(client, inner),
        }
    }

    /// Set the (optional) [`ColumnFamily`](ColumnFamily).
    pub fn cf(mut self, cf: impl Into<ColumnFamily>) -> Self {
        self.state.cf(cf);
        self
    }

    pub fn key_only(mut self) -> Self {
        if let Some(x) = self.state.inner_mut() {
            x.key_only = true;
        };
        self
    }
}

impl Future for BatchScan {
    type Output = Result<Vec<KvPair>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        unsafe { Pin::new_unchecked(&mut Pin::get_unchecked_mut(self).state).poll(cx) }
    }
}

/// An unresolved [`Client::delete_range`](Client::delete_range) request.
///
/// Once resolved this request will result in the deletion of the values in the given
/// range.
pub struct DeleteRange {
    state: RequestState<DeleteRangeInner>,
}

impl DeleteRange {
    fn new(client: Arc<RpcClient>, inner: DeleteRangeInner) -> Self {
        Self {
            state: RequestState::new(client, inner),
        }
    }

    /// Set the (optional) [`ColumnFamily`](ColumnFamily).
    pub fn cf(mut self, cf: impl Into<ColumnFamily>) -> Self {
        self.state.cf(cf);
        self
    }
}

impl Future for DeleteRange {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        unsafe { Pin::new_unchecked(&mut Pin::get_unchecked_mut(self).state).poll(cx) }
    }
}

pub(crate) struct DeleteRangeInner {
    range: Result<(Key, Option<Key>)>,
}

impl DeleteRangeInner {
    fn new(range: Result<(Key, Option<Key>)>) -> Self {
        DeleteRangeInner { range }
    }
}

impl RequestInner for DeleteRangeInner {
    type Resp = ();

    fn execute(self, client: Arc<RpcClient>, cf: Option<ColumnFamily>) -> BoxTryFuture<()> {
        match self.range {
            Ok(range) => Box::new(client.raw_delete_range(range, cf)),
            Err(e) => Box::new(future::err(e)),
        }
    }
}
