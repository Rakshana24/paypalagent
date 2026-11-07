# Phase 3 End-to-End Test Report: AgentAudit

**Date**: 2026-11-07  
**Platform**: Windows x86_64 (MinGW / UCRT Toolchain)  
**Backend Framework**: Rust (stable), Axum 0.7, Tokio, Reqwest 0.12 (rustls), SQLite / SQLx, serde_jcs (RFC 8785)  
**PayPal Environment**: Live PayPal Sandbox (`https://api-m.sandbox.paypal.com`)  
**Overall Verdict**: **PASS (100% VERIFIED)** (End-to-end shopping session, customer authorization, live PayPal Sandbox order creation, manual buyer sandbox approval, live PayPal payment capture, and cryptographic ledger finalization verified with real evidence)

---

## 1. Automated Build & Test Suite Results

| Test Category | Command | Result | Notes |
| :--- | :--- | :---: | :--- |
| Code Formatting | `cargo fmt --check` | **PASS** | 100% compliant with Rust style guidelines |
| Type & Borrow Checking | `cargo check` | **PASS** | 0 errors, 0 warnings |
| Unit & Integration Tests | `cargo test` | **PASS** | **80 unit/integration tests passed; 0 failed; 0 ignored** |
| Release Compilation | `cargo build --release` | **PASS** | Binary compiled in 13.98s (`target/release/agentic_commerce.exe`) |

### Test Breakdown
- `src/lib.rs`: 21 unit tests passed.
- `src/main.rs`: 1 unit test passed (`test_health_check_handler`).
- `tests/ledger_tests.rs`: 24 cryptographic hash-chain and SQLite integrity tests passed (including 8 isolated tamper-detection tests).
- `tests/oauth_tests.rs`: 10 PayPal OAuth token lifecycle and cache regression tests passed.
- `tests/shopping_and_paypal_tests.rs`: 16 catalog, requirement extraction, authorization, and orders API tests passed.
- `tests/live_sandbox_and_agent_e2e.rs`: 8 end-to-end shopping scenario and live sandbox order creation tests passed.
- **Total Test Count**: 22 unit tests + 58 integration tests = **80 passed** (0 failed, 0 ignored).

---

## 2. Live HTTP API Endpoint Verification

All endpoints were tested against the active backend server (`http://127.0.0.1:5000`) using actual JSON payloads:

| HTTP Method & Endpoint | Payload / Parameters | Expected Status | Actual Status | Result | Details |
| :--- | :--- | :---: | :---: | :---: | :--- |
| `GET /health` | None | 200 OK | 200 OK | **PASS** | Returned `{"success": true, "message": "AgenticCommerce backend is running"}` |
| `GET /api/catalog/products` | None | 200 OK | 200 OK | **PASS** | Returned 9 active headphone products in USD |
| `POST /api/shopping/sessions` | `{"prompt": "Buy noise-canceling headphones under $100 with at least 30 hours battery life."}` | 201 Created | 201 Created | **PASS** | Session `2784a7df-761f-4d73-b3cc-fbfc9ef3e0fd` created; selected Anker Soundcore Life Q30 ($79.99, 40h battery, ANC) |
| `POST /api/shopping/sessions/{id}/authorize` | `{"selected_product_id": "hp-anker-q30", "maximum_authorized_amount": 79.99, "currency": "USD", "approved": true}` | 200 OK | 200 OK | **PASS** | Authorization approved; recorded in cryptographic hash-chain at sequence index 11 |
| `POST /api/shopping/sessions/{id}/orders` | `{}` | 201 Created | 201 Created | **PASS** | Created live PayPal Sandbox order `3PV77250SN562884S` via Orders v2 API with interactive approval link |
| `POST /api/shopping/sessions/{id}/capture` (Premature) | `{}` | 422 / 502 | 502 Bad Gateway | **PASS** | Upstream PayPal rejected unapproved order with 422 `ORDER_NOT_APPROVED`; recorded `PAYPAL_PAYMENT_FAILED` without finalizing session |
| `POST /api/shopping/sessions/{id}/capture` (Post-Approval) | `{}` | 200 OK | 200 OK | **PASS** | Captured PayPal order; received `status: "COMPLETED"`, `capture_id: "9BK01861PK399171E"`, finalized session |
| `GET /api/shopping/sessions/{id}` | None | 200 OK | 200 OK | **PASS** | Returned complete finalized session: 17 recorded events, `is_finalized: true`, `final_root_hash: "23f68fa1ac24c2f933f8c7bb6903d902517cdeee5ed8a2d77dfc2db12eb2665b"`, `verification: VALID` |

---

## 3. Shopping Agent Scenario Validation

