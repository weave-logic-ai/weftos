//! Message bus for async channel-agent communication.
//!
//! Provides a thread-safe [`MessageBus`] using bounded MPSC channels
//! for routing inbound messages (from channels) and outbound messages
//! (from the agent pipeline) with configurable backpressure.
//!
//! On native, uses tokio bounded MPSC channels.
//! On browser/WASM, uses futures-channel unbounded MPSC channels.
//!
//! Ported from Python `nanobot/bus/queue.py`.

#[cfg(feature = "native")]
use tokio::sync::Mutex;

use tracing::debug;

use clawft_types::error::ClawftError;
use clawft_types::event::{InboundMessage, OutboundMessage};

/// Default channel capacity for bounded MPSC channels (native only).
#[cfg(feature = "native")]
const DEFAULT_CHANNEL_CAPACITY: usize = 1024;

// ---------------------------------------------------------------------------
// Native implementation (tokio channels)
// ---------------------------------------------------------------------------

#[cfg(feature = "native")]
pub struct MessageBus {
    inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
    inbound_rx: Mutex<tokio::sync::mpsc::Receiver<InboundMessage>>,
    outbound_tx: tokio::sync::mpsc::Sender<OutboundMessage>,
    outbound_rx: Mutex<tokio::sync::mpsc::Receiver<OutboundMessage>>,
}

#[cfg(feature = "native")]
impl MessageBus {
    /// Create a new message bus with the default channel capacity (1024).
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CHANNEL_CAPACITY)
    }

    /// Create a new message bus with a custom channel capacity.
    ///
    /// Higher capacity allows more burst buffering at the cost of memory.
    /// Lower capacity provides tighter backpressure.
    pub fn with_capacity(capacity: usize) -> Self {
        let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(capacity);
        let (outbound_tx, outbound_rx) = tokio::sync::mpsc::channel(capacity);

        debug!(capacity, "MessageBus created with bounded channels");

        Self {
            inbound_tx,
            inbound_rx: Mutex::new(inbound_rx),
            outbound_tx,
            outbound_rx: Mutex::new(outbound_rx),
        }
    }

    /// Publish an inbound message (from a channel adapter) to the bus.
    ///
    /// Uses `try_send` to avoid requiring the caller to be async.
    /// Returns an error if the channel is closed or the buffer is full.
    pub fn publish_inbound(&self, msg: InboundMessage) -> Result<(), ClawftError> {
        debug!(
            channel = %msg.channel,
            chat_id = %msg.chat_id,
            "publishing inbound message"
        );
        self.inbound_tx.try_send(msg).map_err(|e| match e {
            tokio::sync::mpsc::error::TrySendError::Full(_) => {
                ClawftError::Channel("inbound channel full (backpressure)".into())
            }
            tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                ClawftError::Channel("inbound channel closed".into())
            }
        })
    }

    /// Publish an inbound message, waiting asynchronously if the buffer is full.
    ///
    /// Prefer this over [`publish_inbound`](Self::publish_inbound) in async
    /// contexts to avoid dropping messages under backpressure.
    pub async fn publish_inbound_async(&self, msg: InboundMessage) -> Result<(), ClawftError> {
        debug!(
            channel = %msg.channel,
            chat_id = %msg.chat_id,
            "publishing inbound message (async)"
        );
        self.inbound_tx
            .send(msg)
            .await
            .map_err(|e| ClawftError::Channel(format!("inbound channel closed: {e}")))
    }

    /// Consume the next inbound message from the bus.
    ///
    /// Returns `None` if the channel is closed and all buffered messages
    /// have been consumed.
    pub async fn consume_inbound(&self) -> Option<InboundMessage> {
        let mut rx = self.inbound_rx.lock().await;
        rx.recv().await
    }

    /// Dispatch an outbound message (from the agent pipeline) to the bus.
    ///
    /// Uses `try_send` to avoid requiring the caller to be async.
    /// Returns an error if the channel is closed or the buffer is full.
    pub fn dispatch_outbound(&self, msg: OutboundMessage) -> Result<(), ClawftError> {
        debug!(
            channel = %msg.channel,
            chat_id = %msg.chat_id,
            "dispatching outbound message"
        );
        self.outbound_tx.try_send(msg).map_err(|e| match e {
            tokio::sync::mpsc::error::TrySendError::Full(_) => {
                ClawftError::Channel("outbound channel full (backpressure)".into())
            }
            tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                ClawftError::Channel("outbound channel closed".into())
            }
        })
    }

    /// Dispatch an outbound message, waiting asynchronously if the buffer is full.
    pub async fn dispatch_outbound_async(&self, msg: OutboundMessage) -> Result<(), ClawftError> {
        debug!(
            channel = %msg.channel,
            chat_id = %msg.chat_id,
            "dispatching outbound message (async)"
        );
        self.outbound_tx
            .send(msg)
            .await
            .map_err(|e| ClawftError::Channel(format!("outbound channel closed: {e}")))
    }

    /// Consume the next outbound message from the bus.
    ///
    /// Returns `None` if the channel is closed and all buffered messages
    /// have been consumed.
    pub async fn consume_outbound(&self) -> Option<OutboundMessage> {
        let mut rx = self.outbound_rx.lock().await;
        rx.recv().await
    }

    /// Get a cloneable sender handle for publishing inbound messages.
    ///
    /// Channel adapters should clone this sender so that multiple producers
    /// (e.g. Telegram, Slack, Discord) can publish concurrently.
    pub fn inbound_sender(&self) -> tokio::sync::mpsc::Sender<InboundMessage> {
        self.inbound_tx.clone()
    }

    /// Get a cloneable sender handle for dispatching outbound messages.
    ///
    /// Pipeline stages or agent tasks can clone this sender for concurrent
    /// outbound dispatch.
    pub fn outbound_sender(&self) -> tokio::sync::mpsc::Sender<OutboundMessage> {
        self.outbound_tx.clone()
    }
}

