# AgentAudit — Phase 4 Milestone 4.2 Report
## Webhook Persistence & Idempotency Foundation

### 1. Executive Summary
Milestone 4.2 of Phase 4 establishes the durable, idempotent, and cryptographically verified storage foundation for all incoming PayPal webhook events. Prior to business processing or dispute correlation, incoming webhooks are validated, hashed, and persisted with strict SQLite-level deduplication and state transition rules.

All 99 tests from prior phases continue to pass alongside 16 new unit and integration tests, bringing the total passing test suite to **115 tests (0 failures, 0 warnings, 0 ignored)**.

---

### 2. Files Created and Modified

| File | Type | Description |
| :--- | :--- | :--- |
| `src/db/mod.rs` | Modified | Added migration creating `webhook_events` table with CHECK constraints and performance indexes. |
| `src/error.rs` | Modified | Added `WebhookNotFound`, `InvalidStateTransition`, and `PayloadHashMismatch` error variants with HTTP status mappings. |
| `src/models/webhook.rs` | Modified | Added `WebhookProcessingStatus`, `WebhookEventRecord`, `WebhookPersistenceResult`, `compute_payload_hash`, `calculate_retry_delay_seconds`, resource ID extractor, redaction `Debug`, and unit tests. |
| `src/services/webhook_persistence.rs` | Created | `WebhookPersistenceService` providing atomic insertion (`INSERT ... ON CONFLICT DO NOTHING`), payload hash verification, state transitions, and exponential retry scheduling. |
| `src/services/mod.rs` | Modified | Re-exported `webhook_persistence` module and `WebhookPersistenceService`. |
| `tests/webhook_persistence_tests.rs` | Created | 12 integration tests covering persistence, idempotency, byte preservation, hash verification, backoff, concurrency, and constraints. |
| `README.md` | Modified | Updated project architecture tree with new models, services, and integration test suites. |
| `PHASE_4_MILESTONE_4_2_REPORT.md` | Created | Milestone 4.2 architectural and verification report. |

---

### 3. Database Schema: `webhook_events`

The `webhook_events` table is created via SQL migration in `src/db/mod.rs`:

```sql
CREATE TABLE IF NOT EXISTS webhook_events (
    event_id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    resource_type TEXT,
    resource_id TEXT,
    transmission_id TEXT NOT NULL,
    processing_status TEXT NOT NULL CHECK(processing_status IN ('PENDING', 'PROCESSING', 'PROCESSED', 'RETRY', 'FAILED', 'IGNORED')),
    retry_count INTEGER NOT NULL DEFAULT 0 CHECK(retry_count >= 0),
    max_retries INTEGER NOT NULL DEFAULT 5 CHECK(max_retries >= 0),
    last_error TEXT,
    raw_payload BLOB NOT NULL,
    payload_hash TEXT NOT NULL,
    next_retry_at TEXT,
    created_at TEXT NOT NULL,
    processed_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_webhook_events_status_next_retry
    ON webhook_events(processing_status, next_retry_at);

CREATE INDEX IF NOT EXISTS idx_webhook_events_resource_id
    ON webhook_events(resource_id);

CREATE INDEX IF NOT EXISTS idx_webhook_events_created_at
    ON webhook_events(created_at DESC);
```

#### Schema Constraints & Integrity Rules:
- **`CHECK(processing_status IN (...))`**: Restricts statuses to valid lifecycle states at the database engine level.
- **`CHECK(retry_count >= 0)` & `CHECK(max_retries >= 0)`**: Prevents negative retry or configuration counts.
- **Composite Index `(processing_status, next_retry_at)`**: Optimizes worker queries for pending and retryable events without table scans.
- **Index on `resource_id`**: Accelerates correlation queries matching dispute IDs, order IDs, or capture IDs to existing audit sessions.

---

### 4. Idempotency Design

PayPal guarantees at-least-once delivery; network retries and duplicate transmissions are expected. Milestone 4.2 implements single-statement atomic deduplication:

1. **Atomic Ingestion**:
   ```sql
   INSERT INTO webhook_events (
       event_id, event_type, resource_type, resource_id,
       transmission_id, processing_status, retry_count, max_retries,
       last_error, raw_payload, payload_hash, next_retry_at,
       created_at, processed_at
   )
   VALUES (?, ?, ?, ?, ?, 'PENDING', 0, 5, NULL, ?, ?, NULL, ?, NULL)
   ON CONFLICT(event_id) DO NOTHING;
   ```
2. **Deterministic Outcome Detection**:
   - `rows_affected == 1`: The event is new; returns `WebhookPersistenceResult::Inserted(record)`.
   - `rows_affected == 0`: The event was already stored; returns `WebhookPersistenceResult::Duplicate { event_id }`.