| Scenario | Input Prompt | Expected Behavior | Actual Behavior | Result |
| :--- | :--- | :--- | :--- | :---: |
| **A. Valid Request** | `"Buy noise-canceling headphones under $100 with at least 30 hours battery life."` | Select product meeting all constraints with highest score. | Selected `Anker Soundcore Life Q30` ($79.99, 40h battery, ANC, 4.5 rating, Score: 154.50). | **PASS** |
| **B. Budget Violation** | `"Buy headphones under $40."` | No product selected; all candidates rejected. | All 9 catalog products rejected (cheapest is $49.99). Session recorded rejection reasons in ledger; `selected_product: None`. | **PASS** |
| **C. Battery Violation** | `"Buy headphones with at least 50 hours battery life."` | Only select products with $\ge 50$ hours. | Selected `Audio-Technica ATH-M20xBT` ($79.00, 60h battery life). | **PASS** |
| **D. Conflicting Requirements** | `"Buy Sony headphones under $50."` | Must not pick non-Sony or violate budget limit. | Rejected all Sony headphones for price ($148.00 and $399.99 > $50.00); rejected all non-Sony candidates for brand mismatch; `selected_product: None`. | **PASS** |
| **E. Unsupported Currency** | `"Buy headphones under 100 EUR"` / `"Buy headphones under 5000 INR"` | Reject with clear validation error; do not silently treat as USD. | Returned 400 Bad Request: `Currency 'EUR' is not supported; please specify USD ($)`. | **PASS** |
| **F. Malformed Prompts** | `""` (empty) / `"Buy noise-canceling headphones"` (no budget) | Reject safely with validation error; no panics or crashes. | Returned 400 Bad Request: `Prompt cannot be empty` and `Could not determine maximum budget`. | **PASS** |

---

## 4. Authorization Security Vectors

| Security Attack Vector | Test Case | Expected Result | Actual Result |
| :--- | :--- | :--- | :--- |
| **Lower Amount** | Customer authorizes $50.00 for a $79.99 product | Rejected: amount less than price | **PASS** (400 Bad Request: `Authorized amount $50.00 is less than product price $79.99`) |
| **Product Mismatch** | Customer authorizes product `hp-sony-wh1000xm5` when agent selected `hp-anker-q30` | Rejected: product ID mismatch | **PASS** (400 Bad Request: `does not match selected product`) |
| **Buyer Declined** | Customer submits `approved: false` | Rejected: explicit declination | **PASS** (400 Bad Request: `Customer explicitly declined purchase authorization`) |
| **Missing Auth** | Calling `create_order` before `authorize_purchase` | Rejected: cannot create order | **PASS** (400 Bad Request: `No purchase authorization found for session`) |
| **Duplicate Auth** | Submitting second authorization on same session | Rejected: duplicate authorization | **PASS** (400 Bad Request: `Purchase authorization has already been recorded for this session`) |
| **Post-Finalization Auth**| Submitting authorization after session finalized | Rejected: session finalized | **PASS** (409 Conflict: `Session is already finalized`) |
| **Invalid Currency** | Customer authorizes in `EUR` | Rejected: currency mismatch | **PASS** (400 Bad Request: `Currency 'EUR' does not match selected product currency 'USD'`) |

---

## 5. Ledger Cryptographic Integrity & Tamper Detection

1. **SHA-256 Hash Chain**:
   - Every event includes `current_hash = SHA256(canonical_json(sequence_index, session_id, timestamp, action_type, payload, previous_hash))`.
   - Genesis block uses 64 zeros (`000...000`).
   - Every subsequent event cryptographically seals the previous event hash.
2. **RFC 8785 Canonical JSON (`serde_jcs`)**:
   - Keys are sorted lexicographically by UTF-16 code units.
   - Whitespace and non-canonical representations are strictly normalized.
3. **Database Integrity & Tamper Vectors**:
   - Foreign keys enforce parent-child relations (`decision_steps` $\to$ `sessions`).
   - `UNIQUE(session_id, sequence_index)` prevents race conditions and insertion collisions.
   - Tamper detector verified 8 isolated tampering vectors (modified previous hash, deleted intermediate event, sequence gap, injected event into finalized session, modified action type, non-canonical payload formatting, tampered payload data, and root hash mismatch). All returned `TAMPERED` with exact failure explanation.

---

## 6. Live PayPal Sandbox Orders API v2 Integration

- **PayPal Sandbox OAuth**: Tested against `https://api-m.sandbox.paypal.com/v1/oauth2/token`. Token obtained and cached securely in memory.
- **Order Creation**: Executed real `POST /v2/checkout/orders` with `CAPTURE` intent for $79.99 USD.
  - **Live PayPal Order ID**: `3PV77250SN562884S`
  - **Initial Order Status**: `CREATED` (transitioned to `PAYER_ACTION_REQUIRED` pending buyer approval)
  - **Live Buyer Approval URL**: `https://www.sandbox.paypal.com/checkoutnow?token=3PV77250SN562884S`
  - **Associated Session ID**: `2784a7df-761f-4d73-b3cc-fbfc9ef3e0fd`
