//! Feature framework — the seam that makes murmurd modular.
//!
//! A feature owns its state, declares which inbound [`Message`]s it wants,
//! and runs one task for the daemon's lifetime. The only things it sees:
//! [`FeatureCtx::shared`] (send + presence table) and its own filtered inbox.
//!
//! Adding a feature:
//! 1. create `src/foo.rs` with `pub fn feature() -> Feature` + `async fn run`
//! 2. add `foo::feature()` to the registry in `main.rs`
//! 3. if it speaks to peers: add a `Message` variant in murmur-proto

use crate::state::Shared;
use murmur_proto::{DeviceId, Message};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Inbound stream handed to a feature — pre-filtered to its tags.
/// `Arc` because one clipboard frame may feed several subscribers;
/// cloning the payload per sink was the old tax.
pub type Inbox = mpsc::UnboundedReceiver<(DeviceId, std::sync::Arc<Message>)>;

/// Everything a feature gets at runtime. `inbox` is `Some` iff the feature
/// declared a `wants` filter.
pub struct FeatureCtx {
    pub shared: Arc<Shared>,
    pub inbox: Option<Inbox>,
}

pub type BoxRun = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

pub struct Feature {
    pub name: &'static str,
    /// Message `"t"` tags this feature consumes. Empty = the feature
    /// consumes no peer messages and gets no inbox (e.g. the ctl socket).
    /// Tag dispatch replaces closure filters: O(1) lookup, and the tag
    /// list is documentation of what the feature reacts to.
    pub consumes: &'static [&'static str],
    run: fn(FeatureCtx) -> BoxRun,
    /// Self-status: a JSON fragment merged into ctl's status output under
    /// the feature's name. Features stop needing ctl.rs patched by hand.
    pub status: Option<fn(&Arc<Shared>) -> serde_json::Value>,
}

impl Feature {
    pub fn new(
        name: &'static str,
        consumes: &'static [&'static str],
        run: fn(FeatureCtx) -> BoxRun,
    ) -> Self {
        Self { name, consumes, run, status: None }
    }

    pub fn with_status(mut self, f: fn(&Arc<Shared>) -> serde_json::Value) -> Self {
        self.status = Some(f);
        self
    }

    /// Wire the feature: allocate its inbox if it consumes messages,
    /// register the sink in the hub, and return its task future.
    pub fn wire(self, shared: &Arc<Shared>, hub: &mut Hub) -> BoxRun {
        let inbox = (!self.consumes.is_empty()).then(|| {
            let (tx, rx) = mpsc::unbounded_channel();
            for tag in self.consumes {
                hub.sinks.entry(*tag).or_default().push(tx.clone());
            }
            rx
        });
        if let Some(hook) = self.status {
            shared.feature_status.write().unwrap().push((self.name, hook));
        }
        tracing::info!("feature {} wired (inbox: {})", self.name, inbox.is_some());
        (self.run)(FeatureCtx {
            shared: shared.clone(),
            inbox,
        })
    }
}

/// Fans each decrypted inbound message out to the features that want it.
/// Lives on the conn side — conn calls `dispatch` per envelope.
pub struct Hub {
    pub(crate) sinks: std::collections::HashMap<
        &'static str,
        Vec<mpsc::UnboundedSender<(DeviceId, std::sync::Arc<Message>)>>,
    >,
}

impl Hub {
    pub fn new() -> Self {
        Self { sinks: std::collections::HashMap::new() }
    }

    pub fn dispatch(&self, from: DeviceId, msg: &Message) {
        if let Some(sinks) = self.sinks.get(msg.tag()) {
            let msg = std::sync::Arc::new(msg.clone());
            for tx in sinks {
                let _ = tx.send((from, msg.clone()));
            }
        }
    }
}
