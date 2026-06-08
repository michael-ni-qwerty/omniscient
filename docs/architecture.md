# Omniscient — System Architecture

This document defines the architectural components, their exact internal execution structures, individual boundary responsibilities, and the global communication flows of the Omniscient prediction market system.

---

## 1. Gateway (Order API & Matching Engine)

The Gateway acts as the public-facing entry point for user orders and WebSocket streaming feeds. It manages client authentication, in-memory state, and balance reservations.

```text
               +-------------------------------------------------------------+
               |                       GATEWAY BINARY                        |
               |                                                             |
               |   +-------------------+             +-------------------+   |
  HTTP POST    |   |     Order API     |  Internal   |  Matching Engine  |   |
  /orders ---->|-->| - EIP-712 verify  |== IPC =====>| - Price-Time FIFO |   |
 (Signed Order)|   | - Balance checks  | (MPSC Queue)| - Self-Trade Prev |   |
               |   +---------+---------+             +---------+---------+   |
               |             |                                 |             |
               +-------------|---------------------------------|-------------+
                             | SQL                             | Kafka Produce
                             v (Reserve holds)                 v (Provisional fills)
                       +-----+----+                      +-----+----+
                       | Postgres |                      | Redpanda |
                       +----------+                      +----------+
```

### Key Responsibilities
- **Authentication & Validation:** Decodes and verifies EIP-712 order signatures against user addresses and checks sequential nonces.
- **Collateral Hold Reservation:** Checks availability of funds against indexed balances and locks required collateral in Postgres before the order is placed on the book to guarantee pre-trade collateralization.
- **Order Queueing:** Enqueues validated commands to an in-process bounded queue to prevent matching loop memory exhaustion.
- **Matching Execution:** Drives an allocation-free, single-writer matching loop enforcing strict price-time priority and self-trade prevention.
- **WebSocket Broadcast:** Consumes matching records from Redpanda to feed live order books and execution streams to clients.

---

## 2. Settlement Service

The Settlement Service is the operator-controlled worker responsible for bridging off-chain matches to the trustless ledger by preparing and dispatching batched transactions.

```text
       +----------+
       | Redpanda |
       +----+-----+
            |
            | Kafka Consume (orders.matched)
            v
+-------------------------------------------------------+
|                  SETTLEMENT SERVICE                   |
|                                                       |
|  +-------------------+        +--------------------+  |
|  |   Batch Manager   |        | Transaction Engine |  |
|  | - Net positions   |=======>| - EIP-1559 gas bump|  |
|  | - Idempotency key |  IPC   | - Sign via KMS/HSM |  |
|  +-------------------+        +---------+----------+  |
|                                         |             |
+-----------------------------------------|-------------+
                                          | JSON-RPC (settleBatch)
                                          v
                               +----------+----------+
                               | Settlement Exchange |
                               |     (Polygon)       |
                               +---------------------+
```

### Key Responsibilities
- **Delta Aggregation:** Consumes individual matched execution logs from Redpanda and nets position deltas per-user across a configurable batch size.
- **Idempotency Control:** Computes a unique batch hash and maps it to a database state tracking layer, guaranteeing exactly-once application at the chain boundary.
- **KMS Transaction Signing:** Dispatches transaction payloads to an HSM or cloud-managed KMS provider to isolate operator keys.
- **Priority-Fee Bumping:** Implements dynamic gas tracking using EIP-1559 fee structures, auto-replacing stuck transactions via incremental nonce-equivalent replacements.

---

## 3. Resolution Service

The Resolution Service operates as an asynchronous worker that manages market finalization through the resolver registry, implementing AI optimistic-oracle proposals (and deferred post-MVP: Pyth on-chain auto-finalize).

```text
      +--------------------+
      |  External Sources  |
      | (LLM / API)        |
      | Pyth deferred MVP  |
      +---------+----------+
                |
                | HTTP Query / JSON Fetch
                v
+-------------------------------------------------------+
|                  RESOLUTION SERVICE                   |
|                                                       |
|  +-------------------+        +--------------------+  |
|  | Resolver Registry |        | Commit-Reveal Loop |  |
|  | - Class B (API)    |=======>| - Generate hash    |  |
|  | - Class C (AI)     |  IPC   | - Manage dispute   |  |
|  | - Class A (Pyth)   |deferred|                    |  |
|  +-------------------+        +---------+----------+  |
|                                         |             |
+-----------------------------------------|-------------+
                                          | JSON-RPC (commit / reveal)
                                          v
                               +----------+----------+
                               |   Oracle Contract   |
                               |      (Polygon)      |
                               +---------------------+
```

