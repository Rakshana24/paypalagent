# AgentAudit — Phase 4 Milestone 4.3 Report
## Deterministic Dispute-to-Session Correlation Engine

### 1. Executive Summary
Milestone 4.3 implements the **Deterministic Dispute-to-Session Correlation Engine** for AgentAudit. The engine deterministically links incoming PayPal dispute notifications or API payloads to their corresponding immutable purchase audit sessions without heuristics or probabilistic matching.

All 115 tests from prior milestones continue to pass without modification, and 18 new unit and integration tests have been introduced, bringing the total passing test suite to **133 tests (0 failures, 0 warnings, 0 ignored)**.

---

### 2. Correlation Hierarchy

The correlation engine strictly enforces the five-tier resolution priority specified for Milestone 4.3:

```
┌─────────────────────────────────────────────────────────────┐
│ Dispute Payload / Webhook Notification Received             │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ Tier 1: PayPal Capture ID                                   │
│ Extract seller_transaction_id; match via partial index on   │
│ PAYPAL_PAYMENT_CAPTURED events in decision_steps.           │
└──────────────────────┬───────────────────────┬──────────────┘
                       │ Matched               │ Not Found / Absent
                       ▼                       ▼
           ┌──────────────────────┐  ┌────────────────────────────────────┐
           │ Session Verification │  │ Tier 2: Custom ID / Reference ID   │
           │ & Cross-Identifier   │  │ Validate UUID; match sessions table│
           │ Consistency Check    │  │ and confirm associated transaction │
           └───────────┬──────────┘  └─────────────────┬──────────────────┘
                       │ Passed                        │ Matched
                       ▼                               ▼
                 [ MATCHED ]                 ┌───────────────────┐
                                             │ Session Verify &  │
                                             │ Consistency Check │
                                             └─────────┬─────────┘
                                                       │ Not Found / Absent
                                                       ▼
                                             ┌────────────────────────────────────┐
                                             │ Tier 3: PayPal Order ID            │
                                             │ Match against                      │
                                             │ sessions.paypal_order_id via index │
                                             └─────────────────┬──────────────────┘
                                                               │ Not Found / Absent
                                                               ▼
                                             ┌────────────────────────────────────┐
                                             │ Tier 4: Show Dispute Details API   │
                                             │ GET /v1/customer/disputes/{id}     │
                                             │ Retrieve authoritative data and    │
                                             │ retry Tiers 1–3                    │
                                             └─────────────────┬──────────────────┘
                                                               │ Not Found / Insufficient
                                                               ▼
                                             ┌────────────────────────────────────┐
                                             │ Tier 5: Manual Review Required     │
                                             │ If conflicting, tampered,          │
                                             │ unfinalized, or insufficient data  │
                                             └────────────────────────────────────┘
```

#### Tier Specifications:
1. **Tier 1: PayPal Capture ID**
   - Extracts `seller_transaction_id` or `capture_id` from the root resource or `disputed_transactions` array.
   - Executes an indexed lookup on `decision_steps` where `action_type = 'PAYPAL_PAYMENT_CAPTURED'` and `json_extract(payload, '$.capture_id') = ?`.
2. **Tier 2: Custom ID / Reference ID**
   - Extracts `custom`, `custom_id`, or `reference_id` (normalizing `session-` prefixes).
   - Validates RFC 4122 UUID format with `uuid::Uuid::parse_str`. Malformed values are rejected.
   - Matches candidate `session_id` against `sessions`.
   - Rejects unconfirmed sessions: verifies the session possesses a finalized, verified cryptographic ledger containing a `COMPLETED` payment capture.
3. **Tier 3: PayPal Order ID**
   - Matches `order_id` against indexed `sessions.paypal_order_id`.
   - Validates that the session is finalized and contains a confirmed `PAYPAL_PAYMENT_CAPTURED` event.
4. **Tier 4: PayPal Show Dispute Details API**
   - If local identifiers are missing or insufficient to establish a match, retrieves full dispute details via PayPal's authenticated REST endpoint `GET /v1/customer/disputes/{dispute_id}` using `PayPalAuthService`.
   - Extracts authoritative transaction IDs from the PayPal API response.
   - Retries the deterministic correlation hierarchy with `MatchedBy::DisputeDetailsCaptureId`, `DisputeDetailsCustomId`, or `DisputeDetailsOrderId`.
   - If the API returns a transient error or HTTP failure, halts safely and escalates to `MANUAL_REVIEW_REQUIRED`.
