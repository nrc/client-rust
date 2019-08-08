// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use crate::{
    kv_client::{HasError, KvClient, KvRawRequest, RpcFnType, Store},
    pd::PdClient,
    raw::ColumnFamily,
    BoundRange, Error, Key, KvPair, Result, Value,
};

use futures::future::BoxFuture;
use futures::prelude::*;
use futures::stream::BoxStream;
use kvproto::kvrpcpb;
use kvproto::tikvpb::TikvClient;
use std::mem;
use std::sync::Arc;

pub trait RawRequest: Sync + Send + 'static + Sized + Clone {
    type Result;
    type RpcRequest;
    type RpcResponse: HasError + Clone + Send + 'static;
    type KeyType;
    const REQUEST_NAME: &'static str;
    const RPC_FN: RpcFnType<Self::RpcRequest, Self::RpcResponse>;

    fn execute(
        mut self,
        pd_client: Arc<impl PdClient>,
    ) -> BoxFuture<'static, Result<Self::Result>> {
        let stores = self.store_stream(pd_client);
        Self::reduce(
            stores
                .and_then(move |(key, store)| {
                    let request = self.clone().into_request(key, &store);
                    store.dispatch::<Self>(&request, store.call_options())
                })
                .map_ok(move |r| Self::map_result(r))
                .boxed(),
        )
    }

    fn store_stream<PdC: PdClient>(
        &mut self,
        pd_client: Arc<PdC>,
    ) -> BoxStream<'static, Result<(Self::KeyType, Store<PdC::KvClient>)>>;

    fn into_request<KvC: KvClient>(
        self,
        key: Self::KeyType,
        store: &Store<KvC>,
    ) -> Self::RpcRequest;

    fn map_result(result: Self::RpcResponse) -> Self::Result;

    fn reduce(
        results: BoxStream<'static, Result<Self::Result>>,
    ) -> BoxFuture<'static, Result<Self::Result>>;
}

#[derive(Clone)]
pub struct RawGet {
    pub key: Key,
    pub cf: Option<ColumnFamily>,
}

impl RawRequest for RawGet {
    type Result = Option<Value>;
    type RpcRequest = kvrpcpb::RawGetRequest;
    type RpcResponse = kvrpcpb::RawGetResponse;
    type KeyType = Key;
    const REQUEST_NAME: &'static str = "raw_get";
    const RPC_FN: RpcFnType<Self::RpcRequest, Self::RpcResponse> = TikvClient::raw_get_async_opt;

    fn into_request<KvC: KvClient>(
        self,
        key: Self::KeyType,
        store: &Store<KvC>,
    ) -> Self::RpcRequest {
        let mut req = store.request::<Self::RpcRequest>();
        req.set_key(key.into());
        req.maybe_set_cf(self.cf);

        req
    }

    fn store_stream<PdC: PdClient>(
        &mut self,
        pd_client: Arc<PdC>,
    ) -> BoxStream<'static, Result<(Self::KeyType, Store<PdC::KvClient>)>> {
        let key = self.key.clone();
        pd_client
            .store_for_key(&self.key)
            .map_ok(move |store| (key, store))
            .into_stream()
            .boxed()
    }

    fn map_result(mut resp: Self::RpcResponse) -> Self::Result {
        let result: Value = resp.take_value().into();
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    fn reduce(
        results: BoxStream<'static, Result<Self::Result>>,
    ) -> BoxFuture<'static, Result<Self::Result>> {
        results
            .into_future()
            .map(|(f, _)| f.expect("no results should be impossible"))
            .boxed()
    }
}

#[derive(Clone)]
pub struct RawBatchGet {
    pub keys: Vec<Key>,
    pub cf: Option<ColumnFamily>,
}

impl RawRequest for RawBatchGet {
    type Result = Vec<KvPair>;
    type RpcRequest = kvrpcpb::RawBatchGetRequest;
    type RpcResponse = kvrpcpb::RawBatchGetResponse;
    type KeyType = Vec<Key>;
    const REQUEST_NAME: &'static str = "raw_batch_get";
    const RPC_FN: RpcFnType<Self::RpcRequest, Self::RpcResponse> =
        TikvClient::raw_batch_get_async_opt;

    fn into_request<KvC: KvClient>(
        self,
        keys: Self::KeyType,
        store: &Store<KvC>,
    ) -> Self::RpcRequest {
        let mut req = store.request::<Self::RpcRequest>();
        req.set_keys(keys.into_iter().map(Into::into).collect());
        req.maybe_set_cf(self.cf);

        req
    }

    fn store_stream<PdC: PdClient>(
        &mut self,
        pd_client: Arc<PdC>,
    ) -> BoxStream<'static, Result<(Self::KeyType, Store<PdC::KvClient>)>> {
        let mut keys = Vec::new();
        mem::swap(&mut keys, &mut self.keys);