### Key Responsibilities
- **Resolver Registry Dispatch:** Dynamically routes expired markets to specialized handlers (Class B/C optimistic off-chain) mapped inside a unified `Resolver` abstraction. Class A (Pyth) is deferred post-MVP.
- **On-Chain Proof Sourcing (post-MVP):** Would fetch historical Pyth Benchmark payloads on-demand at exact expiry intervals to pass to the Oracle's on-chain verifiers.
- **AI Claim Synthesis:** Directs LLM prompts, parses structured proposals, and verifies cited source URLs to generate cryptographic proof bundles.
- **Commit-Reveal Execution:** Generates commit hashes, schedules revealed payloads, and monitors active challenge/dispute windows.

---

## 4. Chain Indexer

The Chain Indexer is the unidirectional pipeline for synchronizing the finalized on-chain state back into the off-chain Postgres and Gateway execution spaces.

```text
                               +---------------------+
                               |  Polygon RPC Node   |
                               +----------+----------+
                                          |
                                          | WebSocket / JSON-RPC Poll (Logs & Blocks)
                                          v
+------------------------------------------------------------------------------------+
|                                   CHAIN INDEXER                                    |
|                                                                                    |
|  +--------------------+        +---------------------+        +-----------------+  |
|  |    Block Parser    |        | Finality Evaluator  |        | Reorg Monitor   |  |
|  | - Parse log topics |=======>| - Wait 2-5s (Heim)  |=======>| - Rollback state|  |
|  +--------------------+  IPC   +----------+----------+  IPC   +--------+--------+  |
|                                           |                            |           |
+-------------------------------------------|----------------------------|-----------+
                                            | HTTP POST                  | SQL
                                            v (/internal/reconcile)      v
                                      +-----+----+                 +-----+----+
                                      | Gateway  |                 | Postgres |
                                      +----------+                 +----------+
```

### Key Responsibilities
- **Log Parsing:** Subscribes to Polygon EVM execution logs, processing specific contract events (`Deposit`, `BatchSettled`, `OracleResolved`).
- **Finality Tracking:** Enforces a minimum blocks-to-finality delay (~2–5s, Heimdall v2) before propagating transaction updates.
- **Reorg Safe Synchronization:** Tracks chain depth and automatically rolls back unfinalized Postgres records in the event of a network chain reorg.
- **Credit Reconciliation:** Delivers balance-refresh instructions via private HTTP endpoints to notify the Gateway that deposits are safe to trade or batches have settled on-chain.

---

## 5. Smart Contracts (Polygon PoS)

The smart contracts are the ultimate source of truth, enforcing non-custodial ownership and cryptographic checks for user balances.

```text
                             +-------------------+
                             | Settlement Svc /  |
                             |  Resolution Svc   |
                             +---------+---------+
                                       |
                                       | JSON-RPC Writes (signed orders & proofs)
                                       v
+-----------------------------------------------------------------------------+
|                              POLYGON CONTRACTS                              |
|                                                                             |
|      +--------------------+                  +-----------------------+      |
|      | SettlementExchange |                  |    Oracle Contract    |      |
|      | - EIP-712 Verify   |                  | - Commit-Reveal logic |      |
|      | - Apply net deltas |                  | - Bonded disputes     |      |
|      +---------+----------+                  +-----------+-----------+      |
|                |                                         |                  |
|                | calls                                   | sets outcome     |
|                v                                         v                  |
|      +---------+----------+                  +-----------+-----------+      |
|      |   Custody Vault    |                  |  Gnosis CTF (ERC1155) |      |
|      | - Holds USDC pool  |                  | - Outcome positions   |      |
|      | - Escape hatch     |                  | - Redeems winning sets|      |
|      +--------------------+                  +-----------------------+      |
|                                                                             |
+-----------------------------------------------------------------------------+
                                       |
                                       | EVM Event Logs (Deposits, Settlement, Resolves)
                                       v
                               +-------+-------+
                               | Chain Indexer |
                               +---------------+
```

### Key Responsibilities
- **USDC Custody & Escape Hatch:** Vaults deposited ERC-20 collateral. Governs a time-delayed forced escape hatch when paused, allowing user-driven direct redemptions.
- **On-Chain Order Verification:** Re-evaluates order signatures and increments nonces directly on-chain within `SettlementExchange` to secure funds from operator manipulation.
- **Outcome Tokenization:** Leverages the Gnosis Conditional Tokens Framework (CTF) to split, merge, and redeem binary ERC-1155 outcome share sets against collateral reserves.
- **Economic Dispute Routing:** Holds dispute bonds and controls the state machine governing dispute timelines and human/DAO escalations.

---

## 6. Global Integrated Topology

The integrated global diagram displays the multi-zone flow of data, state, and cryptographically signed payloads.