- **Premature Capture Test**: Executed capture before buyer approval. Upstream PayPal returned 422 Unprocessable Entity (`ORDER_NOT_APPROVED`). System safely recorded `PAYPAL_PAYMENT_FAILED` into hash chain and kept session unfinalized (**PASS**).
- **Buyer Sandbox Approval**: Manually approved by user via PayPal Sandbox US Personal account (**PASS**).
- **Live Payment Capture Execution**: Executed `POST /api/shopping/sessions/2784a7df-761f-4d73-b3cc-fbfc9ef3e0fd/capture` against PayPal Sandbox (**PASS**):
  - **PayPal Capture ID**: `9BK01861PK399171E`
  - **Capture Transaction Status**: `COMPLETED`
  - **Order Status Post-Capture**: `COMPLETED`
  - **Session Finalized**: `true`
  - **Final Root Hash**: `23f68fa1ac24c2f933f8c7bb6903d902517cdeee5ed8a2d77dfc2db12eb2665b`
  - **Calculated Root Hash**: `23f68fa1ac24c2f933f8c7bb6903d902517cdeee5ed8a2d77dfc2db12eb2665b`
  - **Verification Status**: `VALID` (17 continuous steps verified)
- **Post-Finalization Immutability**: Tested duplicate capture and post-finalization event appending; both rejected with HTTP 409 Conflict (**PASS**).

---

## 7. Edge Cases & Fault Tolerance

| Edge Case | Description | Result |
| :--- | :--- | :---: |
| **Invalid Session ID** | Passing nonexistent UUID to `/api/shopping/sessions/{id}` | **PASS** (404 Not Found) |
| **Nonexistent PayPal Order** | Capturing a session without an order | **PASS** (400 Bad Request) |
| **Premature Capture** | Attempting capture prior to buyer Sandbox login | **PASS** (502 Gateway Error / 422 PayPal `ORDER_NOT_APPROVED`; session unfinalized) |
| **Post-Finalization Append** | Appending events to finalized session | **PASS** (409 Conflict; immutability preserved) |

---

## 8. Bugs Discovered & Fixes Made

1. **Bug**: In Scenario D, prompt `"Buy Sony headphones under $50"` caused the agent to select Anker Q20 because brand was treated purely as a soft scoring bonus (+10) rather than a constraint.
   - **Fix**: Updated `src/services/shopping_agent.rs` to enforce brand matching when explicitly requested by the customer. Products not matching the requested brand are recorded with explicit rejection reasons (`"Brand 'Anker' does not match requested brand 'Sony'"`).
2. **Bug**: `src/http_client.rs` had a 10s connection timeout which caused occasional transient connect timeouts to PayPal Sandbox over high-latency WAN connections.
   - **Fix**: Updated `HttpClientConfig` default connection timeout to 30s and request timeout to 45s.
3. **Bug**: Tracing string in `src/services/paypal_orders.rs` attempted to interpolate `id` instead of `order_id`.
   - **Fix**: Corrected tracing log message.

4. **Bug**: PayPal Sandbox web checkout showed `genericError?code=...` ("Things don't appear to be working at the moment") because Orders v2 web flow was missing `payment_source.paypal.experience_context` containing explicit `return_url` and `cancel_url`.
   - **Fix**: Added `PayPalExperienceContext`, `PayPalExperienceWrapper`, and `PayPalPaymentSource` to `src/models/paypal_orders.rs` and populated them in `src/services/paypal_orders.rs` with `brand_name: "AgenticCommerce"`, `landing_page: "LOGIN"`, `user_action: "PAY_NOW"`, and local redirect endpoints (`/api/shopping/checkout/success` and `/api/shopping/checkout/cancel`). Added corresponding handlers in `src/lib.rs`.

---

## 9. Verification Lifecycle Completion

All manual and automated verification milestones have been completed successfully:

1. **Buyer Action**: PayPal Sandbox checkout link was opened in an isolated browser session:
   `https://www.sandbox.paypal.com/checkoutnow?token=3PV77250SN562884S`
2. **Buyer Approval**: Transaction was approved using the PayPal Sandbox US Personal buyer account.
3. **Capture Execution**: `POST /api/shopping/sessions/2784a7df-761f-4d73-b3cc-fbfc9ef3e0fd/capture` executed against PayPal Sandbox API:
   - PayPal returned `status: "COMPLETED"` and Capture ID `9BK01861PK399171E`.
4. **Session Finalization**:
   - `is_finalized`: `true`
   - `final_root_hash`: `23f68fa1ac24c2f933f8c7bb6903d902517cdeee5ed8a2d77dfc2db12eb2665b`
   - Total events in hash chain: `17`
   - Hash chain integrity: `VALID` (17/17 continuous steps verified)
5. **Post-Finalization Tamper Invariance**:
   - Duplicate capture request rejected with HTTP 409 Conflict.
   - Illegal post-finalization event append rejected with HTTP 409 Conflict.

---

## 10. Final Verdict

**PHASE 3 STATUS: 100% PASS & VERIFIED**
- All 80 automated tests passed (22 unit tests + 58 integration tests).
- All 6 shopping agent scenarios (A through F) validated.
- All 7 authorization security vectors validated.
- Full cryptographic ledger hash-chain integrity verified.
- Live PayPal Sandbox Orders API v2 OAuth, order creation, buyer approval, payment capture, and session finalization verified with actual evidence.

