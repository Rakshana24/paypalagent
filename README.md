# AgentAudit: AI Commerce Shopping Agent & Verifiable Cryptographic Ledger

**AgentAudit** is a verifiable, audit-grade commerce platform built in Rust for the PayPal AI Hackathon. It pairs an autonomous AI shopping agent with an append-only, tamper-evident cryptographic decision ledger, real PayPal Sandbox Orders v2 checkout, PayPal webhook signature verification, and deterministic dispute-to-session correlation with dual-chain evidence verification.

---

## 1. Project Overview & Architecture

Modern AI shopping agents make autonomous procurement decisions that involve real money. If an agent selects the wrong item, violates a budget, or a customer later disputes a charge, traditional logs can be edited, deleted, or disputed.

AgentAudit solves this by creating two intertwined, cryptographically verifiable Merkle/hash-chain ledgers:
1. **Purchase Decision Ledger**: Records every decision step (`REQUIREMENTS_EXTRACTED`, `PRODUCT_SELECTED`, `PURCHASE_AUTHORIZED`, `PAYPAL_ORDER_CREATED`, `PAYPAL_PAYMENT_CAPTURED`) using RFC 8785 JSON Canonicalization Scheme (JCS) and SHA-256 chaining. Once captured, the purchase session is finalized and sealed with a frozen root hash.
2. **Dispute Audit Ledger**: When a PayPal dispute webhook arrives, the dispute is persisted and anchored either to the finalized purchase session root hash (`PURCHASE_ROOT_HASH`) or to a genesis anchor (`GENESIS_HASH`). Every subsequent event appends an RFC 8785 audit step, creating an immutable history of dispute lifecycle events.
3. **Dual-Chain Verification**: Auditors can cryptographically prove the entire chain of custody: the dispute root hash verifies the dispute history, which links to the purchase session root hash, which verifies every decision the AI agent made leading up to the purchase.

```
                           [ Customer / Client ]
                                     |
               +---------------------+---------------------+
               |                                           |
               v                                           v
    [ Shopping Agent Flow ]                     [ PayPal Webhook Flow ]
               |                                           |
    POST /api/shopping/sessions                 POST /api/webhooks/paypal
               |                                           |
     +---------+---------+                     +-----------+-----------+
     | Requirements      |                     | 1. Size limit (256KB) |
     | Catalog selection |                     | 2. Header extraction  |
     | Budget check      |                     | 3. Raw byte parsing   |
     +---------+---------+                     | 4. Verify signature   |
               |                               | 5. Persist raw bytes  |
    POST .../authorize (Customer)              +-----------+-----------+
               |                                           | (PENDING)
    POST .../orders (PayPal v2)                            v
               |                              [ DisputeConsumerService ]
    POST .../capture (PayPal v2)                           | (BEGIN IMMEDIATE)
               |                               +-----------+-----------+
    POST .../finalize                          | Tier 1: Capture ID    |
               |                               | Tier 2: Custom/UUID   |
     [ Finalized Session ]                     | Tier 3: Order ID      |
     [ Frozen Root Hash  ]                     +-----------+-----------+
               ^                                           |
               |=========== Genesis Anchor ================|
                                                           v
                                                [ Dispute Audit Ledger ]
                                                [ Dual-Chain Integrity ]
```

---

## 2. Main Features & Supported Workflows

- **Deterministic AI Shopping Agent**: Extracts budget, brand, and specifications from natural language prompts, selects matching products, rejects invalid/out-of-budget requests, and logs all evaluations.
- **Explicit Purchase Authorization**: Prevents unapproved purchases. Orders cannot be created or captured without an explicit customer authorization step recorded in the ledger.
- **PayPal Orders v2 Integration**: Generates sandbox approval URLs, tracks order lifecycle, and executes payment captures.
- **Verifiable Decision Ledger**: RFC 8785 (JCS) deterministic canonical serialization, SHA-256 hash chaining, SQLite write-ahead logging (WAL), and single-call cryptographic verification (`GET /api/sessions/:id/verify`).
- **PayPal Webhook Ingress**:
  - Retains and persists original raw request bytes.
  - Verifies PayPal transmission headers and RSA-SHA256 signatures before persistence.
  - Rejects duplicate deliveries with divergent payloads (`409 Conflict`).
  - Idempotently acknowledges duplicate deliveries (`200 OK`).
- **Dispute Consumer & Worker Fencing**:
  - Leases pending events using cryptographically random UUID fencing tokens.
  - Automatic reclamation of abandoned events after 300 seconds.
  - Single atomic `BEGIN IMMEDIATE` transaction coordinating dispute ingestion and fenced status transitions.
  - Stale worker fencing: Reclaimed leases cannot commit dispute mutations.
