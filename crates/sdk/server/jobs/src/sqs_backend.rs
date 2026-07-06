//! AWS SQS-backed [`QueueBackend`]. Pairs with the `server-aws` Lambda adapter.
//!
//! SQS provides the reliability primitives natively, so the mapping is thin:
//! - **reserve** = `ReceiveMessage` with a visibility timeout (the lease).
//! - **attempt** = the message's `ApproximateReceiveCount` system attribute —
//!   SQS counts deliveries for us, so nothing is stored in the body.
//! - **ack** = `DeleteMessage`.
//! - **retry after `delay`** = `ChangeMessageVisibility` to `delay` seconds (the
//!   message reappears then; the receive count keeps climbing).
//! - **dead_letter** = send the body to the configured DLQ (if any) then delete.
//!
//! Each logical queue name maps to one SQS queue URL. `delay` and visibility are
//! second-granular and capped at SQS limits (delay ≤ 15 min).

use crate::persist::BackoffRepr;
use crate::{JobId, OutgoingJob, QueueBackend, QueueError, ReserveOpts, ReservedJob};
use async_trait::async_trait;
use aws_sdk_sqs::types::MessageSystemAttributeName;
use aws_sdk_sqs::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// The message body stored in SQS. `attempt` is NOT here — it comes from SQS's
/// receive count.
#[derive(Serialize, Deserialize)]
struct Envelope {
    name: String,
    payload: Vec<u8>,
    max_attempts: u32,
    /// `BackoffRepr` JSON.
    backoff: String,
}

/// A [`QueueBackend`] backed by AWS SQS.
#[derive(Clone)]
pub struct SqsBackend {
    client: Client,
    /// Logical queue name → SQS queue URL.
    queues: HashMap<String, String>,
    /// Optional dead-letter queue URL.
    dlq: Option<String>,
}

fn be<E: std::fmt::Display>(e: E) -> QueueError {
    QueueError::Backend(e.to_string())
}

fn ms_to_secs_capped(ms: u128, cap: i32) -> i32 {
    let secs = (ms / 1000) as i64;
    secs.clamp(0, cap as i64) as i32
}

impl SqsBackend {
    /// Build from AWS environment config, mapping the `default` queue to
    /// `queue_url`.
    pub async fn connect(queue_url: &str) -> Result<Self, QueueError> {
        let mut queues = HashMap::new();
        queues.insert("default".to_string(), queue_url.to_string());
        Self::with_queues(queues).await
    }

    /// Build with an explicit logical-name → URL map.
    pub async fn with_queues(queues: HashMap<String, String>) -> Result<Self, QueueError> {
        let cfg = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .load()
            .await;
        Ok(Self {
            client: Client::new(&cfg),
            queues,
            dlq: None,
        })
    }

    /// Build from a resolved AWS [`SdkConfig`](aws_config::SdkConfig) and an
    /// explicit logical-name → URL map. This is the shape `idealyst-config`
    /// produces from a `[connections.<name>]` AWS profile, so jobs and email
    /// can share one account.
    pub fn from_aws(config: &aws_config::SdkConfig, queues: HashMap<String, String>) -> Self {
        Self {
            client: Client::new(config),
            queues,
            dlq: None,
        }
    }

    /// Route dead-lettered jobs to this queue URL (otherwise they're deleted).
    pub fn dead_letter_url(mut self, url: impl Into<String>) -> Self {
        self.dlq = Some(url.into());
        self
    }

    fn url(&self, queue: &str) -> Result<&str, QueueError> {
        self.queues
            .get(queue)
            .map(String::as_str)
            .ok_or_else(|| QueueError::Backend(format!("no SQS queue URL configured for `{queue}`")))
    }
}

