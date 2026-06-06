# Omniscient — System Architecture

> Scope: how the pieces fit and how money/state flows end-to-end. Reflects the locked decisions in `.devin/rules/omniscient.md` (Polygon PoS, off-chain Rust CLOB, Redpanda log, Pyth + AI optimistic-oracle resolution). MVP-first: fewest moving parts that are correct and fund-safe.

---

## 1. Component Overview

Numbered edges trace the **happy-path lifecycle** (deposit → trade → settle → resolve → redeem). Color = trust zone.

```mermaid
flowchart TB
    subgraph Client["CLIENT (untrusted)"]
        UI["Web UI / dApp<br/>wallet · EIP-712 signing"]
    end

    subgraph OffChain["OFF-CHAIN SERVICES (Rust, operator)"]
        direction TB
        API["Order API + WS<br/>axum"]
        ME["Matching Engine<br/>single-writer / market"]
        SETT["Settlement Service<br/>alloy · EIP-1559"]
        RES["Resolution Service<br/>provider-agnostic LLM"]
        IDX["Chain Indexer<br/>on/off-chain reconcile"]
        PG[("Postgres<br/>orders · holds · audit")]
    end

    subgraph Broker["REDPANDA — durable event log (Kafka API)"]
        direction LR
        T1(["orders.matched"])
        T3(["settlement.batches"])
        T4(["resolution.events"])
    end

    subgraph Chain["POLYGON PoS — on-chain (trustless)"]
        direction TB
        CUST["Custody / Vault<br/>USDC · EIP-712 verify"]
        EXCH["Settlement Contract<br/>net deltas · batched"]
        CTF["Gnosis CTF<br/>ERC-1155 outcomes"]
        ORACLE["Oracle<br/>commit-reveal · dispute"]
        PYTH["Pyth pull oracle<br/>+ Benchmarks"]
    end

    UI -->|"1 · deposit USDC"| CUST
    UI -->|"2 · signed order (HTTP)"| API
    API <-->|"live feed (WS)"| UI
    API -->|"3 · enqueue cmd"| ME
    ME -->|"4 · publish match"| T1
    T1 -->|"5a · consume"| SETT
    T1 -->|"5b · broadcast"| API
    SETT -->|"6 · net batch tx"| EXCH
    SETT -.->|audit| T3
    EXCH --> CUST
    EXCH --> CTF
    RES -->|"7 · commit/reveal"| ORACLE
    RES -.->|audit| T4
    ORACLE -->|price markets| PYTH
    ORACLE -->|"8 · set outcome"| CTF
    UI -->|"9 · redeem shares"| CTF
    Chain ==>|events| IDX
    IDX -->|"collateral & finality"| API
    IDX --> PG
    API --> PG
    SETT --> PG

    classDef client fill:#e3f2fd,stroke:#1565c0,color:#0d47a1;
    classDef svc fill:#ede7f6,stroke:#5e35b1,color:#311b92;
    classDef broker fill:#fff3e0,stroke:#ef6c00,color:#e65100;
    classDef chain fill:#e8f5e9,stroke:#2e7d32,color:#1b5e20;
    classDef store fill:#fafafa,stroke:#616161,color:#212121;
    class UI client;
    class API,ME,SETT,RES,IDX svc;
    class PG store;
    class T1,T3,T4 broker;
    class CUST,EXCH,CTF,ORACLE,PYTH chain;
```

**Source of truth:** the chain (finalized state) is authoritative for funds, positions, and resolution. Off-chain state is provisional until finality and is reconciled by the indexer.

---

## 1a. Service Interactions (who talks to whom)

What each off-chain service exchanges, the transport, and the direction. Solid = command/data push, dashed = event consume, dotted = read.

```mermaid
flowchart LR
    UI["dApp"]

    subgraph SVC["Off-chain services"]
        direction TB
        API["Order API + WS"]
        ME["Matching Engine"]
        SETT["Settlement"]
        RES["Resolution"]
        IDX["Indexer"]
    end

    PG[("Postgres")]
    RP{{"Redpanda"}}
    RPC["Polygon RPC"]
    AIP["LLM provider"]

    UI <-->|"HTTP orders · WS feed"| API
    API -->|"bounded queue (in-proc)"| ME
    API -.->|"read balances/holds"| PG
    ME ==>|"produce orders.matched"| RP
    RP -. "consume orders.matched" .-> SETT
    RP -. "consume orders.matched" .-> API
    SETT -->|"signed batch tx"| RPC
    SETT ==>|"produce settlement.batches"| RP
    SETT -->|"write batch state"| PG
    RES -->|"prompt + verify source"| AIP
    RES -->|"commit/reveal tx"| RPC
    RES ==>|"produce resolution.events"| RP
    IDX -.->|"poll/subscribe events"| RPC
    IDX -->|"reconcile & write"| PG
    IDX -->|"finality / collateral updates"| API

    classDef a fill:#ede7f6,stroke:#5e35b1,color:#311b92;
    classDef b fill:#fafafa,stroke:#616161,color:#212121;
    class API,ME,SETT,RES,IDX a;
    class PG,RP,RPC,AIP,UI b;
```

