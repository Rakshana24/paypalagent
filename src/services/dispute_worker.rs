use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Notify};
use tracing::{debug, error, info, warn};

use crate::error::AppError;
use crate::services::dispute_consumer::DisputeConsumerService;
use crate::services::webhook_persistence::DEFAULT_LEASE_DURATION_SECS;

/// Default background polling interval when the event queue is idle (2 seconds).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Default maximum number of events to process in a single continuous batch (50).
pub const DEFAULT_MAX_BATCH_SIZE: usize = 50;

/// Thread-safe cooperative cancellation token for coordinating worker shutdown.
///
/// Built on `tokio::sync::watch` to provide zero-allocation cancellation notification
/// without requiring extra external crate dependencies.
#[derive(Clone, Debug)]
pub struct CancellationToken {
    sender: Arc<watch::Sender<bool>>,
    receiver: watch::Receiver<bool>,
}

impl CancellationToken {
    /// Creates a new uncancelled token.
    pub fn new() -> Self {
        let (sender, receiver) = watch::channel(false);
        Self {
            sender: Arc::new(sender),
            receiver,
        }
    }

    /// Triggers cancellation, notifying all listeners and clones.
    pub fn cancel(&self) {
        let _ = self.sender.send(true);
    }

    /// Returns `true` if cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        *self.receiver.borrow()
    }

    /// Asynchronously waits until cancellation has been requested.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let mut rx = self.receiver.clone();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                // Sender was dropped; treat as cancelled
                break;
            }
        }
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Autonomous background worker driving asynchronous dispute webhook consumption.
///
/// Wraps `DisputeConsumerService` in a resilient processing loop:
/// - Wakes up immediately when notified via `tokio::sync::Notify` upon webhook ingress.
/// - Falls back to periodic polling for scheduled retries and reclaimed abandoned leases.
/// - Drains available events in batches to maximize throughput under load.
/// - Avoids busy-spinning when the queue is idle by sleeping on notify/timeout.
/// - Supports cooperative graceful shutdown via `CancellationToken`.
#[derive(Clone)]
pub struct BackgroundDisputeWorker {
    consumer_service: Arc<DisputeConsumerService>,
    notify: Arc<Notify>,
    worker_id: String,
    lease_duration_secs: u64,
    poll_interval: Duration,
    max_batch_size: usize,
}

impl BackgroundDisputeWorker {
    /// Creates a new `BackgroundDisputeWorker` with sensible production defaults.
    pub fn new(consumer_service: Arc<DisputeConsumerService>, notify: Arc<Notify>) -> Self {
        let worker_id = format!("dispute-worker-{}", uuid::Uuid::new_v4());
        Self {
            consumer_service,
            notify,
            worker_id,
            lease_duration_secs: DEFAULT_LEASE_DURATION_SECS,
            poll_interval: DEFAULT_POLL_INTERVAL,
            max_batch_size: DEFAULT_MAX_BATCH_SIZE,
        }
    }

    /// Customizes the worker identifier string.
    pub fn with_worker_id(mut self, worker_id: impl Into<String>) -> Self {
        self.worker_id = worker_id.into();
        self
    }

    /// Customizes the processing lease duration in seconds.
    pub fn with_lease_duration_secs(mut self, lease_duration_secs: u64) -> Self {
        self.lease_duration_secs = lease_duration_secs;
        self
    }

    /// Customizes the fallback idle polling interval.
    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    /// Customizes the maximum batch size drained in a single loop cycle.
    pub fn with_max_batch_size(mut self, max_batch_size: usize) -> Self {
        self.max_batch_size = max_batch_size;
        self
    }

    /// Returns the assigned worker identifier.
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    /// Returns the active lease duration in seconds.
    pub fn lease_duration_secs(&self) -> u64 {
        self.lease_duration_secs
    }

    /// Returns the fallback idle polling interval.
    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    /// Returns the maximum batch size.
    pub fn max_batch_size(&self) -> usize {
        self.max_batch_size
    }

    /// Returns a reference to the shared `Notify` instance.
    pub fn notify(&self) -> &Arc<Notify> {
        &self.notify
    }

