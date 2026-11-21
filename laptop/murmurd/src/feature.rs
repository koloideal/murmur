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

/// Inbound stream handed to a feature — pre-filtered to what it wants.
pub type Inbox = mpsc::UnboundedReceiver<(DeviceId, Message)>;

/// Everything a feature gets at runtime. `inbox` is `Some` iff the feature
/// declared a `wants` filter.
pub struct FeatureCtx {
    pub shared: Arc<Shared>,
    pub inbox: Option<Inbox>,
}

pub type BoxRun = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

pub struct Feature {
    pub name: &'static str,
    /// Inbound filter. `None` = the feature consumes no peer messages and
    /// gets no inbox (e.g. the ctl socket).
    pub wants: Option<fn(&Message) -> bool>,
    run: fn(FeatureCtx) -> BoxRun,
}

impl Feature {
    pub fn new(
        name: &'static str,
        wants: Option<fn(&Message) -> bool>,
        run: fn(FeatureCtx) -> BoxRun,
    ) -> Self {
        Self { name, wants, run }
    }

    /// Wire the feature: allocate its inbox if it wants messages, register
    /// the sink in the hub, and return its task future.
    pub fn wire(self, shared: &Arc<Shared>, hub: &mut Hub) -> BoxRun {
        let inbox = self.wants.map(|wants| {
            let (tx, rx) = mpsc::unbounded_channel();
            hub.sinks.push((wants, tx));
            rx
        });
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
    sinks: Vec<(
        fn(&Message) -> bool,
        mpsc::UnboundedSender<(DeviceId, Message)>,
    )>,
}

impl Hub {
    pub fn new() -> Self {
        Self { sinks: Vec::new() }
    }

    pub fn dispatch(&self, from: DeviceId, msg: &Message) {
        for (wants, tx) in &self.sinks {
            if wants(msg) {
                let _ = tx.send((from, msg.clone()));
            }
        }
    }
}
