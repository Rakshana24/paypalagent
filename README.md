# AgenticCommerce: AI Shopping Agent & Verifiable Cryptographic Ledger

**AgentAudit** backend implementation for the PayPal AI Hackathon:
1. **Phase 1**: PayPal Sandbox OAuth 2.0 authentication and in-memory token lifecycle engine.
2. **Phase 2**: Cryptographic Hash-Chain Ledger, Embedded SQLite Store, and RFC 8785 (JCS) canonical serialization.
3. **Phase 3**: Deterministic Reference AI Shopping Agent, Explicit Purchase Authorization, and Real PayPal Orders v2 API integration.

---

## Architecture & Technology Stack

- **Language & Runtime**: Rust (stable toolchain)
- **Web Framework**: [Axum](https://github.com/tokio-rs/axum) (v0.7)
- **Async Runtime**: [Tokio](https://tokio.rs/) (v1.x, full features)
- **Embedded Database**: [SQLx](https://github.com/launchbadge/sqlx) (v0.9) with SQLite (`WAL` mode, foreign keys, 5s busy timeout)
- **Cryptography & Hashing**: [sha2](https://github.com/RustCrypto/hashes) (SHA-256), [hex](https://github.com/KokaKiwi/rust-hex)
- **Canonical Serialization**: [serde_jcs](https://crates.io/crates/serde_jcs) (strict RFC 8785 JSON Canonicalization Scheme via `ryu-js`), [serde](https://serde.rs/) & [serde_json](https://github.com/serde-rs/json)
- **HTTP Client**: [Reqwest](https://github.com/seanmonstar/reqwest) (v0.12, JSON & rustls-tls)
- **Identifiers & Timestamps**: [uuid](https://github.com/uuid-rs/uuid) (v4), [chrono](https://github.com/chronotope/chrono) (UTC RFC 3339)
- **Configuration & Logging**: [dotenvy](https://github.com/allan2/dotenvy), [tracing](https://github.com/tokio-rs/tracing) & [tracing-subscriber](https://github.com/tokio-rs/tracing)
- **Payment Gateway**: PayPal Orders API v2 (`/v2/checkout/orders`) with Sandbox buyer approval and capture workflow.

---

## Project Structure

```
d:\paypal\
├── data/                               # Embedded SQLite database (agentic_commerce.db)
├── src/
│   ├── config.rs                       # Centralized configuration with secret redaction
│   ├── db/
│   │   └── mod.rs                      # SQLite pool init, WAL mode, migrations (sessions, decision_steps)
│   ├── error.rs                        # Application-wide AppError and Axum IntoResponse mappings
│   ├── http_client.rs                  # Reusable async HTTP client with timeouts & User-Agent
│   ├── ledger/
│   │   ├── mod.rs                      # Ledger module exports
│   │   └── hash.rs                     # Length-prefixed SHA-256 binary encoder & RFC 8785 serializer
│   ├── lib.rs                          # AppState, Axum router, and REST endpoint controllers
│   ├── main.rs                         # Binary entry point and HTTP server initialization
│   ├── models/
│   │   ├── mod.rs                      # Data contract re-exports
│   │   ├── authorization.rs            # PurchaseAuthorization contracts and request/response DTOs
│   │   ├── catalog.rs                  # Product and ProductSpecification models
│   │   ├── config.rs                   # Configuration DTOs
│   │   ├── error.rs                    # Error response DTOs
│   │   ├── health.rs                   # GET /health response contract
│   │   ├── ledger.rs                   # Session, DecisionStep, and VerificationResult DTOs
│   │   ├── oauth.rs                    # PayPal OAuth and auth-check contracts
│   │   ├── paypal_orders.rs            # PayPal Orders v2 request, response, and capture DTOs
│   │   ├── shopping.rs                 # ShoppingRequirements, candidates, rejections, evaluations DTOs
│   │   └── webhook.rs                  # PayPal Webhook envelopes, headers, and signature verification DTOs
│   └── services/
│       ├── mod.rs                      # Service layer re-exports
│       ├── auth.rs                     # PayPal OAuth 2.0 service and in-memory token cache
│       ├── catalog.rs                  # Deterministic local mock headphone catalog service
│       ├── ledger.rs                   # LedgerService: sessions, append event, finalize, concurrency lock
│       ├── paypal_orders.rs            # Real PayPal Orders v2 service: create order & capture
│       ├── requirement_extractor.rs    # Deterministic natural language requirement extractor
│       ├── shopping_agent.rs           # ShoppingAgentService: Milestones 1-4 & authorization
│       ├── verification.rs             # VerificationService: cryptographic hash-chain integrity auditor
│       ├── webhook_persistence.rs     # WebhookPersistenceService: durable SQLite store, idempotency, backoff
│       └── webhook_verifier.rs         # WebhookVerifierService: PayPal signature verification client
├── tests/
│   ├── ledger_tests.rs                 # Database, hashing, integrity verification & concurrency tests
│   ├── live_sandbox_and_agent_e2e.rs   # End-to-end shopping & real PayPal Sandbox suite
│   ├── oauth_tests.rs                  # PayPal OAuth mock integration tests
│   ├── shopping_and_paypal_tests.rs    # Shopping agent, authorization, and PayPal Orders v2 tests
│   ├── webhook_persistence_tests.rs    # Milestone 4.2 webhook persistence, idempotency & backoff tests
│   └── webhook_tests.rs                # Milestone 4.1 signature verification client & security tests
├── .env                                # Local environment secrets (excluded from Git)
├── .env.example                        # Template environment configuration with placeholders
├── .gitignore                          # Excludes target/, .env, data/, *.db*
├── Cargo.toml                          # Package manifest and dependencies
└── README.md                           # Documentation and architectural guide
```

---

## Phase 3 Architecture: Shopping Agent & PayPal Orders

### 1. Deterministic Product Catalog
The mock catalog (`CatalogService`) seeds 9 realistic headphone products in USD:
- `hp-sony-wh1000xm5`: Sony WH-1000XM5 ($399.99, 30h battery, Hybrid Active ANC, Rating: 4.8)
- `hp-bose-qc45`: Bose QuietComfort 45 ($329.00, 24h battery, Acoustic ANC, Rating: 4.7)
- `hp-anker-q30`: Anker Soundcore Life Q30 ($79.99, 40h battery, Hybrid Active ANC, Rating: 4.5)
- `hp-anker-q20`: Anker Soundcore Life Q20 ($49.99, 30h battery, Active ANC, Rating: 4.4)
- `hp-jbl-tune760`: JBL Tune 760NC ($89.95, 35h battery, Active ANC, Rating: 4.3)
- `hp-senn-hd450`: Sennheiser HD 450BT ($129.95, 30h battery, Active ANC, Rating: 4.2)
- `hp-audio-technica-m20x`: Audio-Technica ATH-M20xBT ($79.00, 60h battery, Passive ANC, Rating: 4.4)
- `hp-sony-ch720n`: Sony WH-CH720N ($148.00, 35h battery, Dual Noise Sensor ANC, Rating: 4.5)
- `hp-skullcandy-hesh`: Skullcandy Hesh ANC ($99.99, 22h battery, Active ANC, Rating: 4.1)

### 2. Natural Language Requirement Extraction
[`RequirementExtractor`](file:///d:/paypal/src/services/requirement_extractor.rs) deterministically parses requests like:
> *"Buy noise-canceling headphones under $100 with at least 30 hours battery life."*
- **Budget**: Extracts `$100` (`maximum_budget = 100.0`). Rejects non-positive budgets or missing budgets.
- **Currency**: Validates USD. Rejects unsupported currencies (e.g. INR, EUR, GBP) with helpful instructions.
- **Battery**: Extracts `30` (`minimum_battery_hours = Some(30)`).
- **Features**: Detects `noise-canceling`, `wireless`, etc.
- **Brand**: Detects preferred brands (Sony, Bose, Anker, etc.).

### 3. Shopping Agent Workflow & Milestones
- **Milestone 1 (User Request)**: Creates session in SQLite ledger (Seq 0: `INITIAL_PROMPT`). Records extracted requirements as Seq 1 (`REQUIREMENTS_EXTRACTED`).
- **Milestone 2 (Catalog Exploration)**: Explores category and records candidates in Seq 2 (`CATALOG_SEARCH`).
- **Milestone 3 (Product Comparison)**: Evaluates candidates against mandatory constraints.
  - Rejects items exceeding budget with explicit reason (`PRODUCT_REJECTED`).
  - Rejects items lacking required battery life or ANC (`PRODUCT_REJECTED`).
  - Computes deterministic score for compliant candidates and records comparison matrix (`PRODUCT_COMPARISON`).
- **Milestone 4 (Final Selection)**: Selects highest-scoring product (e.g. `hp-anker-q30`) and records rationale in Seq 7 (`PRODUCT_SELECTED`).

### 4. Explicit Purchase Authorization (Trust Barrier)
Product selection does **NOT** grant payment permission. The agent requires explicit customer approval:
- Customer submits `POST /api/shopping/sessions/:session_id/authorize` with `selected_product_id`, `maximum_authorized_amount`, and `approved: true`.
- Verifies authorized amount $\ge$ product price.
- Commits `PURCHASE_AUTHORIZED` event into the hash chain.
- Orders cannot be created without prior authorization.

### 5. Real PayPal Orders v2 API & Payment Lifecycle
- **Order Creation (`POST /v2/checkout/orders`)**:
  - Uses `intent: "CAPTURE"`, `amount: { currency_code: "USD", value: "79.99" }`.
  - Attaches `custom_id: session_id` and idempotency header `PayPal-Request-Id`.
  - Stores `paypal_order_id` in `sessions` table.
  - Commits `PAYPAL_ORDER_CREATED` event with the buyer approval URL.
- **Sandbox Buyer Approval**:
  - Customer opens approval URL in browser and logs in with their PayPal Sandbox Personal account.
- **Payment Capture (`POST /v2/checkout/orders/{id}/capture`)**:
  - Requires buyer approval.
  - Verifies status is `COMPLETED`.
  - Commits `PAYPAL_PAYMENT_CAPTURED` event with transaction ID.
  - Finalizes the session: `sessions.root_hash` locks to the capture event's hash.

---

## Ledger Event Sequence

Every observable action is cryptographically committed using length-prefixed SHA-256 and RFC 8785 canonical JSON:

```
[Seq 0] INITIAL_PROMPT           -> Genesis block (prev: 64 zeros)
[Seq 1] REQUIREMENTS_EXTRACTED   -> Category, budget, battery, features
[Seq 2] CATALOG_SEARCH           -> Retrieved candidate products
[Seq 3] PRODUCT_REJECTED         -> Price > budget (e.g. Sony XM5)
[Seq 4] PRODUCT_REJECTED         -> Lacks ANC (e.g. Audio-Technica M20x)
[Seq 5] PRODUCT_REJECTED         -> Battery < 30h (e.g. Skullcandy Hesh)
[Seq 6] PRODUCT_COMPARISON       -> Scored candidates matrix
[Seq 7] PRODUCT_SELECTED         -> Chosen item (hp-anker-q30) & rationale
[Seq 8] PURCHASE_AUTHORIZED      -> Customer authorization with spending limit
[Seq 9] PAYPAL_ORDER_CREATED     -> PayPal Order ID & Sandbox approval URL
[Seq 10] PAYPAL_PAYMENT_CAPTURED -> Confirmed transaction & final session root hash
```

---

## API Reference & Examples

### 1. Execute Shopping Agent
```bash
curl -X POST http://localhost:5000/api/shopping/sessions \
  -H "Content-Type: application/json" \
  -d '{"prompt": "Buy noise-canceling headphones under $100 with at least 30 hours battery life."}'
```
**Response (`201 Created`)**:
```json
{
  "success": true,
  "session_id": "8f3b14e9-df72-46a2-921d-72013f9c6d48",
  "prompt": "Buy noise-canceling headphones under $100 with at least 30 hours battery life.",
  "requirements": {
    "category": "headphones",
    "maximum_budget": 100.0,
    "currency": "USD",
    "required_features": ["noise-canceling"],
    "minimum_battery_hours": 30,
    "preferred_brand": null
  },
  "candidates_found": 3,
  "rejected_candidates": [
    {
      "product_id": "hp-sony-wh1000xm5",
      "product_name": "Sony WH-1000XM5 Wireless Noise-Canceling Headphones",
      "price": 399.99,
      "currency": "USD",
      "rejection_reason": "Price $399.99 exceeds customer budget limit of $100.00"
    },
    {
      "product_id": "hp-audio-technica-m20x",
      "product_name": "Audio-Technica ATH-M20xBT Wireless Over-Ear Headphones",
      "price": 79.0,
      "currency": "USD",
      "rejection_reason": "Product lacks active noise cancellation (ANC)"
    },
    {
      "product_id": "hp-skullcandy-hesh",
      "product_name": "Skullcandy Hesh ANC Wireless Noise-Canceling Headphones",
      "price": 99.99,
      "currency": "USD",
      "rejection_reason": "Battery life (22 hours) is below required minimum (30 hours)"
    }
  ],
  "selected_product": {
    "product_id": "hp-anker-q30",
    "name": "Anker Soundcore Life Q30 Hybrid Active Noise Cancelling Headphones",
    "brand": "Anker",
    "category": "headphones",
    "price": 79.99,
    "currency": "USD",
    "rating": 4.5,
    "available_stock": 120,
    "description": "Advanced noise cancellation technology with 40-hour battery life and custom EQ via app.",
    "specifications": {
      "battery_life_hours": 40,
      "noise_cancelling": true,
      "anc_type": "Hybrid Active",
      "wireless": true,
      "bluetooth_version": "5.0",
      "weight_grams": 260,
      "microphone": true,
      "water_resistant": false
    },
    "features": ["noise-canceling", "wireless", "bluetooth", "transparency-mode", "app-control"]
  },
  "selection_rationale": "Selected 'Anker Soundcore Life Q30 Hybrid Active Noise Cancelling Headphones' (Score: 154.50): Meets all mandatory constraints with 4.5-star rating, 40h battery, and price of $79.99 (within $100.00 budget).",
  "current_sequence_index": 7,
  "latest_hash": "2195f00e3cf2c9cebb8c68aa238ea5f7d2fbf0..."
}
```

### 2. Authorize Purchase
```bash
curl -X POST http://localhost:5000/api/shopping/sessions/8f3b14e9-df72-46a2-921d-72013f9c6d48/authorize \
  -H "Content-Type: application/json" \
  -d '{
    "selected_product_id": "hp-anker-q30",
    "maximum_authorized_amount": 85.00,
    "currency": "USD",
    "approved": true
  }'
```
**Response (`200 OK`)**:
```json
{
  "success": true,
  "session_id": "8f3b14e9-df72-46a2-921d-72013f9c6d48",
  "authorization": {
    "session_id": "8f3b14e9-df72-46a2-921d-72013f9c6d48",
    "selected_product_id": "hp-anker-q30",
    "maximum_authorized_amount": 85.0,
    "currency": "USD",
    "authorization_status": "APPROVED",
    "authorization_timestamp": "2026-11-07T12:00:00Z",
    "authorization_method": "CUSTOMER_EXPLICIT_APPROVAL"
  },
  "sequence_index": 8,
  "current_hash": "c530ae8ffb22105..."
}
```

### 3. Create PayPal Sandbox Order
```bash
curl -X POST http://localhost:5000/api/shopping/sessions/8f3b14e9-df72-46a2-921d-72013f9c6d48/orders
```
**Response (`201 Created`)**:
```json
{
  "success": true,
  "session_id": "8f3b14e9-df72-46a2-921d-72013f9c6d48",
  "order_id": "5C7829402M839211K",
  "status": "CREATED",
  "approval_url": "https://www.sandbox.paypal.com/checkoutnow?token=5C7829402M839211K"
}
```

### 4. Buyer Approval & Order Capture
1. Open the `approval_url` in a browser.
2. Sign in with PayPal Sandbox Personal account and click **Complete Purchase**.
3. Call the capture endpoint:
```bash
curl -X POST http://localhost:5000/api/shopping/sessions/8f3b14e9-df72-46a2-921d-72013f9c6d48/capture
```
**Response (`200 OK`)**:
```json
{
  "success": true,
  "session_id": "8f3b14e9-df72-46a2-921d-72013f9c6d48",
  "order_id": "5C7829402M839211K",
  "capture_id": "7TY284919P901234L",
  "status": "COMPLETED",
  "final_root_hash": "9e81b672a912803..."
}
```

### 5. Inspect Full Session Details & Cryptographic Audit
```bash
curl http://localhost:5000/api/shopping/sessions/8f3b14e9-df72-46a2-921d-72013f9c6d48
```
**Response (`200 OK`)**:
```json
{
  "session_id": "8f3b14e9-df72-46a2-921d-72013f9c6d48",
  "initial_prompt": "Buy noise-canceling headphones under $100 with at least 30 hours battery life.",
  "requirements": {
    "category": "headphones",
    "maximum_budget": 100.0,
    "currency": "USD",
    "required_features": ["noise-canceling"],
    "minimum_battery_hours": 30,
    "preferred_brand": null
  },
  "selected_product": { "product_id": "hp-anker-q30", "price": 79.99, ... },
  "authorization": { "authorization_status": "APPROVED", "maximum_authorized_amount": 85.0, ... },
  "paypal_order_id": "5C7829402M839211K",
  "is_finalized": true,
  "root_hash": "9e81b672a912803...",
  "event_count": 11,
  "verification": {
    "session_id": "8f3b14e9-df72-46a2-921d-72013f9c6d48",
    "status": "VALID",
    "total_steps": 11,
    "verified_steps": 11,
    "calculated_root_hash": "9e81b672a912803...",
    "stored_root_hash": "9e81b672a912803...",
    "first_invalid_sequence_index": null,
    "error_code": null,
    "explanation": "Cryptographic hash chain is verified and continuous"
  }
}
```

---

## Automated Testing & Verification

Run the entire verification suite:
```bash
# 1. Format check
cargo fmt --check

# 2. Type & borrow check
cargo check

# 3. Full test suite (72 automated tests)
cargo test

# 4. Optimized release build
cargo build --release
```

Test coverage includes:
- **Phase 1 (10 tests)**: PayPal OAuth authentication, in-memory cache, safety-margin auto-refresh, stampede prevention.
- **Phase 2 (24 tests)**: SQLite schema migrations, foreign keys, unique sequence constraints, persistence across reconnects, length-prefixed hashing, RFC 8785 UTF-16 surrogate sorting, 8 isolated tamper vectors.
- **Phase 3 (16 tests)**:
  - Catalog listing, lookups, and multi-field keyword search.
  - Requirement parsing, budget extraction, currency validation.
  - Shopping agent canonical selection, constraint rejection reasons.
  - Unmatched prompt handling preserving audit session.
  - Explicit purchase authorization, spending limits, duplicate rejection.
  - Real PayPal Orders API v2 creation, custom_id matching, Bearer auth.
  - Buyer approval validation and capture execution.
  - Failed/pending capture handling without false finalization.
  - End-to-end REST lifecycle and finalized root hash alignment.
- **Unit & Documentation Tests (22 tests)**: Serde contracts, health check, secret masking.