#[async_trait]
impl QueueBackend for SqsBackend {
    async fn enqueue(&self, job: OutgoingJob) -> Result<JobId, QueueError> {
        let url = self.url(&job.queue)?;
        let body = serde_json::to_string(&Envelope {
            name: job.name,
            payload: job.payload,
            max_attempts: job.max_attempts.max(1),
            backoff: BackoffRepr::to_json(&job.backoff),
        })
        .map_err(|e| QueueError::Codec(e.to_string()))?;

        let mut req = self
            .client
            .send_message()
            .queue_url(url)
            .message_body(body);
        if let Some(d) = job.delay {
            // SQS DelaySeconds is capped at 900 (15 min).
            req = req.delay_seconds(ms_to_secs_capped(d.as_millis(), 900));
        }
        let out = req.send().await.map_err(be)?;
        Ok(JobId(out.message_id().unwrap_or_default().to_string()))
    }

    async fn reserve(&self, opts: &ReserveOpts) -> Result<Option<ReservedJob>, QueueError> {
        let visibility = ms_to_secs_capped(opts.visibility.as_millis(), 43_200); // ≤ 12h
        for q in &opts.queues {
            let url = self.url(q)?;
            let out = self
                .client
                .receive_message()
                .queue_url(url)
                .max_number_of_messages(1)
                .visibility_timeout(visibility)
                .wait_time_seconds(1)
                .message_system_attribute_names(MessageSystemAttributeName::ApproximateReceiveCount)
                .send()
                .await
                .map_err(be)?;

            let Some(msg) = out.messages().first() else {
                continue;
            };
            let Some(receipt) = msg.receipt_handle() else {
                continue;
            };
            let body = msg.body().unwrap_or_default();
            let env: Envelope =
                serde_json::from_str(body).map_err(|e| QueueError::Codec(e.to_string()))?;

            let attempt = msg
                .attributes()
                .and_then(|a| a.get(&MessageSystemAttributeName::ApproximateReceiveCount))
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(1)
                .max(1);

            return Ok(Some(ReservedJob {
                id: JobId(msg.message_id().unwrap_or_default().to_string()),
                queue: q.clone(),
                name: env.name,
                payload: env.payload,
                attempt,
                max_attempts: env.max_attempts.max(1),
                backoff: BackoffRepr::from_json(&env.backoff),
                receipt: receipt.to_string(),
            }));
        }
        Ok(None)
    }

    async fn ack(&self, job: &ReservedJob) -> Result<(), QueueError> {
        let url = self.url(&job.queue)?;
        self.client
            .delete_message()
            .queue_url(url)
            .receipt_handle(&job.receipt)
            .send()
            .await
            .map_err(be)?;
        Ok(())
    }

    async fn retry(&self, job: &ReservedJob, delay: Duration) -> Result<(), QueueError> {
        let url = self.url(&job.queue)?;
        // Make the message reappear after `delay` (receive count keeps climbing).
        self.client
            .change_message_visibility()
            .queue_url(url)
            .receipt_handle(&job.receipt)
            .visibility_timeout(ms_to_secs_capped(delay.as_millis(), 43_200))
            .send()
            .await
            .map_err(be)?;
        Ok(())
    }

    async fn dead_letter(&self, job: &ReservedJob, reason: &str) -> Result<(), QueueError> {
        // Forward the body (plus a reason) to the DLQ if configured, then delete
        // it from the source so it isn't redelivered.
        if let Some(dlq) = &self.dlq {
            let body = serde_json::to_string(&Envelope {
                name: format!("{} (dead: {reason})", job.name),
                payload: job.payload.clone(),
                max_attempts: job.max_attempts,
                backoff: BackoffRepr::to_json(&job.backoff),
            })
            .map_err(|e| QueueError::Codec(e.to_string()))?;
            self.client
                .send_message()
                .queue_url(dlq)
                .message_body(body)
                .send()
                .await
                .map_err(be)?;
        }
        let url = self.url(&job.queue)?;
        self.client
            .delete_message()
            .queue_url(url)
            .receipt_handle(&job.receipt)
            .send()
            .await
            .map_err(be)?;
        Ok(())
    }
}