3. **Immutability Guarantees**:
   - Duplicate deliveries never overwrite existing payload bytes.
   - Duplicate deliveries never increment `retry_count`.
   - Duplicate deliveries never alter `created_at` or `processing_status`.
   - Fully immune to race conditions during concurrent webhook deliveries (verified by 20-thread concurrency test).

---

### 5. Raw Payload Integrity Design

- **Original Byte Preservation**: The exact byte stream received over HTTP is stored as a raw SQLite `BLOB` without intermediate deserialization and reserialization. This preserves exact buyer comments, original formatting, whitespace, and special characters.
- **Cryptographic Checksum**: A SHA-256 hash (`payload_hash`) is computed over the stored bytes at ingestion.
- **Tamper Detection on Read**: Every read operation (`get_event_by_id`, `get_pending_events`) recalculates SHA-256 over `raw_payload` and verifies equality with `payload_hash`. Any divergence immediately raises `AppError::PayloadHashMismatch`.
- **Log Protection**: `raw_payload` is never emitted in logs; `WebhookEventRecord` implements custom `Debug` that renders `[PROTECTED_RAW_PAYLOAD: N bytes]` to prevent leaking PII or credentials.

---

### 6. Processing Lifecycle & Exponential Backoff

```
             ┌──────────┐
             │ PENDING  │──────────────┐
             └──────────┘              │
                   │                   │
                   ▼                   ▼
            ┌──────────────┐      ┌─────────┐
     ┌─────►│  PROCESSING  │─────►│ IGNORED │
     │      └──────────────┘      └─────────┘
     │        │          │
     │        ▼          ▼
┌─────────┐ ┌─────────┐ ┌────────┐
│  RETRY  │ │PROCESSED│ │ FAILED │
└─────────┘ └─────────┘ └────────┘
     │                       ▲
     └───────────────────────┘ (Max retries reached)
```

- **State Transitions**:
  - `PENDING` -> `PROCESSING`, `IGNORED`
  - `PROCESSING` -> `PROCESSED`, `RETRY`, `FAILED`, `IGNORED`
  - `RETRY` -> `PROCESSING`, `FAILED`, `IGNORED`
  - `PROCESSED`, `FAILED`, `IGNORED` are terminal states.
- **Retry Backoff**:
  $$\text{delay} = \min(5 \times 2^{\text{retry\_count}}, 3600)\text{ seconds}$$
  - Retry 0 (first failure): 5s delay
  - Retry 1: 10s delay
  - Retry 2: 20s delay
  - Retry 3: 40s delay
  - Retry 4: 80s delay
  - Capped at 3600s (1 hour)
- **Retry Limit**: When `retry_count + 1 >= max_retries` (default 5), the event automatically transitions to `FAILED` with failure details preserved in `last_error` and `processed_at` recorded. Events are never silently deleted.

---

### 7. Verification & Test Counts

All checks and test suites run and pass cleanly:

```bash
cargo fmt --check          # PASS (Clean formatting)
cargo check --tests        # PASS (0 errors, 0 warnings)
cargo build --release      # PASS (Release build succeeded in 16.9s)
cargo test                 # PASS (All 115 tests pass)
```

#### Exact Breakdown by Test Suite:

| Suite | Tests | Result |
| :--- | :---: | :---: |
| `src/lib.rs` (Unit tests) | 32 | PASS |
| `src/main.rs` (Unit test) | 1 | PASS |
| `tests/ledger_tests.rs` (Ledger & hash-chain) | 24 | PASS |
| `tests/live_sandbox_and_agent_e2e.rs` (E2E & PayPal live) | 8 | PASS |
| `tests/oauth_tests.rs` (OAuth engine) | 10 | PASS |
| `tests/shopping_and_paypal_tests.rs` (Shopping & Orders v2) | 16 | PASS |
| `tests/webhook_tests.rs` (Milestone 4.1 signature verification) | 12 | PASS |
| `tests/webhook_persistence_tests.rs` (Milestone 4.2 persistence) | 12 | PASS |
| **Total Test Count** | **115** | **PASS** |

---

### 8. Scope Restrictions & Remaining Limitations

#### Strictly Preserved Scope:
- **No HTTP routes** for webhooks yet (deferred to Milestone 4.3).
- **No dispute database tables** yet (deferred to Milestone 4.4).
- **No asynchronous worker threads/daemons** running in the background.
- **No dispute correlation** or session lookup logic yet.
- **No AI analysis or evidence submission**.
- **Zero changes** to Phase 1–3 business logic or finalized session immutability.

#### Limitations to be Addressed in Subsequent Milestones:
- Background worker to poll `get_pending_events()` will be introduced when the webhook consumer pipeline is connected.
- The dual-audit hash chain connecting purchase sessions to dispute records will be introduced in Milestone 4.4.

