// SPDX-License-Identifier: Apache-2.0
// Ported from aws-durable-execution-sdk-python-testing examples.
// Source: examples/src/callback/callback_simple.py
// See NOTICE for attribution.

//! Durable callback (external approval) workflow.
//!
//! Demonstrates:
//! - `ctx.wait_for_callback()` to suspend until an external system completes a callback.
//! - A “submitter” step that would normally notify a human/system with the callback id.
//!
//! Event (JSON):
//! - `{ "request_id": "req-1", "description": "...", "approver_email": "approver@example.com" }`
//!
//! Result (JSON):
//! - `{ "approved": true, "comment": "...", "decision_time": "..." }`
//!
//! Deployed via `examples/template.yaml`. The validation harness
//! `examples/scripts/validate.py` completes callbacks using
//! `send_durable_execution_callback_success` (Lambda API).

use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

/// Input event for the callback example.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// ID of the request needing approval.
    pub request_id: String,
    /// Description of what needs approval.
    pub description: String,
    /// Email of the approver.
    pub approver_email: String,
}

/// Response from the callback example.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    /// Whether the request was approved.
    pub approved: bool,
    /// Optional comment from the approver.
    pub comment: Option<String>,
    /// Timestamp of the decision.
    pub decision_time: String,
}

/// Data returned by the external approval system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalDecision {
    pub approved: bool,
    pub comment: Option<String>,
}

/// Simulates sending an approval request to an external system.
pub async fn send_approval_request(
    callback_id: &str,
    request: &ApprovalRequest,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // In a real application, you would:
    // 1. Send an email/Slack message/etc. with a link containing the callback_id
    // 2. The approver clicks approve/reject
    // 3. Your webhook handler calls AWS Lambda's CompleteCallback API

    println!(
        "Sending approval request to {} for request {} with callback_id {}",
        request.approver_email, request.request_id, callback_id
    );

    Ok(())
}

/// The durable handler with callback for human approval.
pub async fn approval_handler(
    event: ApprovalRequest,
    ctx: DurableContextHandle,
) -> DurableResult<ApprovalResponse> {
    // Configure callback with timeout
    let callback_config =
        CallbackConfig::<ApprovalDecision>::new().with_timeout(Duration::hours(24)); // Wait up to 24 hours for approval

    // Clone the event for use in the closure
    let request = event.clone();

    // Wait for external approval with a submitter function
    // The submitter is called with the callback ID to notify the external system
    let decision: ApprovalDecision = ctx
        .wait_for_callback(
            Some("wait-for-approval"),
            move |callback_id, _step_ctx| {
                let request = request.clone();
                async move { send_approval_request(&callback_id, &request).await }
            },
            Some(callback_config),
        )
        .await?;

    // Process the approval decision
    if decision.approved {
        println!("Request {} was approved", event.request_id);

        let request_id = event.request_id.clone();

        // Perform the approved action
        ctx.step(
            Some("execute-approved-action"),
            move |step_ctx| async move {
                step_ctx.info(&format!("Executing approved action for {}", request_id));
                // In real code: perform_approved_action(request_id).await?;
                Ok(())
            },
            None,
        )
        .await?;
    }

    Ok(ApprovalResponse {
        approved: decision.approved,
        comment: decision.comment,
        decision_time: chrono::Utc::now().to_rfc3339(),
    })
}

/// Alternative: Create callback handle manually for more control.
#[allow(dead_code)]
pub async fn manual_callback_handler(
    event: ApprovalRequest,
    ctx: DurableContextHandle,
) -> DurableResult<ApprovalResponse> {
    // Create a callback handle without immediately waiting
    let callback_handle: CallbackHandle<ApprovalDecision> =
        ctx.create_callback(Some("approval-callback"), None).await?;

    // Send the callback ID to the external system
    send_approval_request(callback_handle.callback_id(), &event)
        .await
        .map_err(|e| DurableError::step_failed_msg("send-request", 1, e.to_string()))?;

    // Do some other work while waiting...
    ctx.step(
        Some("prepare-for-decision"),
        |step_ctx| async move {
            step_ctx.info("Preparing resources while waiting for approval");
            Ok(())
        },
        None,
    )
    .await?;

    // Now wait for the callback to complete
    let decision = callback_handle.wait().await?;

    Ok(ApprovalResponse {
        approved: decision.approved,
        comment: decision.comment,
        decision_time: chrono::Utc::now().to_rfc3339(),
    })
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let handler = with_durable_execution_service(approval_handler, None);
    lambda_runtime::run(handler).await
}
