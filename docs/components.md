# Omniscient — Component & Service Catalog

> Scope: every service, tool, and contract you will **code** or **operate**, what it is responsible for, the technology, its interfaces, and exactly **who it talks to and how**. Companion to `architecture.md` (which covers end-to-end flows). Reflects locked decisions in `.devin/rules/omniscient.md`. MVP-first: fewest moving parts that are correct and fund-safe.

---

## 0. How to read this doc

Each component has a fixed template:

- **Kind** — service you write / tool you operate / contract you deploy.
- **Tech** — language + key libs.
- **Process** — what binary/container it runs as.
- **Owns** — the state it is the source of truth for.
- **Inbound / Outbound** — who calls it and who it calls, with transport.
- **Interfaces** — concrete endpoints / topics / functions.
- **Failure & scaling** — how it degrades, how it recovers.
- **Fund-safety** — the invariants it must never break.

**Transport legend**

```
 HTTP    request/response over HTTP (REST-ish JSON)
 WS      WebSocket (server push, live feed)
 KAFKA   Redpanda, Kafka wire protocol (produce/consume)
 RPC     Ethereum JSON-RPC (eth_*, to Polygon node)
 SQL     Postgres wire protocol
 IPC     in-process channel (same binary, bounded queue)
 SIGN    request to KMS/HSM for a signature
```

---

## 1. System map (all components)

```
                              UNTRUSTED                           TRUSTLESS (Polygon PoS)
   ┌──────────────────────┐                              ┌──────────────────────────────────┐
   │   Web UI / dApp       │                              │  Custody/Vault   (USDC, EIP-712)  │
   │   wallet · EIP-712    │                              │  Settlement Exchange (net deltas) │
   └───────┬──────────────┘                              │  Gnosis CTF      (ERC-1155)        │
           │ HTTP orders / WS feed                        │  Oracle          (commit-reveal)  │
           │                                              │  Pyth contract   (pull + bench)   │
   ════════╪═══════════════════════════════════════════  └───────▲──────────────▲────────────┘
           │                OPERATOR (Rust)                       │ RPC          │ RPC
           ▼                                                      │              │
   ┌───────────────────────────────┐                             │              │
   │  GATEWAY  (1 binary)           │                       ┌─────┴──────┐  ┌────┴───────────┐
   │  ┌─────────────┐  IPC  ┌─────┐ │                       │ SETTLEMENT │  │  RESOLUTION    │
   │  │ Order API+WS│──────▶│ ME  │ │                       │  Service   │  │  Service       │
   │  │   (axum)    │◀──────│s-wtr│ │                       └──▲──────┬──┘  └──▲──────┬──────┘
   │  └──────┬──────┘  KAFKA└──┬──┘ │                          │      │        │      │
   └─────────┼──────────────┬──┼────┘                         │KAFKA │KAFKA   │KAFKA │ HTTP
             │ SQL          │  │ KAFKA produce                 │consume│produce │      ▼
             ▼              │  ▼                               │      ▼        │  ┌─────────┐
   ┌──────────────┐        │ ┌─────────────────────────────────┴───────────┐  │  │  LLM    │
   │  Postgres     │◀──SQL──┼─┤            REDPANDA  (Kafka API)            │  │  │ provider│
   │ orders·holds  │        │ │  orders.matched · positions.updated ·       │  │  └─────────┘
   │ batches·audit │        │ │  settlement.batches · resolution.events     │  │
   └──────▲────────┘        │ └──────────────────────▲──────────────────────┘  │
          │ SQL             │ KAFKA consume           │ KAFKA produce           │
          │                 ▼                         │                         │
          │          ┌────────────┐                   │                         │
          └──────────│  INDEXER   │───────────────────┘                         │
            SQL write │  on/off    │  RPC poll/subscribe events  ───────────────┘
                      │  reconcile │◀──── Polygon RPC (logs, finality)
                      └─────┬──────┘
                            │ HTTP (finality / collateral updates)
                            ▼
                       Order API (Gateway)

   SIGNING:  Settlement & Resolution ── SIGN ──▶ KMS/HSM (operator keys)
   ADMIN:    privileged on-chain actions ──────▶ Safe multisig + timelock
```

**Two rules the map enforces:**

- The **Matching Engine never touches the chain or DB**. It only consumes commands (IPC) and produces matches (KAFKA).
- **Settlement and Resolution are the only writers to Polygon.** The **Indexer is the only reader back** from finalized chain state into the off-chain world.

---

## 2. Process topology (what actually runs)