- **Deterministic 5-Tier Dispute Correlation**:
  - Matches disputes to purchase sessions via Capture ID, Custom ID / Session UUID, Order ID, and fallback Show Dispute Details API.
  - Strictly requires candidate sessions to be finalized and verified.
  - Supports late correlation when an initially unmatched dispute later matches a finalized session.
- **Out-of-Order Hardening**:
  - Authoritative PayPal timestamps (`dispute_create_time`, `dispute_update_time`) drive updates.
  - Older out-of-order webhooks cannot overwrite newer dispute status.

---

## 3. Technology Stack

- **Language**: Rust (2021 edition, stable toolchain)
- **Web Framework**: [Axum](https://github.com/tokio-rs/axum) (v0.7) with Tower
- **Asynchronous Runtime**: [Tokio](https://tokio.rs/) (v1.x, full features)
- **Database Engine**: [SQLx](https://github.com/launchbadge/sqlx) (v0.9) with SQLite (`WAL` mode, foreign keys, 5s busy timeout)
- **Cryptography**: [sha2](https://github.com/RustCrypto/hashes) (SHA-256), [hex](https://github.com/KokaKiwi/rust-hex)
- **Canonical Serialization**: [serde_jcs](https://crates.io/crates/serde_jcs) (RFC 8785 JSON Canonicalization Scheme), `serde`, `serde_json`
- **HTTP Client**: [Reqwest](https://github.com/seanmonstar/reqwest) (v0.12, JSON, rustls-tls)
- **Identifiers & Time**: [uuid](https://github.com/uuid-rs/uuid) (v4), [chrono](https://github.com/chronotope/chrono) (UTC RFC 3339)
- **Logging & Diagnostics**: [tracing](https://github.com/tokio-rs/tracing), `tracing-subscriber` (with secret redaction)

---

## 4. Prerequisites & Setup Instructions

### Prerequisites
- **Rust Toolchain**: 1.75+ (recommended: latest stable `rustup update stable`)
- **SQLite**: Embedded via `sqlx` (no external database server needed)
- **PayPal Developer Account**: Sandbox account with Client ID and Client Secret from the [PayPal Developer Dashboard](https://developer.paypal.com/)

### Installation
```bash
# Clone the repository
git clone https://github.com/your-org/paypalagent.git
cd paypalagent

# Copy environment configuration
cp .env.example .env
```

---

## 5. Environment-Variable Configuration

Configure the application by editing `.env` (or setting environment variables):

```bash
# Server Configuration
PORT=5000
RUST_LOG=agentic_commerce=info,tower_http=info,axum=info

# Embedded SQLite Database Configuration
DATABASE_URL=sqlite:data/agentic_commerce.db?mode=rwc

# PayPal Sandbox API Configuration
PAYPAL_BASE_URL=https://api-m.sandbox.paypal.com
PAYPAL_CLIENT_ID=your_paypal_sandbox_client_id_here
PAYPAL_CLIENT_SECRET=your_paypal_sandbox_client_secret_here

# PayPal Webhook Configuration
PAYPAL_WEBHOOK_ID=your_paypal_sandbox_webhook_id_here
```

> **Security Note:** Secrets in `.env` are strictly excluded from source control via `.gitignore`. The `Config` struct enforces secret redaction in all `Debug` and logging outputs.

---

## 6. Database Migrations & Startup

AgentAudit uses an embedded SQLite database with automatic transactional migrations on startup:
- Creates required tables (`sessions`, `decision_steps`, `webhook_events`, `disputes`, `dispute_audit_steps`).
- Configures `PRAGMA journal_mode = WAL;`, `PRAGMA synchronous = NORMAL;`, `PRAGMA busy_timeout = 5000;`, and `PRAGMA foreign_keys = ON;`.
- Installs the genesis anchor immutability trigger `trg_protect_dispute_genesis_anchor`.
- Seamlessly applies forward migrations if upgrading from an earlier schema.

### Starting the Server
```bash
cargo run
```
The server will bind to `http://127.0.0.1:5000`.

---

## 7. Available HTTP Endpoints

### System & Health
- `GET /health`: Health check endpoint. Returns `{"success": true, "message": "AgenticCommerce backend is running"}`.
- `GET /api/paypal/auth-check`: Verifies PayPal Sandbox OAuth connectivity and returns cached token status without leaking credentials.

### Shopping Agent & Purchase Flow
- `GET /api/catalog/products`: Returns the deterministic mock headphone product catalog.
- `POST /api/shopping/sessions`: Submits a shopping prompt (e.g. `{"prompt": "Buy noise canceling headphones under $100"}`). Evaluates budget, selects product, and initializes purchase ledger.
- `GET /api/shopping/sessions/:session_id`: Returns session status, requirement extraction details, and product candidate evaluations.
- `POST /api/shopping/sessions/:session_id/authorize`: Submits customer purchase authorization (`{"authorized": true}`). Records `PURCHASE_AUTHORIZED` in ledger.
- `POST /api/shopping/sessions/:session_id/orders`: Creates a real PayPal Sandbox Order v2 and returns the buyer approval link.
- `POST /api/shopping/sessions/:session_id/capture`: Captures PayPal payment after buyer approval, appends `PAYPAL_PAYMENT_CAPTURED`, and automatically finalizes the ledger.

### Verifiable Ledger & Audit
- `GET /api/sessions/:session_id`: Retrieves session metadata.
- `GET /api/sessions/:session_id/events`: Retrieves all decision steps in sequence.
- `GET /api/sessions/:session_id/verify`: Executes full cryptographic verification of the hash-chain ledger, recomputing all step hashes and verifying the root hash.

### PayPal Webhook Ingress
- `POST /api/webhooks/paypal`: Ingests incoming PayPal webhook deliveries.
  - Requires transmission headers (`paypal-auth-algo`, `paypal-cert-url`, `paypal-transmission-id`, `paypal-transmission-sig`, `paypal-transmission-time`).
  - Verifies signature against PayPal's verification API.
  - Persists exact raw bytes to SQLite as `PENDING`.
  - Returns `202 Accepted` for new events, `200 OK` for duplicate events, `409 Conflict` for divergent duplicates, `401 Unauthorized` for bad signatures, and `400 Bad Request` for missing headers or bad JSON.

---

## 8. PayPal Sandbox Setup & Webhook Configuration

1. Log in to the [PayPal Developer Dashboard](https://developer.paypal.com/).
2. Create a REST API app under **Sandbox Apps & Credentials**.
3. Copy **Client ID** and **Client Secret** into your `.env`.
4. In the PayPal Developer Dashboard under your App settings, click **Add Webhook**.
5. Set Webhook URL (e.g. using ngrok for local testing: `https://<your-subdomain>.ngrok.app/api/webhooks/paypal`).
6. Select events to subscribe to:
   - `Customer dispute created` (`CUSTOMER.DISPUTE.CREATED`)
   - `Customer dispute resolved` (`CUSTOMER.DISPUTE.RESOLVED`)
   - `Customer dispute updated` (`CUSTOMER.DISPUTE.UPDATED`)
   - `Payment capture completed` (`PAYMENT.CAPTURE.COMPLETED`)
7. Copy the generated **Webhook ID** and set `PAYPAL_WEBHOOK_ID` in `.env`.

---

## 9. Shopping Agent & Purchase Authorization Flow

```
1. Customer submits prompt: "Buy Sony WH-1000XM5 under $400"
   --> Agent parses budget ($400), brand (Sony), features (ANC).
   --> Matches catalog product ($399.99).
   --> Creates session in database with genesis hash.

2. Explicit Authorization Gate:
   --> Order CANNOT be placed without authorization.
   --> Customer reviews and calls POST .../authorize.
   --> Ledger records PURCHASE_AUTHORIZED step.

3. PayPal Sandbox Checkout:
   --> POST .../orders calls PayPal Orders v2 API with custom_id = session_id.
   --> Customer approves payment in PayPal Sandbox portal.
   --> POST .../capture executes payment capture.
   --> Ledger records PAYPAL_PAYMENT_CAPTURED and finalizes session.
   --> Root hash is sealed and immutable.
```

---

## 10. Dispute Ingestion, Correlation & Evidence Verification

```
1. Customer opens dispute in PayPal Sandbox.
2. PayPal transmits webhook to POST /api/webhooks/paypal:
   - Ingress verifies RSA-SHA256 signature via PayPal API.
   - Exact raw bytes saved in SQLite with PENDING status.
3. DisputeConsumerService acquires lease:
   - Generates UUID fencing token.
   - Runs deterministic local correlation (Tiers 1-3).
   - In single BEGIN IMMEDIATE transaction:
     * Upserts dispute record anchored to purchase root hash.
     * Appends RFC 8785 canonical audit step.
     * Marks webhook PROCESSED under fencing token.
4. Dual-Chain Verification:
   - Verifies dispute audit steps from genesis anchor to dispute root hash.
   - Verifies purchase session ledger from genesis hash to purchase root hash.
   - Proves zero tampering across the complete shopping and dispute lifecycle.
```

---

## 11. Test Commands

Run the comprehensive project test suite:

```bash
# Check code formatting
cargo fmt --check

# Validate all targets and tests compile without warnings
cargo check --all-targets

# Run the complete test suite (213 tests across 13 test suites)
cargo test

# Build optimized release binary
cargo build --release
```

### Test Suite Structure
| Test Suite | File | Tests | Purpose |
|---|---|---|---|
| Library Unit Tests | `src/lib.rs` | 52 | Canonical hashing, models, error formatting, migrations |
| Binary Unit Tests | `src/main.rs` | 1 | Health check integration |
| Dispute Webhook E2E | `tests/dispute_webhook_e2e_tests.rs` | 11 | End-to-end webhook ingress to dual-chain verification |
| Dispute Consumer | `tests/dispute_consumer_tests.rs` | 16 | Worker fencing, lease reclamation, retry backoff |
| Dispute Correlation | `tests/dispute_correlation_tests.rs` | 15 | 5-tier deterministic correlation hierarchy |
| Dispute Service | `tests/dispute_service_tests.rs` | 17 | Immutability triggers, out-of-order updates, late correlation |
| Webhook HTTP Route | `tests/webhook_http_tests.rs` | 11 | HTTP status codes, headers, size limits, idempotency |
| Webhook Persistence | `tests/webhook_persistence_tests.rs` | 20 | Byte preservation, SHA-256 divergence, concurrency |
| Webhook Verification | `tests/webhook_tests.rs` | 12 | PayPal signature verification client, cert URL validation |
| Ledger Tests | `tests/ledger_tests.rs` | 24 | RFC 8785 hashing, tamper detection, finalization |
| Shopping & PayPal | `tests/shopping_and_paypal_tests.rs` | 16 | Shopping agent selection, authorization gate, order capture |
| OAuth Tests | `tests/oauth_tests.rs` | 10 | PayPal OAuth token caching, expiration refresh, mock errors |
| Live Sandbox & E2E | `tests/live_sandbox_and_agent_e2e.rs` | 8 | End-to-end shopping with real/mock sandbox workflows |

---

## 12. Security Model & Known Limitations

### Security Controls
- **Zero Secret Exposure**: Client secrets, OAuth tokens, and webhook signatures are never logged or exposed in HTTP responses.
- **Pre-Persistence Verification**: Webhooks are never persisted before PayPal cryptographic signature verification succeeds.
- **Payload Divergence Rejection**: Webhooks with duplicate event IDs but different raw payload bytes are rejected with `409 Conflict`.
- **Worker Fencing**: All state updates require active worker lease tokens. Stale workers fenced out by timeout cannot commit dispute mutations.
- **Genesis Immutability**: Database trigger `trg_protect_dispute_genesis_anchor` prevents updating genesis anchor hashes.
- **Strict Finalization Guard**: Disputes can only link to sessions with verified, sealed root hashes.
- **Conservative Correlation**: Disputes are never linked based on weak attributes (amount, customer name, email, or timestamps). Unmatched disputes are anchored to `GENESIS_HASH` for manual review.

### Known Limitations
- **Hash Chain Scope**: A valid cryptographic hash chain proves append-only immutability and sequence integrity of agent decisions and recorded payloads; it does not physical delivery of goods or customer satisfaction, which are governed by PayPal's dispute resolution framework.
- **Local Correlation vs. Fallback**: Local consumer correlation operates on Tiers 1–3 (Capture ID, Custom ID, Order ID). Tier 4 (calling PayPal Show Dispute Details API) is reserved for explicit audit reconciliation and is omitted from the consumer path to avoid external network latency.
- **Worker Supervisor**: Step 5 & 6 provide discrete, fenced consumer methods (`process_next_event`, `process_leased_event`). A continuous background polling loop / multi-threaded worker pool is left for background service orchestrators.

---

## 13. Current Implementation Status & Future Work

### Current Status: Phase 4 Milestone 4.4 Complete
- [x] Phase 1: PayPal OAuth 2.0 & Token Engine
- [x] Phase 2: Cryptographic Hash-Chain Ledger & SQLite Store
- [x] Phase 3: Deterministic Shopping Agent & PayPal Orders v2 Integration
- [x] Phase 4 Milestone 4.1: Webhook Signature Verification Client
- [x] Phase 4 Milestone 4.2: Durable Webhook Store & Idempotency
- [x] Phase 4 Milestone 4.3: 5-Tier Deterministic Dispute Correlation
- [x] Phase 4 Milestone 4.4: Production Verification, Fencing, Out-of-Order Hardening, HTTP Ingress, and E2E Testing

### Future Work
- **Background Worker Daemon**: Multi-worker background process with graceful shutdown and configurable polling intervals.
- **Merchant Web Dashboard**: UI for merchants to review flagged disputes (`MANUAL_REVIEW_REQUIRED`) and inspect dual-chain verification proofs.
- **Multi-Currency Support**: Dynamic foreign exchange rate anchoring for non-USD transactions.