**Read this as:** the Matching Engine never talks to the chain; the Settlement and Resolution services are the **only** writers to Polygon; the Indexer is the **only** path back from finalized chain state into off-chain services. Redpanda decouples produce/consume so a consumer outage never stalls matching.

---

## 2. End-to-End Trade Flow

```mermaid
sequenceDiagram
    actor U as User (wallet)
    participant API as Order API (axum)
    participant ME as Matching Engine
    participant RP as Redpanda
    participant ST as Settlement Svc
    participant CH as Polygon (Custody/CTF/Exch)
    participant IX as Indexer

    Note over U,CH: 1. Deposit
    U->>CH: deposit USDC to Custody
    CH-->>IX: Deposit event
    IX->>API: credit balance (after finality)

    Note over U,ME: 2. Order (off-chain match)
    U->>API: EIP-712 signed order
    API->>API: verify sig + nonce, check collateral hold
    API->>ME: enqueue command (bounded queue)
    ME->>ME: price-time match (sync, alloc-free)
    ME->>RP: orders.matched (provisional)
    RP-->>API: push to WS (provisional)

    Note over ST,CH: 3. Net batch settlement
    RP->>ST: consume orders.matched
    ST->>ST: net position deltas per batch (idempotent key)
    ST->>CH: submit batch tx (verifies each order sig)
    CH-->>IX: BatchSettled event
    IX->>API: mark fills final (after ~2-5s finality)

    Note over U,CH: 4. Resolve + redeem (see §4)
    U->>CH: redeem winning CTF shares -> USDC
```

⚠️ **INVARIANT:** the WS feed shows **provisional** matches. Clients must render fills as provisional until the indexer confirms finalized settlement. Broker exactly-once is within-broker only — the settlement→chain boundary has its own idempotency key + on-chain dedup.

---

## 3. Market Lifecycle (state machine)

```mermaid
stateDiagram-v2
    [*] --> open: create (collateral bond)
    open --> halted: circuit-breaker / pause
    halted --> open: resume
    open --> expired: block.timestamp >= expiry
    expired --> proposed: AI / Pyth proposes outcome
    proposed --> disputed: bonded challenge in window
    proposed --> resolved: window passes, no dispute
    disputed --> resolved: human / DAO escalation
    resolved --> settled: final batch applied
    settled --> redeemable: winners redeem CTF -> USDC
    redeemable --> [*]
```

Illegal transitions revert on-chain. Off-chain services treat state as authoritative only from finalized chain state. Expiry and dispute windows key off **on-chain block timestamp**, not the scheduler clock.

---

## 4. Resolution — Optimistic Oracle

Two paths depending on market type.

```mermaid
flowchart TB
    EXP["Market expired"] --> TYPE{Market type}

    TYPE -->|"Price (X > $P by Y)"| PYTH["Fetch Pyth Benchmark<br/>at expiry Y"]
    PYTH --> VERIFY["parsePriceFeedUpdates<br/>(verify on-chain, pay updateFee)"]
    VERIFY --> AUTO["Deterministic outcome"]
    AUTO --> FINAL

    TYPE -->|"Real-world fact"| AI["LLM proposer<br/>(provider-agnostic)"]
    AI --> SRC["Fetch source URLs<br/>+ verify claims"]
    SRC --> CONF{Confidence >= threshold?}
    CONF -->|no / VOID| HUMAN["Fallback: human / admin (multisig)"]
    CONF -->|yes| COMMIT["Commit intent hash on-chain"]
    COMMIT --> REVEAL["Reveal outcome + evidence"]
    REVEAL --> WIN{Dispute in bonded window?}
    WIN -->|no| FINAL["Finalize outcome"]
    WIN -->|yes| ESC["Escalate -> human / DAO"]
    ESC --> FINAL
    HUMAN --> FINAL
    FINAL["Outcome final -> CTF redeemable"]

    classDef auto fill:#e8f5e9,stroke:#2e7d32,color:#1b5e20;
    classDef ai fill:#ede7f6,stroke:#5e35b1,color:#311b92;
    classDef human fill:#fff3e0,stroke:#ef6c00,color:#e65100;
    classDef done fill:#e3f2fd,stroke:#1565c0,color:#0d47a1;
    class PYTH,VERIFY,AUTO auto;
    class AI,SRC,COMMIT,REVEAL ai;
    class HUMAN,ESC human;
    class FINAL done;
```