5. **Tier 5: Manual Review (`MANUAL_REVIEW_REQUIRED`) & Not Found (`NOT_FOUND`)**
   - Any conflicting identifiers (e.g. Capture ID pointing to Session A, but Order ID pointing to Session B) immediately trigger `MANUAL_REVIEW_REQUIRED`.
   - Multiple candidate sessions immediately trigger `MANUAL_REVIEW_REQUIRED`.
   - Tampered ledger hash chains or unfinalized sessions trigger `MANUAL_REVIEW_REQUIRED`.
   - Missing transaction identifiers trigger `MANUAL_REVIEW_REQUIRED`.
   - Explicitly unknown capture or order identifiers return `NOT_FOUND`.
   - **Never matches on buyer name, email, amount, or timestamps**.

---

### 3. Database Changes

Zero existing tables or data records were modified. Two non-breaking, performance-optimizing SQLite indexes were added to `src/db/mod.rs` during migration initialization:

```sql
-- Accelerates Tier 3 Order ID lookups:
CREATE INDEX IF NOT EXISTS idx_sessions_paypal_order_id
    ON sessions(paypal_order_id);

-- Accelerates Tier 1 Capture ID lookups over Phase 3 capture events:
CREATE INDEX IF NOT EXISTS idx_decision_steps_capture_id
    ON decision_steps(json_extract(payload, '$.capture_id'))
    WHERE action_type = 'PAYPAL_PAYMENT_CAPTURED';
```

