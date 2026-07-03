//! Live host test for the SQS backend. `#[ignore]` — needs AWS credentials in
//! the environment and a real (ideally short-poll) SQS queue. Run with:
//!
//! ```sh
//! JOBS_TEST_SQS_URL=https://sqs.us-east-1.amazonaws.com/123456789012/jobstest \
//!   AWS_REGION=us-east-1 \
//!   cargo test -p jobs --features sqs -- --ignored
//! ```
#![cfg(feature = "sqs")]

use jobs::{OutgoingJob, QueueBackend, ReserveOpts, SqsBackend};
use std::time::Duration;

#[tokio::test]
#[ignore = "requires AWS credentials + a live SQS queue (JOBS_TEST_SQS_URL)"]
async fn sqs_enqueue_reserve_ack_roundtrip() {
    let url = std::env::var("JOBS_TEST_SQS_URL").expect("JOBS_TEST_SQS_URL");
    let b = SqsBackend::connect(&url).await.expect("connect to sqs");

    b.enqueue(OutgoingJob::new("hello", b"payload".to_vec()))
        .await
        .unwrap();

    // SQS is eventually consistent; poll a few times.
    let opts = ReserveOpts {
        queues: vec!["default".into()],
        visibility: Duration::from_secs(30),
    };
    let mut reserved = None;
    for _ in 0..10 {
        if let Some(r) = b.reserve(&opts).await.unwrap() {
            reserved = Some(r);
            break;
        }
    }
    let r = reserved.expect("should receive the message");
    assert_eq!(r.name, "hello");
    assert_eq!(r.payload, b"payload");
    assert!(r.attempt >= 1);
    b.ack(&r).await.unwrap();
}