**Key properties:**

- **AI is a proposer, not the oracle.** Economic dispute + on-chain commit-reveal provide neutrality, not the model itself.
- **Never finalize on raw model output** — source fetch + claim verification is mandatory.
- **Pre-commitment binds** the concrete output + cited evidence; verification (not text regeneration) is the reproducible step. Temperature 0, but no assumption of bit-identical re-runs.
- Resolution latency (2–15s async) **never blocks settlement**.

---

## 5. Event / Topic Design (Redpanda)

| Topic | Producer | Consumers | Key | Notes |
|---|---|---|---|---|
| `orders.matched` | Matching Engine | Settlement, WS Broadcaster | `market_id` | Preserves per-market ordering; isolated consumer groups |
| `positions.updated` | Settlement | Indexer, API | `user_id` | Compacted |
| `settlement.batches` | Settlement | Indexer, audit | `batch_id` | Idempotency key per batch |
| `resolution.events` | Resolution | Indexer, audit | `market_id` | Versioned schema, legal/audit record |

Broker justified by **durability, replay, decoupling, audit** — not throughput. All payloads carry an explicit schema version; commit offsets after processing; DLQ for poison messages; bounded exponential backoff on retries.

---

## 6. Source-of-Truth & Reconciliation

| State | Provisional source | Authoritative source |
|---|---|---|
| Balances / collateral | API holds (Postgres) | Custody contract (via indexer, post-finality) |
| Fills / positions | `orders.matched` (WS) | Settlement contract / CTF (post-finality) |
| Market outcome | AI proposal / Pyth read | Oracle contract (post dispute window) |
| Order book | In-memory (Matching Engine) | Rebuilt from snapshot + replay-from-offset |

**Finality & reorg:** settlement is final only after Polygon deterministic finality (~2–5s, Heimdall v2). The indexer rolls back off-chain state on reorg before finality.

---

## 7. Fund-Safety Invariants (cross-cutting)

- **Solvency / conservation:** collateral pool ≥ total owed to winners at all times; matching, fees, settlement, and rounding never create or destroy value; rounding always favors the pool.
- **Non-custodial:** trades settle only from EIP-712-signed user orders; the operator cannot fabricate trades. Forced-withdrawal / escape hatch path preserved in v1 custody design.
- **Pre-trade collateralization:** orders enter the book only after available balance is verified against indexed on-chain custody; collateral is reserved on accept.
- **Fee↔solvency coupling:** maker rebate (0.1%) funded strictly by taker fee (0.5%) — net protocol fee ≥ 0 per match.
- **Settlement idempotency:** per-batch key + on-chain dedup so a retry cannot double-apply.
- **Backpressure:** bounded settlement backlog; slow/halt matching rather than let pre-settlement state diverge unbounded.
- **Crash recovery:** in-memory book rebuildable via periodic snapshot + replay-from-offset.

---

## 8. Trust Boundaries

```mermaid
flowchart LR
    subgraph Untrusted
        U["Users / dApp"]
        AIP["AI provider API"]
        RPC["RPC provider"]
    end
    subgraph Operator["Operator (semi-trusted, constrained by code)"]
        SVC["Rust services"]
        KEYS["Signing keys (KMS/HSM)"]
    end
    subgraph Trustless["Trustless (on-chain)"]
        CONTRACTS["Custody / CTF / Exchange / Oracle"]
    end

    U -->|signed orders| SVC
    AIP -->|untrusted output -> verified| SVC
    RPC -->|reads/writes| SVC
    SVC -->|signed txs| CONTRACTS
    KEYS --> SVC
    CONTRACTS -->|enforces non-custodial rules| U

    classDef untrusted fill:#ffebee,stroke:#c62828,color:#b71c1c;
    classDef operator fill:#fff3e0,stroke:#ef6c00,color:#e65100;
    classDef trustless fill:#e8f5e9,stroke:#2e7d32,color:#1b5e20;
    class U,AIP,RPC untrusted;
    class SVC,KEYS operator;
    class CONTRACTS trustless;
```

- User input, AI output, and RPC responses are **untrusted** — validate/verify everything.
- Operator services are constrained by on-chain rules: they **cannot** move funds without user signatures.
- Privileged on-chain actions sit behind **multisig (Safe) + timelock**; pausable circuit-breaker on settlement, withdrawals, resolution.

⚠️ **COMPLIANCE:** prediction markets carry heavy regulatory exposure (US CFTC + state gambling law). Geofencing and legal review are first-class requirements — chain choice provides no regulatory cover.