MVP runs as **4 Rust processes + 2 stateful tools**, plus external providers.

```
 ┌──────────────────────────────────────────────────────────────┐
 │ DEPLOYMENT (single host / small cluster, MVP)                  │
 │                                                                │
 │  gateway        (Order API + WS + Matching Engine, 1 binary)   │
 │  settlement     (consumes matches, submits batches)            │
 │  resolution     (LLM proposer + commit/reveal)                 │
 │  indexer        (chain → off-chain reconciliation)             │
 │                                                                │
 │  redpanda       (single binary, no JVM/ZK)                     │
 │  postgres                                                      │
 └──────────────────────────────────────────────────────────────┘
   external:  Polygon RPC provider · LLM provider · Pyth Hermes · KMS/HSM · Safe
```

**MVP simplification:** the Order API and Matching Engine are **co-located in one binary** (`gateway`) and communicate over an in-process bounded channel — no network hop, no broker, on the hot path. They split into separate processes only if a single market's throughput ever outgrows one host (not an MVP concern).

---

## 3. Service catalog (off-chain, Rust)

### 3.1 Gateway — Order API + WebSocket

```
        clients                         gateway binary
   ┌──────────────┐  HTTP /orders  ┌──────────────────────────────┐
   │  dApp/wallet  │───────────────▶│ Order API (axum)              │
   │               │◀──── WS ───────│  • verify EIP-712 sig + nonce │  IPC   ┌──────────┐
   └──────────────┘  live feed     │  • check collateral hold      │───────▶│ Matching │
                                    │  • broadcast fills            │◀───────│ Engine   │
                                    └───────┬───────────────┬───────┘ (n/a)  └────┬─────┘
                                       SQL  │          KAFKA│consume          KAFKA│produce
                                            ▼               ▼ orders.matched       ▼
                                        Postgres        Redpanda            Redpanda
```

- **Kind:** service (you code it).
- **Tech:** Rust, `axum` (HTTP + native `WebSocketUpgrade`), `tokio`, `sqlx`, `alloy` (EIP-712 verify only), `rdkafka`.
- **Process:** part of `gateway` binary.
- **Owns:** open orders, collateral **holds/reservations**, nonces (in Postgres). Provisional, until finality.
- **Inbound:**
  - `dApp → HTTP`: submit/cancel signed orders, query book/positions.
  - `dApp ↔ WS`: subscribe to live fills/book deltas.
  - `Indexer → HTTP`: finality + collateral updates (internal, authenticated).
  - `Redpanda → KAFKA`: consumes `orders.matched` to broadcast provisional fills to WS.
- **Outbound:**
  - `→ IPC`: enqueue validated commands to Matching Engine (bounded queue).
  - `→ SQL`: read balances/holds, persist orders/nonces.
- **Interfaces (HTTP, port `8080`):**
  - `POST /orders` — submit EIP-712 signed order.
  - `DELETE /orders/{id}` — cancel (signed).
  - `GET /markets`, `GET /markets/{id}/book`, `GET /positions`.
  - `GET /ws` — WebSocket upgrade (live feed).
  - internal `POST /internal/reconcile` — indexer pushes finality/collateral (mTLS / shared secret).
- **Failure & scaling:** stateless aside from in-memory WS subscriptions; horizontally scalable behind a LB **except** it co-hosts the single-writer ME (see §3.2). Slow WS consumers dropped per a defined policy; never block matching.
- **Fund-safety:** accepts **only EIP-712-signed orders**; reserves collateral **on accept** before the order enters the book; rate-limits submission to resist spam/DoS.

---

### 3.2 Matching Engine (CLOB, single-writer per market)

```
   Order API ──IPC(bounded MPSC)──▶ ┌───────────────────────────┐ ──KAFKA──▶ orders.matched
   (cmd stream: new/cancel/amend)   │  single matcher thread     │           (key = market_id)
                                    │  per market, sync,         │
                                    │  alloc-free hot path,      │
                                    │  strict price-time         │
                                    │  priority + STP            │
                                    └───────────────────────────┘
```