// ---------------------------------------------------------------------------
// Browser implementation (futures-channel)
// ---------------------------------------------------------------------------

#[cfg(feature = "browser")]
pub struct MessageBus {
    inbound_tx: futures_util::lock::Mutex<futures_channel::mpsc::UnboundedSender<InboundMessage>>,
    inbound_rx: futures_util::lock::Mutex<futures_channel::mpsc::UnboundedReceiver<InboundMessage>>,
    outbound_tx: futures_util::lock::Mutex<futures_channel::mpsc::UnboundedSender<OutboundMessage>>,
    outbound_rx:
        futures_util::lock::Mutex<futures_channel::mpsc::UnboundedReceiver<OutboundMessage>>,
}

#[cfg(feature = "browser")]
impl MessageBus {
    /// Create a new message bus (browser: unbounded channels).
    pub fn new() -> Self {
        let (inbound_tx, inbound_rx) = futures_channel::mpsc::unbounded();
        let (outbound_tx, outbound_rx) = futures_channel::mpsc::unbounded();

        debug!("MessageBus created with unbounded channels (browser)");

        Self {
            inbound_tx: futures_util::lock::Mutex::new(inbound_tx),
            inbound_rx: futures_util::lock::Mutex::new(inbound_rx),
            outbound_tx: futures_util::lock::Mutex::new(outbound_tx),
            outbound_rx: futures_util::lock::Mutex::new(outbound_rx),
        }
    }

    /// Create a new message bus (capacity parameter ignored on browser).
    pub fn with_capacity(_capacity: usize) -> Self {
        Self::new()
    }