        pd_client
            .clone()
            .group_keys_by_region(keys.into_iter())
            .and_then(move |(region_id, key)| {
                pd_client
                    .clone()
                    .store_for_id(region_id)
                    .map_ok(move |store| (key, store))
            })
            .boxed()
    }

    fn map_result(mut resp: Self::RpcResponse) -> Self::Result {
        resp.take_pairs().into_iter().map(Into::into).collect()
    }

    fn reduce(
        results: BoxStream<'static, Result<Self::Result>>,
    ) -> BoxFuture<'static, Result<Self::Result>> {
        results.try_concat().boxed()
    }
}

#[derive(Clone)]
pub struct RawPut {
    pub key: Key,
    pub value: Value,
    pub cf: Option<ColumnFamily>,
}

impl RawPut {
    pub fn new(
        key: impl Into<Key>,
        value: impl Into<Value>,
        cf: &Option<ColumnFamily>,
    ) -> Result<RawPut> {
        let value = value.into();
        if value.is_empty() {
            return Err(Error::empty_value());
        }

        let key = key.into();
        Ok(RawPut {
            key,
            value,
            cf: cf.clone(),
        })
    }
}

impl RawRequest for RawPut {
    type Result = ();
    type RpcRequest = kvrpcpb::RawPutRequest;
    type RpcResponse = kvrpcpb::RawPutResponse;
    type KeyType = KvPair;
    const REQUEST_NAME: &'static str = "raw_put";
    const RPC_FN: RpcFnType<Self::RpcRequest, Self::RpcResponse> = TikvClient::raw_put_async_opt;

    fn into_request<KvC: KvClient>(
        self,
        key: Self::KeyType,
        store: &Store<KvC>,
    ) -> Self::RpcRequest {
        let mut req = store.request::<Self::RpcRequest>();
        req.set_key(key.0.into());
        req.set_value(key.1.into());
        req.maybe_set_cf(self.cf);

        req
    }

    fn store_stream<PdC: PdClient>(
        &mut self,
        pd_client: Arc<PdC>,
    ) -> BoxStream<'static, Result<(Self::KeyType, Store<PdC::KvClient>)>> {
        let kv = (self.key.clone(), self.value.clone()).into();
        pd_client
            .store_for_key(&self.key)
            .map_ok(move |store| (kv, store))
            .into_stream()
            .boxed()
    }

    fn map_result(_: Self::RpcResponse) -> Self::Result {}

    fn reduce(
        results: BoxStream<'static, Result<Self::Result>>,
    ) -> BoxFuture<'static, Result<Self::Result>> {
        results
            .into_future()
            .map(|(f, _)| f.expect("no results should be impossible"))
            .boxed()
    }
}

#[derive(Clone)]
pub struct RawBatchPut {
    pub pairs: Vec<KvPair>,
    pub cf: Option<ColumnFamily>,
}

impl RawBatchPut {
    pub fn new(
        pairs: impl IntoIterator<Item = impl Into<KvPair>>,
        cf: &Option<ColumnFamily>,
    ) -> Result<RawBatchPut> {
        let pairs: Vec<KvPair> = pairs.into_iter().map(Into::into).collect();
        if pairs.iter().any(|pair| pair.value().is_empty()) {
            return Err(Error::empty_value());
        }

        Ok(RawBatchPut {
            pairs,
            cf: cf.clone(),
        })
    }
}

impl RawRequest for RawBatchPut {
    type Result = ();
    type RpcRequest = kvrpcpb::RawBatchPutRequest;
    type RpcResponse = kvrpcpb::RawBatchPutResponse;
    type KeyType = Vec<KvPair>;
    const REQUEST_NAME: &'static str = "raw_batch_put";
    const RPC_FN: RpcFnType<Self::RpcRequest, Self::RpcResponse> =
        TikvClient::raw_batch_put_async_opt;

    fn into_request<KvC: KvClient>(
        self,
        pairs: Self::KeyType,
        store: &Store<KvC>,
    ) -> Self::RpcRequest {
        let mut req = store.request::<Self::RpcRequest>();
        req.set_pairs(pairs.into_iter().map(Into::into).collect());
        req.maybe_set_cf(self.cf);

        req
    }

    fn store_stream<PdC: PdClient>(
        &mut self,
        pd_client: Arc<PdC>,
    ) -> BoxStream<'static, Result<(Self::KeyType, Store<PdC::KvClient>)>> {
        let mut pairs = Vec::new();
        mem::swap(&mut pairs, &mut self.pairs);