- **Kind:** service / module (you code it). **The fund-safety-critical core.**
- **Tech:** Rust, **sync** (no `tokio` in the loop), `crossbeam` bounded queue for ingress only.
- **Process:** thread(s) inside `gateway`; **one writer thread per market** owning that book.
- **Owns:** the in-memory order book (provisional). Rebuildable from snapshot + replay-from-offset.
- **Inbound:** `IPC` command stream from Order API (serialized; the only way to mutate a book).
- **Outbound:** `KAFKA` produce `orders.matched` (provisional matches).
- **Interfaces:** internal command enum (`NewOrder`/`Cancel`/`Amend`), not a network API.
- **Matching semantics (must be deterministic + test-covered):** strict price-time priority, **self-trade prevention**, order types limit/IOC/FOK/post-only, partial-fill rules, cancel/amend through the **same** command stream.
- **Failure & scaling:** determinism > parallelism — **never** parallelize the match loop. Crash recovery via periodic book snapshot + Kafka replay. Per-market sharding is the only scaling axis.
- **Fund-safety:** never matches uncollateralized orders (API guarantees the hold first); **fee↔solvency** — maker rebate (0.1%) funded strictly by taker fee (0.5%), net fee ≥ 0 per match; rounding favors the pool.

---

### 3.3 Settlement Service

```
   orders.matched ──KAFKA consume──▶ ┌──────────────────────────┐
   (key=market_id)                   │ Settlement Service        │
                                      │  • net position deltas    │──RPC──▶ Settlement Exchange
                                      │    per batch (idempotent) │         (verifies each order sig)
                                      │  • EIP-1559 gas + nonce   │
                                      │  • wait for finality      │◀─SIGN─ KMS/HSM
                                      └───┬───────────────┬───────┘
                                     SQL  │          KAFKA│produce
                                          ▼               ▼
                                      Postgres        settlement.batches / positions.updated
```

- **Kind:** service (you code it).
- **Tech:** Rust, `tokio`, `alloy` (tx build/sign/submit), `rdkafka`, `sqlx`.
- **Process:** `settlement` binary.
- **Owns:** batch state machine (building → submitted → finalized), per-batch idempotency keys (Postgres).
- **Inbound:** `Redpanda → KAFKA` consume `orders.matched`.
- **Outbound:**
  - `→ RPC`: submit net-delta batch tx to Settlement Exchange contract.
  - `→ SIGN`: request tx signature from KMS/HSM (operator key, **not** user funds).
  - `→ SQL`: persist batch state.
  - `→ KAFKA`: produce `settlement.batches` (audit) and `positions.updated`.
- **Interfaces:** no public API; metrics endpoint only. Contract calls: `settleBatch(netDeltas, signedOrders[])`.
- **Failure & scaling:** EIP-1559 priority-fee bumping + replacement on stuck tx; **idempotent submission keyed per batch** so a retry cannot double-apply; waits ~2–5s Heimdall v2 finality before marking settled; bounded backlog → applies backpressure upstream rather than diverging.
- **Fund-safety:** the settlement→chain boundary has its **own** idempotency key + on-chain dedup — broker exactly-once is within-broker only and does **not** cross this boundary. Batch size tuned to **gas**, not a magic number.

---

### 3.4 Resolution Service

```
                 market expired
                      │
            ┌─────────┴──────────┐
   price market           real-world fact
      │ RPC                     │ HTTP
      ▼                          ▼
   Pyth (Benchmarks)        LLM provider ──▶ fetch+verify sources
   parsePriceFeedUpdates         │
      │                          ▼
      └────────▶ ┌──────────────────────────┐ ──RPC──▶ Oracle contract
                 │ Resolution Service        │         (commit hash → reveal)
                 │  • deterministic for price│◀─SIGN─ KMS/HSM
                 │  • LLM proposer for facts │
                 │  • commit-reveal + bond   │──KAFKA──▶ resolution.events
                 └──────────────────────────┘
```

- **Kind:** service (you code it).
- **Tech:** Rust, `tokio`, provider-agnostic LLM client (HTTP), `alloy`, `rdkafka`, HTTP source-fetcher.
- **Process:** `resolution` binary.
- **Owns:** proposal/commit/reveal state per market, evidence bundle (cited sources), audit trail.
- **Inbound:** triggered by market expiry (from indexer events / scheduler keyed off **on-chain block timestamp**).
- **Outbound:**
  - `→ HTTP`: LLM provider (proposer) + source URL fetch/verify.
  - `→ RPC`: Pyth Benchmarks read + on-chain `parsePriceFeedUpdates`; Oracle commit/reveal txs.
  - `→ SIGN`: tx signing via KMS/HSM.
  - `→ KAFKA`: produce `resolution.events` (versioned, legal/audit record).