    /// Publish an inbound message to the bus.
    pub fn publish_inbound(&self, msg: InboundMessage) -> Result<(), ClawftError> {
        debug!(
            channel = %msg.channel,
            chat_id = %msg.chat_id,
            "publishing inbound message"
        );
        // For browser single-threaded context, try_lock should always succeed.
        if let Some(tx) = self.inbound_tx.try_lock() {
            tx.unbounded_send(msg)
                .map_err(|_| ClawftError::Channel("inbound channel closed".into()))
        } else {
            Err(ClawftError::Channel("inbound channel busy".into()))
        }
    }

    /// Publish an inbound message (async version).
    pub async fn publish_inbound_async(&self, msg: InboundMessage) -> Result<(), ClawftError> {
        debug!(
            channel = %msg.channel,
            chat_id = %msg.chat_id,
            "publishing inbound message (async)"
        );
        let tx = self.inbound_tx.lock().await;
        tx.unbounded_send(msg)
            .map_err(|_| ClawftError::Channel("inbound channel closed".into()))
    }

    /// Consume the next inbound message from the bus.
    pub async fn consume_inbound(&self) -> Option<InboundMessage> {
        use futures_util::StreamExt;
        let mut rx = self.inbound_rx.lock().await;
        rx.next().await
    }

    /// Dispatch an outbound message to the bus.
    pub fn dispatch_outbound(&self, msg: OutboundMessage) -> Result<(), ClawftError> {
        debug!(
            channel = %msg.channel,
            chat_id = %msg.chat_id,
            "dispatching outbound message"
        );
        if let Some(tx) = self.outbound_tx.try_lock() {
            tx.unbounded_send(msg)
                .map_err(|_| ClawftError::Channel("outbound channel closed".into()))
        } else {
            Err(ClawftError::Channel("outbound channel busy".into()))
        }
    }

    /// Dispatch an outbound message (async version).
    pub async fn dispatch_outbound_async(&self, msg: OutboundMessage) -> Result<(), ClawftError> {
        debug!(
            channel = %msg.channel,
            chat_id = %msg.chat_id,
            "dispatching outbound message (async)"
        );
        let tx = self.outbound_tx.lock().await;
        tx.unbounded_send(msg)
            .map_err(|_| ClawftError::Channel("outbound channel closed".into()))
    }

    /// Consume the next outbound message from the bus.
    pub async fn consume_outbound(&self) -> Option<OutboundMessage> {
        use futures_util::StreamExt;
        let mut rx = self.outbound_rx.lock().await;
        rx.next().await
    }
}

// ---------------------------------------------------------------------------
// No-op fallback (neither native nor browser -- e.g. wasip2)
// ---------------------------------------------------------------------------

#[cfg(not(any(feature = "native", feature = "browser")))]
pub struct MessageBus {
    _private: (),
}

#[cfg(not(any(feature = "native", feature = "browser")))]
impl MessageBus {
    /// Create a no-op message bus (no channel backend available).
    pub fn new() -> Self {
        debug!("MessageBus created (no-op fallback, no channel backend)");
        Self { _private: () }
    }

    /// Create a no-op message bus (capacity parameter ignored).
    pub fn with_capacity(_capacity: usize) -> Self {
        Self::new()
    }

    /// Always returns an error (no channel backend).
    pub fn publish_inbound(&self, _msg: InboundMessage) -> Result<(), ClawftError> {
        Err(ClawftError::Channel("no channel backend available".into()))
    }

    /// Always returns an error (no channel backend).
    pub async fn publish_inbound_async(&self, _msg: InboundMessage) -> Result<(), ClawftError> {
        Err(ClawftError::Channel("no channel backend available".into()))
    }

    /// Always returns `None` (no channel backend).
    pub async fn consume_inbound(&self) -> Option<InboundMessage> {
        None
    }

    /// Always returns an error (no channel backend).
    pub fn dispatch_outbound(&self, _msg: OutboundMessage) -> Result<(), ClawftError> {
        Err(ClawftError::Channel("no channel backend available".into()))
    }