        pd_client
            .clone()
            .group_keys_by_region(pairs.into_iter())
            .and_then(move |(region_id, pair)| {
                pd_client
                    .clone()
                    .store_for_id(region_id)
                    .map_ok(move |store| (pair, store))
            })
            .boxed()
    }

    fn map_result(_: Self::RpcResponse) -> Self::Result {}

    fn reduce(
        results: BoxStream<'static, Result<Self::Result>>,
    ) -> BoxFuture<'static, Result<Self::Result>> {
        results.try_collect().boxed()
    }
}

#[derive(Clone)]
pub struct RawDelete {
    pub key: Key,
    pub cf: Option<ColumnFamily>,
}

impl RawRequest for RawDelete {
    type Result = ();
    type RpcRequest = kvrpcpb::RawDeleteRequest;
    type RpcResponse = kvrpcpb::RawDeleteResponse;
    type KeyType = Key;
    const REQUEST_NAME: &'static str = "raw_delete";
    const RPC_FN: RpcFnType<Self::RpcRequest, Self::RpcResponse> = TikvClient::raw_delete_async_opt;

    fn into_request<KvC: KvClient>(
        self,
        key: Self::KeyType,
        store: &Store<KvC>,
    ) -> Self::RpcRequest {
        let mut req = store.request::<Self::RpcRequest>();
        req.set_key(key.into());
        req.maybe_set_cf(self.cf);

        req
    }

    fn store_stream<PdC: PdClient>(
        &mut self,
        pd_client: Arc<PdC>,
    ) -> BoxStream<'static, Result<(Self::KeyType, Store<PdC::KvClient>)>> {
        let key = self.key.clone();
        pd_client
            .store_for_key(&self.key)
            .map_ok(move |store| (key, store))
            .into_stream()
            .boxed()
    }

    fn map_result(_: Self::RpcResponse) -> Self::Result {}

    fn reduce(
        results: BoxStream<'static, Result<Self::Result>>,
    ) -> BoxFuture<'static, Result<Self::Result>> {
        results
            .into_future()
            .map(|(f, _)| f.expect("no results should be impossible"))
            .boxed()
    }
}

#[derive(Clone)]
pub struct RawBatchDelete {
    pub keys: Vec<Key>,
    pub cf: Option<ColumnFamily>,
}

impl RawRequest for RawBatchDelete {
    type Result = ();
    type RpcRequest = kvrpcpb::RawBatchDeleteRequest;
    type RpcResponse = kvrpcpb::RawBatchDeleteResponse;
    type KeyType = Vec<Key>;
    const REQUEST_NAME: &'static str = "raw_batch_delete";
    const RPC_FN: RpcFnType<Self::RpcRequest, Self::RpcResponse> =
        TikvClient::raw_batch_delete_async_opt;

    fn into_request<KvC: KvClient>(
        self,
        keys: Self::KeyType,
        store: &Store<KvC>,
    ) -> Self::RpcRequest {
        let mut req = store.request::<Self::RpcRequest>();
        req.set_keys(keys.into_iter().map(Into::into).collect());
        req.maybe_set_cf(self.cf);

        req
    }

    fn store_stream<PdC: PdClient>(
        &mut self,
        pd_client: Arc<PdC>,
    ) -> BoxStream<'static, Result<(Self::KeyType, Store<PdC::KvClient>)>> {
        let mut keys = Vec::new();
        mem::swap(&mut keys, &mut self.keys);

        pd_client
            .clone()
            .group_keys_by_region(keys.into_iter())
            .and_then(move |(region_id, key)| {
                pd_client
                    .clone()
                    .store_for_id(region_id)
                    .map_ok(move |store| (key, store))
            })
            .boxed()
    }

    fn map_result(_: Self::RpcResponse) -> Self::Result {}

    fn reduce(
        results: BoxStream<'static, Result<Self::Result>>,
    ) -> BoxFuture<'static, Result<Self::Result>> {
        results.try_collect().boxed()
    }
}

#[derive(Clone)]
pub struct RawDeleteRange {
    pub range: BoundRange,
    pub cf: Option<ColumnFamily>,
}

impl RawRequest for RawDeleteRange {
    type Result = ();
    type RpcRequest = kvrpcpb::RawDeleteRangeRequest;
    type RpcResponse = kvrpcpb::RawDeleteRangeResponse;
    type KeyType = (Key, Key);
    const REQUEST_NAME: &'static str = "raw_delete_range";
    const RPC_FN: RpcFnType<Self::RpcRequest, Self::RpcResponse> =
        TikvClient::raw_delete_range_async_opt;

    fn into_request<KvC: KvClient>(
        self,
        (start_key, end_key): Self::KeyType,
        store: &Store<KvC>,
    ) -> Self::RpcRequest {
        let mut req = store.request::<Self::RpcRequest>();
        req.set_start_key(start_key.into());
        req.set_end_key(end_key.into());
        req.maybe_set_cf(self.cf);

        req
    }

