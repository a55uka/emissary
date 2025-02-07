// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use crate::{
    crypto::StaticPublicKey,
    error::{ChannelError, QueryError},
    netdb::LOG_TARGET,
    primitives::{LeaseSet2, RouterId},
};

use bytes::Bytes;
use futures_channel::oneshot;
use thingbuf::mpsc;

use alloc::vec::Vec;

/// Recycling strategy for [`NetDbAction`].
#[derive(Default, Clone)]
pub struct NetDbActionRecycle(());

impl thingbuf::Recycle<NetDbAction> for NetDbActionRecycle {
    fn new_element(&self) -> NetDbAction {
        NetDbAction::Dummy
    }

    fn recycle(&self, element: &mut NetDbAction) {
        *element = NetDbAction::Dummy;
    }
}

/// Query kind.
pub enum NetDbAction {
    /// [`LeaseSet2`] query.
    QueryLeaseSet2 {
        /// Key,
        key: Bytes,

        /// Oneshot sender used to send the result to caller.
        tx: oneshot::Sender<Result<LeaseSet2, QueryError>>,
    },

    /// [`RouterInfo`] query.
    QueryRouterInfo {
        /// Router ID.
        router_id: RouterId,

        /// Oneshot sender used to send the result to caller.
        ///
        /// Note that the [`RouterInfo`] is not directly send to the caller but [`NetDb`] stores
        /// the received [`RouterInfo`] directly into [`ProfileStorage`].
        tx: oneshot::Sender<Result<(), QueryError>>,
    },

    /// Get `RouterId`'s of the floodfills closest to `key`.
    GetClosestFloodfills {
        /// Key.
        key: Bytes,

        /// Oneshot sender used to send the result to caller.
        tx: oneshot::Sender<Vec<(RouterId, StaticPublicKey)>>,
    },

    /// Publish [`RouterInfo`] to `NetDb`
    PublishRouterInfo {
        /// Router ID.
        router_id: RouterId,

        /// Serialized, uncompressed [`RouterInfo`].
        router_info: Bytes,
    },

    /// Dummy value.
    Dummy,
}

impl Default for NetDbAction {
    fn default() -> Self {
        Self::Dummy
    }
}

/// Handle to [`NetDb`].
#[derive(Clone)]
pub struct NetDbHandle {
    /// TX channel for sending queries to [`NetDb`].
    tx: mpsc::Sender<NetDbAction, NetDbActionRecycle>,
}

impl NetDbHandle {
    /// Create new [`NetDbHandle`].
    pub(super) fn new(tx: mpsc::Sender<NetDbAction, NetDbActionRecycle>) -> Self {
        Self { tx }
    }

    /// Send `DatabaseLookup` for a `LeaseSet2` identified by `key`.
    ///
    /// On success returns a `oneshot::Receiver` the caller must poll for a reply poll for a reply.
    /// If the query succeeded, `LeaseSet2` is returned and if ti failed, `QueryError` is returned.
    ///
    /// If the channel towards `NetDb` is full, `ChannelError::Full` is returned and the caller must
    /// retry later.
    pub fn query_leaseset(
        &self,
        key: Bytes,
    ) -> Result<oneshot::Receiver<Result<LeaseSet2, QueryError>>, ChannelError> {
        let (tx, rx) = oneshot::channel();

        self.tx
            .try_send(NetDbAction::QueryLeaseSet2 { key, tx })
            .map(|_| rx)
            .map_err(From::from)
    }

    /// Send `DatabaseLookup` for a `RouterInf` identified by `router_id`.
    ///
    /// On success returns a `oneshot::Receiver` the caller must poll for a reply poll for a reply.
    /// If the query succeeded, `RouterInfo` is returned and if ti failed, `QueryError` is returned.
    ///
    /// If the channel towards `NetDb` is full, `ChannelError::Full` is returned and the caller must
    /// retry later.
    pub fn query_router_info(
        &self,
        router_id: RouterId,
    ) -> Result<oneshot::Receiver<Result<(), QueryError>>, ChannelError> {
        let (tx, rx) = oneshot::channel();

        self.tx
            .try_send(NetDbAction::QueryRouterInfo { router_id, tx })
            .map(|_| rx)
            .map_err(From::from)
    }