```text
+---------------------------------------------------------------------------------------------------+
| CLIENT (Untrusted)                                                                                |
|                                                                                                   |
|                     HTTP POST /orders (Signed EIP-712 Order)                                      |
|            +--------------------------------------------------------+                             |
|            |                                                        |                             |
|            v                                                        |                             |
|      +-----+----+             WS Fills / Orderbook Feed             |                             |
|      |  Web UI  |<==========================================+       |                             |
|      +-----+----+                                           |       |                             |
|            |                                                |       |                             |
|            | deposit USDC                                   |       |                             |
|            |                                                |       |                             |
+------------|------------------------------------------------|-------|-----------------------------+
             |                                                |       |
             |                                                |       |
+------------v------------------------------------------------|-------|-----------------------------+
| OPERATOR (Off-chain Rust Stack)                             |       |                             |
|                                                             |       |                             |
|      +----------+              HTTP Balance Credits         |       |                             |
|      | Gateway  |<---------------------------------------+  |       |                             |
|      |          |                                        |  |       |                             |
|      |  Order   |-- IPC (Enqueues Cmd) --+               |  |       |                             |
|      |   API    |                        v               |  |       |                             |
|      +----+-----+                +-------+--------+      |  |       |                             |
|           |                      |    Matching    |      |  |       |                             |
|           | SQL                  |  Engine (Book) |      |  |       |                             |
|           v                      +-------+--------+      |  |       |                             |
|      +----+-----+                        |               |  |       |                             |
|      | Postgres |                        | Produce       |  |       |                             |
|      +----+-----+                        v               |  |       |                             |
|           ^                      +-------+--------+      |  |       |                             |
|           |                      |    Redpanda    |------+  |       |                             |
|           | SQL                  | `orders.matched`         |       |                             |
|           |                      +-------+--------+         |       |                             |
|           |                              |                  |       |                             |
|           |                              | Consume          |       |                             |
|           |                              v                  |       |                             |
|           |                      +-------+--------+         |       |                             |
|           |                      |   Settlement   |         |       |                             |
|           |                      |    Service     |         |       |                             |
|           |                      +-------+--------+         |       |                             |
|           |                              |                  |       |                             |
|           |                              | EIP-1559 Tx      |       |                             |
|           |                              v (net deltas)     |       |                             |
|           |                                                 |       |                             |
|           |                      +-------+--------+         |       |                             |
|           |                      |   Resolution   |         |       |                             |
|           |                      |    Service     |         |       |                             |
|           |                      +----+--+--------+         |       |                             |
|           |                           |  |                  |       |                             |
|           |            HTTP Resolves  |  | Commit / Reveal  |       |                             |
|           |        +------------------+  | Tx               |       |                             |
|           |        v                     v                  |       |                             |
|           |  +-----+----+                                   |       |                             |
|           |  | Pyth/LLM |                                   |       |                             |
|           |  +----------+                                   |       |                             |
|           |                                                 |       |                             |
|           |                      +-------+--------+         |       |                             |
|           +----------------------|  Chain Indexer |<-----+  |       |                             |
|                                  +----------------+      |  |       |                             |
|                                                          |  |       |                             |
+----------------------------------------------------------|--|-------|-----------------------------+
                                                           |  |       |
                                                           |  |       |
+----------------------------------------------------------|--|-------|-----------------------------+
| ON-CHAIN (Polygon PoS trustless execution)               |  |       |                             |
|                                                          |  |       |                             |
|                                    JSON-RPC reads        |  |       |                             |
|                     +------------------------------------+  |       |                             |
|                     |                                       |       |                             |
|                     v                                       |       |                             |
|            +--------+-------+                               |       |                             |
|            |  RPC Node/Logs |<-----+                        |       |                             |
|            +----------------+      |                        |       |                             |
|                                    |                        |       |                             |
|      +-----------------------------+--------------------+   |       |                             |
|      |             ON-CHAIN SOLIDITY CONTRACTS          |   |       |                             |
|      |                                                  |   |       |                             |
|      |     +------------------+   +------------------+  |   |       v                             |
|      |     |  Custody Vault   |   |SettlementExchange|<-|---|-------+                             |
|      |     +------------------+   +------------------+  |   | (Re-verifies Order Signatures)      |
|      |              ^                      ^            |   |                                     |
|      |              | balance transfers    | calls      |   |                                     |
|      |              v                      |            |   |                                     |
|      |     +------------------+            |            |   |                                     |
|      |     | Gnosis CTF tokens|<-----------+            |   |                                     |
|      |     +------------------+                         |   |                                     |
|      |              ^                                   |   |                                     |
|      |              | sets outcome                      |   |                                     |
|      |     +--------+---------+                         |   |                                     |
|      |     |  Oracle Contract |<------------------------+---|-------------------------------------+
|      |     +------------------+                             | (Commit-Reveal proposal + Dispute)  |
|      |                                                  |                                         |
|      +--------------------------------------------------+                                         |
|                                                                                                   |
+---------------------------------------------------------------------------------------------------+
```