    /// Always returns an error (no channel backend).
    pub async fn dispatch_outbound_async(&self, _msg: OutboundMessage) -> Result<(), ClawftError> {
        Err(ClawftError::Channel("no channel backend available".into()))
    }

    /// Always returns `None` (no channel backend).
    pub async fn consume_outbound(&self) -> Option<OutboundMessage> {
        None
    }
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;

    fn make_inbound(content: &str) -> InboundMessage {
        InboundMessage {
            channel: "test".into(),
            sender_id: "user1".into(),
            chat_id: "chat1".into(),
            content: content.into(),
            timestamp: Utc::now(),
            media: vec![],
            metadata: HashMap::new(),
        }
    }

    fn make_outbound(content: &str) -> OutboundMessage {
        OutboundMessage {
            channel: "test".into(),
            chat_id: "chat1".into(),
            content: content.into(),
            reply_to: None,
            media: vec![],
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn publish_and_consume_inbound() {
        let bus = MessageBus::new();
        let msg = make_inbound("hello");
        bus.publish_inbound(msg).unwrap();

        let received = bus.consume_inbound().await.unwrap();
        assert_eq!(received.content, "hello");
        assert_eq!(received.channel, "test");
    }

    #[tokio::test]
    async fn dispatch_and_consume_outbound() {
        let bus = MessageBus::new();
        let msg = make_outbound("reply");
        bus.dispatch_outbound(msg).unwrap();

        let received = bus.consume_outbound().await.unwrap();
        assert_eq!(received.content, "reply");
    }

    #[tokio::test]
    async fn multiple_inbound_messages_in_order() {
        let bus = MessageBus::new();
        for i in 0..5 {
            bus.publish_inbound(make_inbound(&format!("msg-{i}")))
                .unwrap();
        }

        for i in 0..5 {
            let msg = bus.consume_inbound().await.unwrap();
            assert_eq!(msg.content, format!("msg-{i}"));
        }
    }

    #[tokio::test]
    async fn multiple_outbound_messages_in_order() {
        let bus = MessageBus::new();
        for i in 0..5 {
            bus.dispatch_outbound(make_outbound(&format!("out-{i}")))
                .unwrap();
        }

        for i in 0..5 {
            let msg = bus.consume_outbound().await.unwrap();
            assert_eq!(msg.content, format!("out-{i}"));
        }
    }

    #[tokio::test]
    async fn default_creates_valid_bus() {
        let bus = MessageBus::default();
        bus.publish_inbound(make_inbound("default-test")).unwrap();
        bus.dispatch_outbound(make_outbound("default-test"))
            .unwrap();
    }

    #[tokio::test]
    async fn inbound_and_outbound_are_independent() {
        let bus = MessageBus::new();

        bus.publish_inbound(make_inbound("in")).unwrap();
        bus.dispatch_outbound(make_outbound("out")).unwrap();

        let inbound = bus.consume_inbound().await.unwrap();
        let outbound = bus.consume_outbound().await.unwrap();

        assert_eq!(inbound.content, "in");
        assert_eq!(outbound.content, "out");
    }

    // The following tests use tokio-specific APIs (spawn, try_send on tokio
    // Sender, etc.) and only compile under the native feature.

    #[cfg(feature = "native")]
    #[tokio::test]
    async fn inbound_sender_allows_multi_producer() {
        let bus = MessageBus::new();
        let tx1 = bus.inbound_sender();
        let tx2 = bus.inbound_sender();

        tx1.try_send(make_inbound("from-tx1")).unwrap();
        tx2.try_send(make_inbound("from-tx2")).unwrap();

        let msg1 = bus.consume_inbound().await.unwrap();
        let msg2 = bus.consume_inbound().await.unwrap();
        assert_eq!(msg1.content, "from-tx1");
        assert_eq!(msg2.content, "from-tx2");
    }

    #[cfg(feature = "native")]
    #[tokio::test]
    async fn outbound_sender_allows_multi_producer() {
        let bus = MessageBus::new();
        let tx1 = bus.outbound_sender();
        let tx2 = bus.outbound_sender();

        tx1.try_send(make_outbound("from-tx1")).unwrap();
        tx2.try_send(make_outbound("from-tx2")).unwrap();

        let msg1 = bus.consume_outbound().await.unwrap();
        let msg2 = bus.consume_outbound().await.unwrap();
        assert_eq!(msg1.content, "from-tx1");
        assert_eq!(msg2.content, "from-tx2");
    }

    #[cfg(feature = "native")]
    #[tokio::test]
    async fn consume_returns_none_when_all_senders_dropped() {
        let (tx, rx) = tokio::sync::mpsc::channel::<InboundMessage>(16);
        let rx = tokio::sync::Mutex::new(rx);
        tx.try_send(make_inbound("msg")).unwrap();
        drop(tx);

        let mut guard = rx.lock().await;
        assert!(guard.recv().await.is_some());
        assert!(guard.recv().await.is_none());
    }

    #[cfg(feature = "native")]
    #[tokio::test]
    async fn publish_inbound_error_on_closed_channel() {
        let (tx, rx) = tokio::sync::mpsc::channel::<InboundMessage>(16);
        drop(rx);

        let result = tx.try_send(make_inbound("orphan"));
        assert!(result.is_err());
    }

    #[cfg(feature = "native")]
    #[tokio::test]
    async fn dispatch_outbound_error_on_closed_channel() {
        let (tx, rx) = tokio::sync::mpsc::channel::<OutboundMessage>(16);
        drop(rx);

        let result = tx.try_send(make_outbound("orphan"));
        assert!(result.is_err());
    }

    #[cfg(feature = "native")]
    #[tokio::test]
    async fn concurrent_publish_and_consume() {
        let bus = std::sync::Arc::new(MessageBus::new());
        let bus_clone = bus.clone();

        let producer = tokio::spawn(async move {
            for i in 0..100 {
                bus_clone
                    .publish_inbound_async(make_inbound(&format!("concurrent-{i}")))
                    .await
                    .unwrap();
            }
        });

        let consumer = tokio::spawn(async move {
            let mut received = Vec::new();
            for _ in 0..100 {
                if let Some(msg) = bus.consume_inbound().await {
                    received.push(msg.content);
                }
            }
            received
        });

        producer.await.unwrap();
        let results = consumer.await.unwrap();
        assert_eq!(results.len(), 100);
    }

    #[cfg(feature = "native")]
    #[test]
    fn message_bus_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MessageBus>();
    }

    #[cfg(feature = "native")]
    #[tokio::test]
    async fn custom_capacity_bus() {
        let bus = MessageBus::with_capacity(4);
        for i in 0..4 {
            bus.publish_inbound(make_inbound(&format!("msg-{i}")))
                .unwrap();
        }
        let result = bus.publish_inbound(make_inbound("overflow"));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("backpressure"),
            "expected backpressure error, got: {err_msg}"
        );
    }

    #[cfg(feature = "native")]
    #[tokio::test]
    async fn async_publish_waits_when_full() {
        let bus = std::sync::Arc::new(MessageBus::with_capacity(2));
        let bus_producer = bus.clone();
        let bus_consumer = bus.clone();

        bus.publish_inbound(make_inbound("a")).unwrap();
        bus.publish_inbound(make_inbound("b")).unwrap();

        let producer = tokio::spawn(async move {
            bus_producer
                .publish_inbound_async(make_inbound("c"))
                .await
                .unwrap();
        });

        tokio::task::yield_now().await;
        let _ = bus_consumer.consume_inbound().await;

        producer.await.unwrap();
        let msg_b = bus_consumer.consume_inbound().await.unwrap();
        let msg_c = bus_consumer.consume_inbound().await.unwrap();
        assert_eq!(msg_b.content, "b");
        assert_eq!(msg_c.content, "c");
    }
}