- **Interfaces:** no public API. Contract calls: `commit(marketId, hash)`, `reveal(marketId, outcome, evidence)`.
- **Failure & scaling:** async 2–15s, **never blocks settlement**; low confidence / VOID → fallback to human/admin (multisig); on dispute in bonded window → escalate to human/DAO.
- **Fund-safety:** **AI is a proposer, not the oracle** — neutrality comes from economic dispute + on-chain commit-reveal. **Never finalize on raw model output**; source-fetch + claim verification mandatory. Pre-commitment binds concrete output + evidence (temp 0, no bit-identical assumption).

---

### 3.5 Chain Indexer

```
   Polygon RPC ──RPC(logs/subscribe)──▶ ┌──────────────┐ ──SQL──▶ Postgres
   (Deposit/BatchSettled/                │ Indexer      │
    OutcomeSet/Withdraw)                 │  • reconcile  │ ──HTTP──▶ Order API
                                         │  • finality   │  (credit balances, mark fills final)
                                         │  • reorg roll  │
                                         └──────────────┘
```

- **Kind:** service (you code it).
- **Tech:** Rust, `tokio`, `alloy` (log subscription / `eth_getLogs`), `sqlx`.
- **Process:** `indexer` binary.
- **Owns:** the **off-chain mirror of finalized chain state** (balances, positions, outcomes). The bridge that makes the chain the source of truth.
- **Inbound:** `Polygon RPC → RPC` (event logs, block/finality).
- **Outbound:**
  - `→ SQL`: write reconciled state.
  - `→ HTTP`: push finality + collateral updates to Order API (`/internal/reconcile`).
- **Interfaces:** consumes contract events `Deposit`, `BatchSettled`, `OutcomeSet`, `Withdraw`; metrics endpoint.
- **Failure & scaling:** **rolls back off-chain state on reorg before finality**; credits deposits / marks fills final **only after** deterministic finality; resumable from last processed block (checkpoint in Postgres).
- **Fund-safety:** the **only** path from finalized chain → off-chain; never treats pre-finality state as authoritative.

---

## 4. Tooling catalog (you operate, not code)

### 4.1 Redpanda (durable event log, Kafka API)

- **Why it exists:** **durability, replay, decoupling, audit/legal record** — explicitly **not** throughput buffering.
- **Tech:** single binary (no JVM/ZooKeeper), Kafka wire protocol. Code against the Kafka API so the broker stays swappable.
- **Talks to:** producers `gateway`(ME), `settlement`, `resolution`; consumers `settlement`, `gateway`(WS), `indexer`.
- **Port:** Kafka API `9092`, admin `9644`, schema registry `8081`.
- **Topics:** see `architecture.md §5`. Keys preserve per-market / per-user ordering; compaction on `positions.updated`.
- ⚠️ INVARIANT: exactly-once is **within-broker only** — does **not** extend across the settlement→chain boundary.

### 4.2 Postgres

- **Why:** durable relational state for orders, holds/reservations, nonces, batch state, indexer checkpoints, audit.
- **Talks to:** `gateway`, `settlement`, `indexer` (all `SQL`). Resolution writes audit too.
- **Port:** `5432`.
- **Note:** provisional/operational store — **never** the authority for funds (the chain is).

### 4.3 Polygon RPC provider

- **Why:** node access for reads (logs, balances, finality) and tx submission.
- **Talks to:** `settlement`, `resolution`, `indexer` (`RPC`).
- **Note:** untrusted — validate responses; redundant providers + retry/backoff; watch for reorgs before finality.

### 4.4 LLM provider (provider-agnostic)

- **Why:** the AI **proposer** for real-world-fact resolution.
- **Talks to:** `resolution` (`HTTP`).
- **Note:** untrusted output → must be source-verified before commit. Provider-agnostic interface so it's swappable.

### 4.5 Pyth (pull oracle + Benchmarks)

- **Why:** deterministic price-market resolution.
- **Talks to:** `resolution` fetches update data (Hermes, HTTP) and verifies on-chain via the Pyth contract (`parsePriceFeedUpdates`, pay `getUpdateFee`).
- **Note:** validate staleness (`getPriceNoOlderThan`), confidence, status; expiry markets use **Benchmarks at expiry**, never a live spot read.

### 4.6 KMS/HSM (signing keys)

- **Why:** custody of **operator** signing keys for settlement/resolution txs. **Never** user funds.
- **Talks to:** `settlement`, `resolution` (`SIGN`).

### 4.7 Safe multisig + timelock

- **Why:** authority for privileged on-chain actions (pause, upgrade, admin-override resolution).
- **Talks to:** humans → on-chain. Never an operator service auto-path.

---

## 5. On-chain contracts (you deploy — Solidity/Foundry)

