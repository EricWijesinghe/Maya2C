//! Per-call RPC metrics (SLO rpc-availability and rpc-latency, `docs/SLO.md`).
//!
//! A jsonrpsee middleware rather than a line in every method: forty methods
//! each timing themselves is forty chances to forget one. The method label is
//! the served name or `other` — a label copied from a request would let any
//! client mint unbounded time series in the scraper.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use jsonrpsee::MethodResponse;
use jsonrpsee::core::middleware::{Batch, Notification, RpcServiceT};
use jsonrpsee::types::Request;

use crate::metrics::Metrics;

/// Wraps an RPC service, recording every call.
#[derive(Clone, Debug)]
pub struct Metered<S> {
    inner: S,
    metrics: Arc<Metrics>,
    known: Arc<BTreeSet<String>>,
}

impl<S> Metered<S> {
    /// Wraps `inner`, labelling only the `known` method names.
    pub fn new(inner: S, metrics: Arc<Metrics>, known: Arc<BTreeSet<String>>) -> Self {
        Self {
            inner,
            metrics,
            known,
        }
    }
}

impl<S> RpcServiceT for Metered<S>
where
    S: RpcServiceT<MethodResponse = MethodResponse> + Send + Sync + Clone + 'static,
{
    type MethodResponse = S::MethodResponse;
    type NotificationResponse = S::NotificationResponse;
    type BatchResponse = S::BatchResponse;

    fn call<'a>(
        &self,
        request: Request<'a>,
    ) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
        let method = if self.known.contains(request.method_name()) {
            request.method_name().to_string()
        } else {
            "other".to_string()
        };
        let inner = self.inner.clone();
        let metrics = Arc::clone(&self.metrics);
        async move {
            let started = Instant::now();
            let response = inner.call(request).await;
            metrics.observe_rpc(method, response.is_success(), started.elapsed());
            response
        }
    }

    fn batch<'a>(
        &self,
        requests: Batch<'a>,
    ) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
        self.inner.batch(requests)
    }

    fn notification<'a>(
        &self,
        n: Notification<'a>,
    ) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
        self.inner.notification(n)
    }
}