    fn store_stream<PdC: PdClient>(
        &mut self,
        pd_client: Arc<PdC>,
    ) -> BoxStream<'static, Result<(Self::KeyType, Store<PdC::KvClient>)>> {
        let range = self.range.clone();
        pd_client
            .stores_for_range(range)
            .map_ok(move |store| {
                // TODO should be bounded by self.range
                let range = store.region.range();
                (range, store)
            })
            .into_stream()
            .boxed()
    }

    fn map_result(_: Self::RpcResponse) -> Self::Result {}

    fn reduce(
        results: BoxStream<'static, Result<Self::Result>>,
    ) -> BoxFuture<'static, Result<Self::Result>> {
        results
            .into_future()
            .map(|(f, _)| f.expect("no results should be impossible"))
            .boxed()
    }
}

#[derive(Clone)]
pub struct RawScan {
    pub range: BoundRange,
    // TODO this limit is currently treated as a per-region limit, not a total
    // limit.
    pub limit: u32,
    pub key_only: bool,
    pub cf: Option<ColumnFamily>,
}

impl RawRequest for RawScan {
    type Result = Vec<KvPair>;
    type RpcRequest = kvrpcpb::RawScanRequest;
    type RpcResponse = kvrpcpb::RawScanResponse;
    type KeyType = (Key, Key);
    const REQUEST_NAME: &'static str = "raw_scan";
    const RPC_FN: RpcFnType<Self::RpcRequest, Self::RpcResponse> = TikvClient::raw_scan_async_opt;

    fn into_request<KvC: KvClient>(
        self,
        (start_key, end_key): Self::KeyType,
        store: &Store<KvC>,
    ) -> Self::RpcRequest {
        let mut req = store.request::<Self::RpcRequest>();
        req.set_start_key(start_key.into());
        req.set_end_key(end_key.into());
        req.set_limit(self.limit);
        req.set_key_only(self.key_only);
        req.maybe_set_cf(self.cf);

        req
    }

    fn store_stream<PdC: PdClient>(
        &mut self,
        pd_client: Arc<PdC>,
    ) -> BoxStream<'static, Result<(Self::KeyType, Store<PdC::KvClient>)>> {
        let range = self.range.clone();
        pd_client
            .stores_for_range(range)
            .map_ok(move |store| {
                // TODO seems like these should be bounded by self.range
                let range = store.region.range();
                (range, store)
            })
            .into_stream()
            .boxed()
    }

    fn map_result(mut resp: Self::RpcResponse) -> Self::Result {
        resp.take_kvs().into_iter().map(Into::into).collect()
    }

    fn reduce(
        results: BoxStream<'static, Result<Self::Result>>,
    ) -> BoxFuture<'static, Result<Self::Result>> {
        results.try_concat().boxed()
    }
}

#[derive(Clone)]
pub struct RawBatchScan {
    pub ranges: Vec<BoundRange>,
    pub each_limit: u32,
    pub key_only: bool,
    pub cf: Option<ColumnFamily>,
}

impl RawRequest for RawBatchScan {
    type Result = Vec<KvPair>;
    type RpcRequest = kvrpcpb::RawBatchScanRequest;
    type RpcResponse = kvrpcpb::RawBatchScanResponse;
    type KeyType = Vec<BoundRange>;
    const REQUEST_NAME: &'static str = "raw_batch_scan";
    const RPC_FN: RpcFnType<Self::RpcRequest, Self::RpcResponse> =
        TikvClient::raw_batch_scan_async_opt;

    fn into_request<KvC: KvClient>(
        self,
        ranges: Self::KeyType,
        store: &Store<KvC>,
    ) -> Self::RpcRequest {
        let mut req = store.request::<Self::RpcRequest>();
        req.set_ranges(ranges.into_iter().map(Into::into).collect());
        req.set_each_limit(self.each_limit);
        req.set_key_only(self.key_only);
        req.maybe_set_cf(self.cf);

        req
    }

    fn store_stream<PdC: PdClient>(
        &mut self,
        _pd_client: Arc<PdC>,
    ) -> BoxStream<'static, Result<(Self::KeyType, Store<PdC::KvClient>)>> {
        future::err(Error::unimplemented()).into_stream().boxed()
    }

    fn map_result(mut resp: Self::RpcResponse) -> Self::Result {
        resp.take_kvs().into_iter().map(Into::into).collect()
    }

    fn reduce(
        results: BoxStream<'static, Result<Self::Result>>,
    ) -> BoxFuture<'static, Result<Self::Result>> {
        results.try_concat().boxed()
    }
}