| Contract | Responsibility | Called by | Reuse |
|---|---|---|---|
| **Custody / Vault** | Holds USDC; EIP-712 deposit/withdraw; forced-withdrawal escape hatch | dApp (deposit/withdraw), Settlement | bespoke, OZ primitives |
| **Settlement Exchange** | Applies **net position deltas in batches**; verifies each order's EIP-712 sig + nonce | Settlement Service | evaluate Polymarket `CTFExchange` pattern |
| **Gnosis CTF** | ERC-1155 outcome tokens: split/merge/redeem vs USDC collateral | Exchange, dApp (redeem) | **reuse Gnosis CTF as-is** |
| **Oracle** | commit-reveal outcome + bonded dispute window | Resolution Service, disputers | bespoke (optimistic oracle) |
| **Pyth contract** | On-chain price verification (`parsePriceFeedUpdates`) | Resolution Service | **reuse Pyth deployment** |

- **Units:** price [0,1] scaled to 1e6; complete CTF set = 1 USDC; winning share redeems for 1 USDC; USDC = 6 decimals. Document units at every boundary.
- **Access control:** OZ `AccessControl`, every privileged action behind **Safe multisig + timelock**; pausable circuit-breaker on settlement, withdrawals, resolution.
- **Guards:** reentrancy (CEI + `nonReentrant`), signature replay (EIP-712 + nonces), oracle staleness, rounding always favors the pool. Foundry unit + invariant/fuzz tests; Slither in CI.

---

## 6. Communication matrix (who → who)

| From | To | Transport | Payload / purpose | Sync? |
|---|---|---|---|---|
| dApp | Order API | HTTP | submit/cancel signed order, queries | req/resp |
| dApp | Order API | WS | subscribe live fills/book | push |
| dApp | Custody | RPC (wallet) | deposit / withdraw / redeem | tx |
| Order API | Matching Engine | IPC | validated command stream | bounded queue |
| Matching Engine | Redpanda | KAFKA | produce `orders.matched` | async |
| Redpanda | Settlement | KAFKA | consume `orders.matched` | async |
| Redpanda | Order API | KAFKA | consume `orders.matched` → WS | async |
| Settlement | Settlement Exchange | RPC | net-delta batch tx | tx + finality wait |
| Settlement | KMS/HSM | SIGN | sign operator tx | req/resp |
| Settlement | Redpanda | KAFKA | produce `settlement.batches`, `positions.updated` | async |
| Resolution | LLM provider | HTTP | proposal + source fetch | req/resp |
| Resolution | Pyth / Oracle | RPC | verify price, commit/reveal | tx |
| Resolution | Redpanda | KAFKA | produce `resolution.events` | async |
| Indexer | Polygon RPC | RPC | poll/subscribe events, finality | poll/sub |
| Indexer | Postgres | SQL | write reconciled state | write |
| Indexer | Order API | HTTP | finality / collateral updates | req/resp |
| {gateway,settlement,indexer} | Postgres | SQL | operational state | read/write |

---

## 7. Port & endpoint reference (MVP defaults, configurable)

```
 gateway      :8080   HTTP API + /ws        (public, behind LB/TLS)
 gateway      :7001   /metrics              (internal)
 settlement   :7002   /metrics              (internal)
 resolution   :7003   /metrics              (internal)
 indexer      :7004   /metrics              (internal)
 redpanda     :9092   Kafka API             (internal)
 redpanda     :9644   admin                 (internal)
 redpanda     :8081   schema registry       (internal)
 postgres     :5432   SQL                   (internal)
```

Only `gateway:8080` is public. Everything else is operator-internal.

---

## 8. Build order (what to code first)

A dependency-aware MVP sequence:

1. **Contracts** (Foundry): Custody + reuse CTF → Settlement Exchange → Oracle. Invariant/fuzz tests + Slither.
2. **Gateway**: Order API (EIP-712 verify, holds in Postgres) + Matching Engine (deterministic, replay-tested).
3. **Redpanda topics** + schema versioning.
4. **Settlement Service**: net-delta batching, idempotent submission, finality wait.
5. **Indexer**: event ingestion, reorg handling, reconcile → Order API.
6. **Resolution Service**: Pyth path first (deterministic), then LLM proposer + commit-reveal.
7. **Cross-cutting**: `tracing`, metrics + alert thresholds, circuit-breaker/pause wiring, KMS/HSM + Safe.

---

> Keep this catalog in sync with `architecture.md`. When a new service, topic, contract, or external dependency is added, add a row here **and** update the system map in §1.
