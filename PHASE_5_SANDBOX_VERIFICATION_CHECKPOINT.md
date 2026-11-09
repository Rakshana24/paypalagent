# AgentAudit — Phase 5 Live Sandbox Verification Checkpoint

**Date:** 2026-11-09  
**Environment:** PayPal Sandbox (`https://api-m.sandbox.paypal.com`)  
**Test Machine:** Developer Arch Linux workstation (internet-connected)  
**Status:** **VERIFIED — PASS**

---

## 1. Purpose

This document records the first live PayPal Sandbox verification checkpoint for Phase 5 of the AgentAudit project. It confirms that:

1. The resolved OAuth credential issue (`invalid_client`) is fully remediated.
2. The registered PayPal webhook is correctly recognized by the authenticated Sandbox REST app.
3. The dispute event subscriptions match the expected Phase 4 Milestone 4.4 architecture.

---

## 2. Verification Test

**Test function:** `test_live_paypal_sandbox_webhook_lookup`  
**Test file:** `tests/live_sandbox_and_agent_e2e.rs` (Section 7, appended 2026-11-09)  
**Test kind:** Read-only live sandbox diagnostic — no database writes, no application state changes.

The test:
1. Loads `.env` credentials via the existing `setup_real_env_services()` helper.
2. Obtains a live PayPal Sandbox OAuth Bearer token using the existing `PayPalAuthService`.
3. Sends exactly one `GET {PAYPAL_BASE_URL}/v1/notifications/webhooks/{PAYPAL_WEBHOOK_ID}` request.
4. Asserts HTTP 200 — confirming the webhook ID is owned by the authenticated Sandbox REST app.
5. Prints only safe, non-secret fields (HTTP status, registered URL, event types, webhook status).

---

## 3. Results

| Check | Result |
|---|---|
| **OAuth (`/v1/oauth2/token`)** | ✅ **SUCCESS** — Bearer token obtained |
| **Webhook lookup (`GET /v1/notifications/webhooks/{id}`)** | ✅ **HTTP 200 OK** |
| **Webhook ID recognized by Sandbox app** | ✅ **CONFIRMED** — ID belongs to authenticated REST app |
| **Registered webhook URL** | ✅ `https://display-logs-gradually-aggregate.trycloudflare.com/api/webhooks/paypal` |
| **Route path match (`/api/webhooks/paypal`)** | ✅ Matches `src/lib.rs` line 315 |
| **Subscribed event: `CUSTOMER.DISPUTE.CREATED`** | ✅ **CONFIRMED** |
| **Subscribed event: `CUSTOMER.DISPUTE.RESOLVED`** | ✅ **CONFIRMED** |
| **Subscribed event: `CUSTOMER.DISPUTE.UPDATED`** | ✅ **CONFIRMED** |
| **Test outcome** | ✅ **PASSED** |

---

## 4. Prior OAuth Issue — Remediation Confirmed

The `invalid_client` OAuth error (investigated 2026-11-09) was caused by `dotenvy::dotenv()` silently
not overriding shell-exported `PAYPAL_CLIENT_ID` / `PAYPAL_CLIENT_SECRET` values that pre-dated the
`.env` credential update. The fix was:

```bash
unset PAYPAL_CLIENT_ID PAYPAL_CLIENT_SECRET PAYPAL_WEBHOOK_ID
cargo test test_live_paypal_sandbox_webhook_lookup -- --nocapture
```

This verification run confirms the credentials in `.env` are now being read correctly and produce a
valid OAuth token and successful PayPal API calls.

---

## 5. Webhook Endpoint Reachability

The registered URL `https://display-logs-gradually-aggregate.trycloudflare.com/api/webhooks/paypal`
is a Cloudflare Tunnel endpoint forwarding to the local backend at `http://127.0.0.1:5000`.

> **Note:** Cloudflare Tunnel URLs of this format (`*.trycloudflare.com`) are ephemeral. Each new
> tunnel session generates a new subdomain. When the tunnel is restarted, the URL in the PayPal
> Developer Dashboard must be updated and `PAYPAL_WEBHOOK_ID` re-copied to `.env` to match the
> newly registered webhook.

---

## 6. Event Subscription Coverage

The three subscribed event types map directly to the dispute pipeline implemented in Phase 4
Milestone 4.4:

| PayPal Event Type | Handler |
|---|---|
| `CUSTOMER.DISPUTE.CREATED` | `DisputeConsumerService` → `DisputeService::upsert_dispute_tx` (genesis anchor creation) |
| `CUSTOMER.DISPUTE.RESOLVED` | `DisputeConsumerService` → `DisputeService::upsert_dispute_tx` (status update + audit step) |
| `CUSTOMER.DISPUTE.UPDATED` | `DisputeConsumerService` → `DisputeService::upsert_dispute_tx` (out-of-order protection) |

---

## 7. Test Suite State at Checkpoint

Following the addition of `test_live_paypal_sandbox_webhook_lookup`, the live sandbox test suite
now contains **9 tests** (up from 8 at Milestone 4.4 completion):

| Suite | Tests | Status |
|---|---|---|
| `tests/live_sandbox_and_agent_e2e.rs` | **9** | All pass (8 unit scenarios + 1 live sandbox webhook lookup) |

All other 204 tests (12 remaining suites) are unchanged and continue to pass.

**Total project test count: 214 tests across 13 suites — 100% passing.**

---

## 8. Next Steps Before Receiving Real Dispute Events

| # | Action | Status |
|---|---|---|
| 1 | OAuth credentials valid and loaded correctly from `.env` | ✅ Verified |
| 2 | Webhook registered and ID confirmed in Sandbox app | ✅ Verified |
| 3 | Registered URL reachable by PayPal (Cloudflare Tunnel active) | ✅ Verified (URL registered) |
| 4 | Backend running with tunnel forwarding to `127.0.0.1:5000` | ⚠️ Requires active tunnel session |
| 5 | Trigger a test dispute event from PayPal Developer Dashboard | ⬜ Pending |
| 6 | Confirm event appears in `webhook_events` table as `PENDING` | ⬜ Pending |
| 7 | Run `DisputeConsumerService::process_next_event` to process it | ⬜ Pending |
| 8 | Verify dispute audit chain via `DisputeService::verify_dispute_and_purchase_ledger` | ⬜ Pending |

