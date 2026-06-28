# Gateway

Public-facing entry point for user orders and WebSocket streaming. Single binary combining axum HTTP/WS server with a dedicated matcher thread.

**Source:** `backend/gateway/src/main.rs`
**Port:** `:8080` (only public service)
**Dependencies:** Postgres, Redpanda, crossbeam channel

## Architecture

```mermaid
flowchart TD
    subgraph Gateway["GATEWAY BINARY"]
        API["Order API (axum)<br/>POST /orders · POST /internal/reconcile<br/>GET /health · GET /ws"]
        ME["Matcher Thread<br/>crossbeam::bounded(10_000)"]
        WS["WS Broadcast Loop<br/>rdkafka StreamConsumer → broadcast::channel"]
        API == "crossbeam Sender" ===> ME
    end

    Client["dApp"] -->|"HTTP POST /orders"| API
    API -->|"SQL (holds)"| PG[(Postgres)]
    API -->|"try_send cmd"| ME
    ME -->|"Kafka produce"| RP([Redpanda<br/>orders.matched])
    RP -->|"Kafka consume"| WS
    WS -->|"broadcast::Sender"| Client

    classDef bin fill:#ede7f6,stroke:#5e35b1,color:#311b92,stroke-width:2px;
    classDef ext fill:#e3f2fd,stroke:#1565c0,color:#0d47a1;
    classDef store fill:#fafafa,stroke:#616161,color:#212121;
    classDef broker fill:#fff3e0,stroke:#ef6c00,color:#e65100;
    class Gateway bin;
    class Client ext;
    class PG store;
    class RP broker;
```

## Endpoints

| Method | Path | Auth | Description |
|---|---|---|---|
| POST | `/orders` | — | submit signed order; validates signature is 65-byte hex + has `maker` field; inserts hold placeholder; enqueues to matcher |
| POST | `/internal/reconcile` | `x-internal-secret` header | called by Indexer to credit deposits post-finality; upserts `users` + `balances` |
| GET | `/health` | — | checks Postgres `SELECT 1` + Kafka metadata fetch; returns `{status, db, kafka}` |
| GET | `/ws` | — | WebSocket upgrade; subscribes to `broadcast::Receiver<Value>` fed by Kafka consumer |

## Matcher Thread

Spawned via `std::thread::Builder` (not tokio) — sync thread receiving `MatchCommand` from a bounded `crossbeam::channel::bounded(10_000)`. Currently logs received commands. The matching engine logic (price-time priority, self-trade prevention, allocation-free hot path) is the documented design target.

## WS Broadcast Loop

Async tokio task consuming from Redpanda topic `orders.matched` (consumer group `gateway-ws-group`). Each message is parsed as JSON and sent via `broadcast::channel(1024)`. Commits offsets asynchronously after processing.

## Order Submission Flow

```mermaid
sequenceDiagram
    actor U as User (wallet)
    participant API as Order API (axum)
    participant PG as Postgres
    participant ME as Matcher Thread
    participant RP as Redpanda
    U->>API: POST /orders (signed order JSON)
    API->>API: validate signature (65 bytes hex) + maker address
    API->>PG: INSERT hold (placeholder, amount=0)
    API->>ME: try_send MatchCommand::PlaceOrder
    ME->>ME: log command (matcher stub)
    API-->>U: 202 Accepted
```

## Reconcile Flow

```mermaid
sequenceDiagram
    participant IX as Indexer
    participant API as Order API
    participant PG as Postgres
    IX->>API: POST /internal/reconcile {user, deposit, block_number}
    API->>API: verify x-internal-secret header
    API->>PG: INSERT users ON CONFLICT DO NOTHING
    API->>PG: UPSERT balances (available_amount += deposit)
    API-->>IX: 200 OK
```

## Shared Domain Types

The `shared` crate (`backend/shared/src/domain.rs`) provides:

- **`Address`** — `[u8; 20]` newtype with hex serde
- **`MarketId`** — `[u8; 32]` newtype with hex serde
- **`Bytes32`** — `[u8; 32]` newtype with hex serde
- **`Price`** — `u64` newtype, `SCALE = 1_000_000`
- **`Quantity`** — `u64` newtype
- **`OrderId`** — `Uuid` newtype
- **`BatchId`** — `[u8; 32]` newtype
- **`OrderSide`** — enum `Buy = 0`, `Sell = 1`
- **`Order`** — mirrors `SettlementExchange.Order` (includes `signer`, `condition_id`, `parent_collection_id` for off-chain EIP-712 hashing)
- **`SignedOrder`** — `Order` + `Vec<u8>` signature
- **EIP-712 helpers:** `compute_domain_separator`, `hash_order`, `eip712_signing_hash`, `verify_order_signature`

## Configuration

Loaded from environment via `shared::config::AppConfig`:

- `GATEWAY_BIND` — listen address (default `0.0.0.0:8080`)
- `GATEWAY_INTERNAL_SECRET` — shared secret for `/internal/reconcile`
- `DATABASE_URL`, `KAFKA_BROKERS`, `RPC_URL`, `CHAIN_ID`
- Contract addresses: `CUSTODY_ADDR`, `SETTLEMENT_EXCHANGE_ADDR`, `ORACLE_ADDR`, `CTF_ADDR`, `USDC_ADDR`
- `OPERATOR_KEY` — hex private key (dev only; KMS in production)
- `LLM_API_URL`, `LLM_API_KEY` — for Resolution Service
- `LOG_FORMAT` — `pretty` or `json`