#### Query Plan Verification
SQLite `EXPLAIN QUERY PLAN` confirms direct index scans:
```
QUERY PLAN
`--SEARCH decision_steps USING INDEX idx_decision_steps_capture_id (<expr>=?)
```

---

### 4. New Files and Modified Files

| File | Type | Description |
| :--- | :--- | :--- |
| `src/models/correlation.rs` | Created | Defined `CorrelationStatus`, `MatchedBy`, `CorrelationResult`, `ExtractedDisputeIdentifiers`, `extract_dispute_identifiers()`, and unit tests. |
| `src/services/dispute_correlation.rs` | Created | `DisputeCorrelationService` implementing the 5-tier correlation hierarchy, Show Dispute Details fallback, ledger verification, and cross-identifier consistency checks. |
| `tests/dispute_correlation_tests.rs` | Created | 15 integration tests covering all 14 required test cases plus webhook envelope integration. |
| `src/db/mod.rs` | Modified | Added `idx_sessions_paypal_order_id` and partial expression index `idx_decision_steps_capture_id`. |
| `src/models/mod.rs` | Modified | Re-exported `correlation` models module. |
| `src/services/mod.rs` | Modified | Re-exported `dispute_correlation` service module. |
| `README.md` | Modified | Updated project architecture tree. |
| `PHASE_4_MILESTONE_4_3_REPORT.md` | Created | Milestone 4.3 architectural, verification, and security report. |

---

### 5. Test Cases and Exact Counts

All 14 specified test scenarios from Milestone 4.3 are implemented in `tests/dispute_correlation_tests.rs`:

| Test Name | Scenario Tested | Outcome / Verification |
| :--- | :--- | :--- |
| `test_01_successful_capture_id_matching` | Tier 1 Capture ID match | `MATCHED`, `matched_by: CAPTURE_ID`, verified session IDs |
| `test_02_successful_custom_id_matching` | Tier 2 Custom ID / Reference ID match | `MATCHED`, `matched_by: CUSTOM_ID`, confirms transaction |
| `test_03_successful_order_id_matching` | Tier 3 Order ID match | `MATCHED`, `matched_by: ORDER_ID`, verified order |
| `test_04_fallback_to_paypal_show_dispute_details` | Tier 4 Fallback via PayPal API | `MATCHED`, `matched_by: DISPUTE_DETAILS_CAPTURE_ID` |
| `test_05_unknown_capture_id` | Unknown capture ID query | `NOT_FOUND`, no session association created |
| `test_06_unknown_order_id` | Unknown order ID query | `NOT_FOUND`, no session association created |
| `test_07_invalid_session_uuid` | Malformed session UUID | Rejected from Tier 2, `NOT_FOUND`, no association |
| `test_08_conflicting_identifiers` | Capture ID and Order ID from different sessions | `MANUAL_REVIEW_REQUIRED`, explanation notes conflict |
| `test_09_multiple_candidate_sessions` | Ambiguous session candidate query | `MANUAL_REVIEW_REQUIRED`, explanation notes multiple candidates |
| `test_10_session_with_invalid_ledger_verification` | Tampered cryptographic hash chain | `MANUAL_REVIEW_REQUIRED`, detects digest mismatch |
| `test_11_unfinalized_session` | Session where `root_hash` is NULL | `MANUAL_REVIEW_REQUIRED`, rejects unfinalized session |
| `test_12_no_available_transaction_identifiers` | Empty dispute payload | `MANUAL_REVIEW_REQUIRED`, notes missing identifiers |
| `test_13_paypal_api_failure_during_fallback` | PayPal Show Dispute Details 500 error | `MANUAL_REVIEW_REQUIRED`, graceful error escalation |
| `test_14_manual_review_result_without_creating_an_incorrect_association` | Demographic/amount match without transaction IDs | `MANUAL_REVIEW_REQUIRED`, prevents false positive |
| `test_correlate_webhook_event_envelope` | Webhook envelope integration | `MATCHED`, end-to-end webhook extraction and resolution |

#### Exact Test Breakdown Across Entire Repository:

| Test Suite | Previous Pass Count | Milestone 4.3 Pass Count | Result |
| :--- | :---: | :---: | :---: |
| `src/lib.rs` (Unit tests) | 32 | 35 (+3 new unit tests) | PASS |
| `src/main.rs` (Unit test) | 1 | 1 | PASS |
| `tests/dispute_correlation_tests.rs` | 0 | 15 (+15 new integration tests) | PASS |
| `tests/ledger_tests.rs` | 24 | 24 | PASS |
| `tests/live_sandbox_and_agent_e2e.rs` | 8 | 8 | PASS |
| `tests/oauth_tests.rs` | 10 | 10 | PASS |
| `tests/shopping_and_paypal_tests.rs` | 16 | 16 | PASS |
| `tests/webhook_persistence_tests.rs` | 12 | 12 | PASS |
| `tests/webhook_tests.rs` | 12 | 12 | PASS |
| **Total Test Count** | **115** | **133** | **PASS** |

---

### 6. Security Considerations

1. **Immutability of Audit Ledgers**:
   - The correlation engine runs completely read-only against `sessions` and `decision_steps`. It never alters, updates, or deletes finalized purchase records.
2. **Cryptographic Integrity Enforcement**:
   - Every candidate session is validated via `VerificationService::verify_session()`. If a session hash chain has been tampered with or contains non-canonical RFC 8785 JSON, it is rejected and flagged for manual review.
3. **Prevention of False Positive Associations**:
   - The correlation engine strictly refuses to match based on buyer names, email addresses, order amounts, or timestamps. Only deterministic transaction identifiers are accepted.
4. **Cross-Identifier Consistency Validation**:
   - When a dispute payload contains multiple identifiers (e.g. capture ID and order ID), all identifiers must agree with the candidate session. Any divergence triggers `MANUAL_REVIEW_REQUIRED`.
5. **UUID Validation Against Injection / Arbitrary Reference IDs**:
   - `custom` and `reference_id` fields are validated using RFC 4122 parsing. Non-UUID strings are rejected before querying the session database.
6. **Graceful Fallback Failure**:
   - PayPal API errors, HTTP 5xx responses, and network timeouts during Tier 4 Show Dispute Details requests are caught safely and transitioned to `MANUAL_REVIEW_REQUIRED` without panics or application instability.

---

### 7. Remaining Limitations

1. **Disputes Table & Dual-Audit Chain (Deferred to Milestone 4.4)**:
   - Durable persistence of dispute records in a dedicated `disputes` table and creation of the dual-audit cryptographic hash chain linking disputes to purchase sessions will be completed in Milestone 4.4.
2. **Public Webhook Route Integration**:
   - The HTTP endpoint binding persisted webhook events to the correlation service is deferred to Milestone 4.4.
3. **AI Dispute Analysis & Evidence Submission (Phase 5)**:
   - Autonomous synthesis of dispute evidence packages and submission via PayPal Customer Disputes API will be implemented in subsequent phases.