    /// Returns a reference to the underlying `DisputeConsumerService`.
    pub fn consumer_service(&self) -> &Arc<DisputeConsumerService> {
        &self.consumer_service
    }

    /// Drains available events in sequence up to `max_batch_size` until the queue is exhausted.
    ///
    /// Returns the number of events processed during this batch invocation.
    pub async fn process_available_batch(&self) -> Result<usize, AppError> {
        let mut processed_count = 0;

        while processed_count < self.max_batch_size {
            match self
                .consumer_service
                .process_next_event(&self.worker_id, self.lease_duration_secs)
                .await
            {
                Ok(Some(outcome)) => {
                    processed_count += 1;
                    debug!(
                        worker_id = %self.worker_id,
                        batch_count = processed_count,
                        outcome = ?outcome,
                        "Processed dispute event in background batch"
                    );
                }
                Ok(None) => {
                    // Queue has no eligible events ready for processing right now
                    break;
                }
                Err(err) => {
                    error!(
                        worker_id = %self.worker_id,
                        error = %err,
                        "Error processing event in background batch; aborting current batch"
                    );
                    return Err(err);
                }
            }
        }

        Ok(processed_count)
    }

    /// Runs the autonomous background loop until the cancellation token is signaled.
    pub async fn run(&self, cancel_token: CancellationToken) {
        info!(
            worker_id = %self.worker_id,
            poll_interval_ms = %self.poll_interval.as_millis(),
            lease_duration_secs = %self.lease_duration_secs,
            max_batch_size = %self.max_batch_size,
            "Starting background dispute processing worker loop"
        );

        while !cancel_token.is_cancelled() {
            // 1. Drain available events in the queue
            match self.process_available_batch().await {
                Ok(count) if count > 0 => {
                    debug!(
                        worker_id = %self.worker_id,
                        processed_count = count,
                        "Batch successfully drained dispute events"
                    );

                    // Yield to avoid starving cooperative tasks
                    tokio::task::yield_now().await;

                    if cancel_token.is_cancelled() {
                        break;
                    }

                    // If we processed a full batch, loop immediately to drain further without waiting
                    if count >= self.max_batch_size {
                        continue;
                    }
                }
                Ok(_) => {
                    // Queue is idle; proceed to wait
                }
                Err(err) => {
                    warn!(
                        worker_id = %self.worker_id,
                        error = %err,
                        "Batch processing failed with error; applying brief backoff"
                    );

                    // Brief error backoff to avoid busy-spinning on persistent DB errors
                    tokio::select! {
                        _ = cancel_token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                    }
                    continue;
                }
            }

            // 2. Queue is idle: wait for Notify signal, polling interval timeout, or cancellation
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!(
                        worker_id = %self.worker_id,
                        "Shutdown signal received; terminating background dispute worker"
                    );
                    break;
                }
                _ = self.notify.notified() => {
                    debug!(
                        worker_id = %self.worker_id,
                        "Dispute worker notified of incoming webhook event; initiating processing"
                    );
                }
                _ = tokio::time::sleep(self.poll_interval) => {
                    // Periodic poll interval elapsed; re-check queue for scheduled retries or reclaimed leases
                }
            }
        }

        info!(
            worker_id = %self.worker_id,
            "Background dispute processing worker terminated cleanly"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::time::Instant;

    use crate::config::Config;
    use crate::db::init_db;
    use crate::http_client::{create_http_client, HttpClientConfig};
    use crate::models::webhook::{PayPalWebhookEvent, WebhookPersistenceResult};
    use crate::services::auth::PayPalAuthService;
    use crate::services::dispute_consumer::DisputeConsumerService;
    use crate::services::dispute_correlation::DisputeCorrelationService;
    use crate::services::dispute_service::DisputeService;
    use crate::services::verification::VerificationService;
    use crate::services::webhook_persistence::WebhookPersistenceService;

    // Helper to create test worker infrastructure
    async fn setup_test_worker() -> (
        BackgroundDisputeWorker,
        Arc<WebhookPersistenceService>,
        Arc<tokio::sync::Notify>,
        sqlx::SqlitePool,
    ) {
        let pool = init_db("sqlite::memory:").await.unwrap();

        let config = Arc::new(Config {
            port: 0,
            rust_log: "info".to_string(),
            paypal_base_url: "http://127.0.0.1:9999".to_string(),
            database_url: "sqlite::memory:".to_string(),
            paypal_client_id: "mock_client".to_string(),
            paypal_client_secret: "mock_secret".to_string(),
            paypal_webhook_id: Some("mock_webhook_id".to_string()),
        });

        let http_client = create_http_client(&HttpClientConfig::default()).unwrap();
        let auth_service = Arc::new(PayPalAuthService::new(config.clone(), http_client.clone()));
        let verification = Arc::new(VerificationService::new(pool.clone()));
        let dispute_service = Arc::new(DisputeService::new(pool.clone(), verification.clone()));
        let correlation = Arc::new(DisputeCorrelationService::new(
            config,
            http_client,
            auth_service,
            pool.clone(),
            verification,
        ));
        let persistence = Arc::new(WebhookPersistenceService::new(pool.clone()));
        let consumer = Arc::new(DisputeConsumerService::new(
            pool.clone(),
            persistence.clone(),
            dispute_service,
            correlation,
        ));
        let notify = Arc::new(Notify::new());

        let worker = BackgroundDisputeWorker::new(consumer, notify.clone())
            .with_poll_interval(Duration::from_millis(50))
            .with_lease_duration_secs(30);

        (worker, persistence, notify, pool)
    }

    #[tokio::test]
    async fn test_cancellation_token_lifecycle() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());

        let token_clone = token.clone();
        assert!(!token_clone.is_cancelled());

        token.cancel();
        assert!(token.is_cancelled());
        assert!(token_clone.is_cancelled());

        // cancelled() should return immediately once cancelled
        token.cancelled().await;
        token_clone.cancelled().await;
    }

    #[tokio::test]
    async fn test_cancellation_token_waiter() {
        let token = CancellationToken::new();
        let token_clone = token.clone();

        let handle = tokio::spawn(async move {
            token_clone.cancelled().await;
            true
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        token.cancel();

        let result = handle.await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_worker_builder_and_defaults() {
        let (worker, _, _, _) = setup_test_worker().await;
        assert!(worker.worker_id().starts_with("dispute-worker-"));
        assert_eq!(
            worker.lease_duration_secs(),
            30 // customized in setup_test_worker
        );
        assert_eq!(worker.poll_interval(), Duration::from_millis(50));
        assert_eq!(worker.max_batch_size(), DEFAULT_MAX_BATCH_SIZE);

        let customized = worker
            .with_worker_id("custom-worker-42")
            .with_lease_duration_secs(120)
            .with_poll_interval(Duration::from_millis(500))
            .with_max_batch_size(10);

        assert_eq!(customized.worker_id(), "custom-worker-42");
        assert_eq!(customized.lease_duration_secs(), 120);
        assert_eq!(customized.poll_interval(), Duration::from_millis(500));
        assert_eq!(customized.max_batch_size(), 10);
    }

    #[tokio::test]
    async fn test_worker_idle_polling_and_clean_cancellation() {
        let (worker, _, _, _) = setup_test_worker().await;
        let token = CancellationToken::new();
        let token_clone = token.clone();

        let worker_task = tokio::spawn(async move {
            worker.run(token_clone).await;
        });

        // Let the worker idle for several poll cycles
        tokio::time::sleep(Duration::from_millis(150)).await;

        let start = Instant::now();
        token.cancel();

        // Worker should shut down promptly
        worker_task.await.unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(200),
            "Worker took too long to cancel: {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_worker_drain_batch_success() {
        let (worker, persistence, _, _) = setup_test_worker().await;

        // Insert 3 dispute webhook events
        for i in 1..=3 {
            let event_id = format!("WH-EVT-BATCH-{}", i);
            let dispute_id = format!("PP-D-BATCH-{}", i);
            let payload = serde_json::json!({
                "id": event_id,
                "event_type": "CUSTOMER.DISPUTE.CREATED",
                "resource_type": "dispute",
                "create_time": "2026-11-09T10:00:00Z",
                "resource": {
                    "dispute_id": dispute_id,
                    "status": "UNDER_REVIEW",
                    "reason": "MERCHANDISE_OR_SERVICE_NOT_RECEIVED"
                }
            });
            let raw_bytes = serde_json::to_vec_pretty(&payload).unwrap();
            let parsed: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();

            let res = persistence
                .persist_event(&parsed, &format!("TX-{}", i), &raw_bytes)
                .await
                .unwrap();
            assert!(matches!(res, WebhookPersistenceResult::Inserted(_)));
        }

        // Drain the batch
        let processed = worker.process_available_batch().await.unwrap();
        assert_eq!(processed, 3);

        // Next batch should return 0 (drained)
        let processed_again = worker.process_available_batch().await.unwrap();
        assert_eq!(processed_again, 0);
    }

    #[tokio::test]
    async fn test_worker_notify_wakeup_and_process() {
        let (worker, persistence, notify, _) = setup_test_worker().await;
        let token = CancellationToken::new();
        let token_clone = token.clone();

        // Run worker with longer poll interval to prove wakeup comes from Notify
        let worker_long_poll = worker.clone().with_poll_interval(Duration::from_secs(60));

        let worker_task = tokio::spawn(async move {
            worker_long_poll.run(token_clone).await;
        });

        // Worker is now sleeping on Notify / long poll
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Insert an event
        let payload = serde_json::json!({
            "id": "WH-EVT-NOTIFY-01",
            "event_type": "CUSTOMER.DISPUTE.CREATED",
            "resource_type": "dispute",
            "create_time": "2026-11-09T10:00:00Z",
            "resource": {
                "dispute_id": "PP-D-NOTIFY-01",
                "status": "UNDER_REVIEW",
                "reason": "MERCHANDISE_OR_SERVICE_NOT_RECEIVED"
            }
        });
        let raw_bytes = serde_json::to_vec_pretty(&payload).unwrap();
        let parsed: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();

        persistence
            .persist_event(&parsed, "TX-NOTIFY-01", &raw_bytes)
            .await
            .unwrap();

        // Wake up worker
        notify.notify_one();

        // Give the worker time to process the notified event
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Verify event was processed
        let event = persistence
            .get_event_by_id("WH-EVT-NOTIFY-01")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            event.processing_status,
            crate::models::webhook::WebhookProcessingStatus::Processed
        );

        // Cancel worker
        token.cancel();
        worker_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_worker_respects_max_batch_size() {
        let (worker, persistence, _, _) = setup_test_worker().await;
        let worker_batch_2 = worker.with_max_batch_size(2);

        // Insert 5 events
        for i in 1..=5 {
            let event_id = format!("WH-EVT-LIMIT-{}", i);
            let dispute_id = format!("PP-D-LIMIT-{}", i);
            let payload = serde_json::json!({
                "id": event_id,
                "event_type": "CUSTOMER.DISPUTE.CREATED",
                "resource_type": "dispute",
                "create_time": "2026-11-09T10:00:00Z",
                "resource": {
                    "dispute_id": dispute_id,
                    "status": "UNDER_REVIEW"
                }
            });
            let raw_bytes = serde_json::to_vec_pretty(&payload).unwrap();
            let parsed: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();

            persistence
                .persist_event(&parsed, &format!("TX-{}", i), &raw_bytes)
                .await
                .unwrap();
        }

        // Batch 1: exactly 2
        let b1 = worker_batch_2.process_available_batch().await.unwrap();
        assert_eq!(b1, 2);

        // Batch 2: exactly 2
        let b2 = worker_batch_2.process_available_batch().await.unwrap();
        assert_eq!(b2, 2);

        // Batch 3: remaining 1
        let b3 = worker_batch_2.process_available_batch().await.unwrap();
        assert_eq!(b3, 1);

        // Batch 4: 0
        let b4 = worker_batch_2.process_available_batch().await.unwrap();
        assert_eq!(b4, 0);
    }
}