    /// Get `RouterId`'s and encryption public keys of the floodfills closest to `key`.
    ///
    /// Return channel is dropped by [`NetDb`] if there are no floodfills available.
    pub fn get_closest_floodfills(
        &self,
        key: Bytes,
    ) -> Result<oneshot::Receiver<Vec<(RouterId, StaticPublicKey)>>, ChannelError> {
        let (tx, rx) = oneshot::channel();

        self.tx
            .try_send(NetDbAction::GetClosestFloodfills { key, tx })
            .map(|_| rx)
            .map_err(From::from)
    }

    /// Publish `router_info` under `router_id` in `NetDb`
    pub fn publish_router_info(&self, router_id: RouterId, router_info: Bytes) {
        if let Err(error) = self.tx.try_send(NetDbAction::PublishRouterInfo {
            router_id,
            router_info,
        }) {
            tracing::warn!(
                target: LOG_TARGET,
                ?error,
                "failed to send router info for publication to netdb",
            );
        }
    }

    #[cfg(test)]
    /// Create new [`NetDbHandle`] for testing.
    pub fn create() -> (Self, mpsc::Receiver<NetDbAction, NetDbActionRecycle>) {
        let (tx, rx) = mpsc::with_recycle(64, NetDbActionRecycle::default());

        (Self { tx }, rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crypto::SigningPrivateKey,
        primitives::RouterInfo,
        runtime::{mock::MockRuntime, Runtime},
    };
    use rand_core::RngCore;

    #[test]
    fn send_leaseset_query() {
        let (tx, rx) = mpsc::with_recycle(5, NetDbActionRecycle(()));
        let handle = NetDbHandle::new(tx);

        assert!(handle.query_leaseset(Bytes::from(vec![1, 2, 3, 4])).is_ok());

        match rx.try_recv() {
            Ok(NetDbAction::QueryLeaseSet2 { key, .. }) => {
                assert_eq!(key, Bytes::from(vec![1, 2, 3, 4]));
            }
            _ => panic!("invalid event"),
        }
    }

    #[test]
    fn send_router_info_query() {
        let (tx, rx) = mpsc::with_recycle(5, NetDbActionRecycle(()));
        let handle = NetDbHandle::new(tx);
        let remote = RouterId::random();

        assert!(handle.query_router_info(remote.clone()).is_ok());

        match rx.try_recv() {
            Ok(NetDbAction::QueryRouterInfo { router_id, .. }) => {
                assert_eq!(router_id, remote);
            }
            _ => panic!("invalid event"),
        }
    }

    #[test]
    fn publish_router_info() {
        let (tx, rx) = mpsc::with_recycle(5, NetDbActionRecycle(()));
        let handle = NetDbHandle::new(tx);
        let (router_id, router_info) = {
            let mut static_key_bytes = vec![0u8; 32];
            let mut signing_key_bytes = vec![0u8; 32];

            MockRuntime::rng().fill_bytes(&mut static_key_bytes);
            MockRuntime::rng().fill_bytes(&mut signing_key_bytes);

            let signing_key = SigningPrivateKey::from_bytes(&signing_key_bytes).unwrap();

            let router_info =
                RouterInfo::from_keys::<MockRuntime>(static_key_bytes, signing_key_bytes);
            let router_id = router_info.identity.id();

            (router_id, Bytes::from(router_info.serialize(&signing_key)))
        };
        handle.publish_router_info(router_id.clone(), router_info);

        match rx.try_recv() {
            Ok(NetDbAction::PublishRouterInfo {
                router_id: key,
                router_info,
            }) => {
                assert_eq!(key, router_id);
                assert!(RouterInfo::parse(router_info).is_some());
            }
            _ => panic!("invalid event"),
        }
    }

    #[test]
    fn channel_full() {
        let (tx, _rx) = mpsc::with_recycle(5, NetDbActionRecycle(()));
        let handle = NetDbHandle::new(tx);

        for _ in 0..5 {
            assert!(handle.query_leaseset(Bytes::from(vec![1, 2, 3, 4])).is_ok());
        }

        // try to send one more element and ensure the call fails
        match handle.query_leaseset(Bytes::from(vec![1, 2, 3, 4])).unwrap_err() {
            ChannelError::Full => {}
            error => panic!("invalid error: {error}"),
        }
    }
}
