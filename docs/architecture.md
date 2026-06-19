# Omniscient — System Architecture

This document defines the architectural components, their exact internal execution structures, individual boundary responsibilities, and the global communication flows of the Omniscient prediction market system.

---

## 1. Gateway (Order API & Matching Engine)

The Gateway acts as the public-facing entry point for user orders and WebSocket streaming feeds. It manages client authentication, in-memory state, and balance reservations.

```mermaid
flowchart TD
    subgraph Gateway["GATEWAY BINARY"]
        API["Order API<br/>- EIP-712 verify<br/>- Balance checks"]
        ME["Matching Engine<br/>- Price-Time FIFO<br/>- Self-Trade Prev"]
        API == "Internal IPC<br/>(MPSC Queue)" ===> ME
    end

    Client["HTTP POST /orders<br/>(Signed Order)"] --> API
    API -->|"SQL<br/>(Reserve holds)"| PG[(Postgres)]
    ME -->|"Kafka Produce<br/>(Provisional fills)"| RP([Redpanda])

    classDef bin fill:#ede7f6,stroke:#5e35b1,color:#311b92,stroke-width:2px;
    classDef ext fill:#e3f2fd,stroke:#1565c0,color:#0d47a1;
    classDef store fill:#fafafa,stroke:#616161,color:#212121;
    classDef broker fill:#fff3e0,stroke:#ef6c00,color:#e65100;

    class Gateway bin;
    class Client ext;
    class PG store;
    class RP broker;
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

```mermaid
flowchart TD
    RP([Redpanda]) -->|"Kafka Consume<br/>(orders.matched)"| BM

    subgraph SS["SETTLEMENT SERVICE"]
        BM["Batch Manager<br/>- Net positions<br/>- Idempotency key"]
        TE["Transaction Engine<br/>- EIP-1559 gas bump<br/>- Sign via KMS/HSM"]
        BM == "IPC" ===> TE
    end

    TE -->|"JSON-RPC (settleBatch)"| SE["Settlement Exchange<br/>(Polygon)"]

    classDef bin fill:#ede7f6,stroke:#5e35b1,color:#311b92,stroke-width:2px;
    classDef broker fill:#fff3e0,stroke:#ef6c00,color:#e65100;
    classDef chain fill:#e8f5e9,stroke:#2e7d32,color:#1b5e20;

    class SS bin;
    class RP broker;
    class SE chain;
```

### Key Responsibilities
- **Delta Aggregation:** Consumes individual matched execution logs from Redpanda and nets position deltas per-user across a configurable batch size.
- **Idempotency Control:** Computes a unique batch hash and maps it to a database state tracking layer, guaranteeing exactly-once application at the chain boundary.
- **KMS Transaction Signing:** Dispatches transaction payloads to an HSM or cloud-managed KMS provider to isolate operator keys.
- **Priority-Fee Bumping:** Implements dynamic gas tracking using EIP-1559 fee structures, auto-replacing stuck transactions via incremental nonce-equivalent replacements.

---

## 3. Resolution Service

The Resolution Service operates as an asynchronous worker that manages market finalization through the resolver registry. **MVP is AI-only:** every market binds to the AI optimistic-oracle resolver. Deterministic on-chain sources (Pyth auto-finalize) and trusted-API sources are deferred post-MVP behind the same `Resolver` trait.

```mermaid
flowchart TD
    Ext["External Sources<br/>(LLM / API)<br/>Pyth deferred MVP"] -->|"HTTP Query / JSON Fetch"| RR

    subgraph RS["RESOLUTION SERVICE"]
        RR["Resolver Registry<br/>- AI (MVP)<br/>- API (deferred)<br/>- Pyth (deferred)"]
        CRL["Commit-Reveal Loop<br/>- Generate hash<br/>- Manage dispute"]
        RR == "IPC (deferred)" ===> CRL
    end

    CRL -->|"JSON-RPC (commit / reveal)"| Oracle["Oracle Contract<br/>(Polygon)"]

    classDef bin fill:#ede7f6,stroke:#5e35b1,color:#311b92,stroke-width:2px;
    classDef ext fill:#e3f2fd,stroke:#1565c0,color:#0d47a1;
    classDef chain fill:#e8f5e9,stroke:#2e7d32,color:#1b5e20;

    class RS bin;
    class Ext ext;
    class Oracle chain;
```

### Key Responsibilities
- **Resolver Registry Dispatch:** Routes expired markets to the **AI optimistic resolver** (MVP) inside a unified `Resolver` abstraction. Deterministic on-chain (Pyth) and trusted-API resolvers are deferred post-MVP behind the same trait.
- **On-Chain Proof Sourcing (post-MVP):** Would fetch historical Pyth Benchmark payloads on-demand at exact expiry intervals to pass to the Oracle's on-chain verifiers.
- **AI Claim Synthesis:** Directs LLM prompts, parses structured proposals, and verifies cited source URLs to generate cryptographic proof bundles.
- **Commit-Reveal Execution:** Generates commit hashes, schedules revealed payloads, and monitors active challenge/dispute windows.

---

## 4. Chain Indexer

The Chain Indexer is the unidirectional pipeline for synchronizing the finalized on-chain state back into the off-chain Postgres and Gateway execution spaces.

```mermaid
flowchart TD
    RPC["Polygon RPC Node"] -->|"WebSocket / JSON-RPC Poll<br/>(Logs & Blocks)"| BP

    subgraph CI["CHAIN INDEXER"]
        BP["Block Parser<br/>- Parse log topics"]
        FE["Finality Evaluator<br/>- Wait 2-5s (Heim)"]
        RM["Reorg Monitor<br/>- Rollback state"]
        BP == "IPC" ===> FE
        FE == "IPC" ===> RM
    end

    FE -->|"HTTP POST<br/>(/internal/reconcile)"| GW["Gateway"]
    RM -->|"SQL"| PG[(Postgres)]

    classDef bin fill:#ede7f6,stroke:#5e35b1,color:#311b92,stroke-width:2px;
    classDef ext fill:#e3f2fd,stroke:#1565c0,color:#0d47a1;
    classDef store fill:#fafafa,stroke:#616161,color:#212121;
    classDef chain fill:#e8f5e9,stroke:#2e7d32,color:#1b5e20;

    class CI bin;
    class GW bin;
    class RPC chain;
    class PG store;
```

### Key Responsibilities
- **Log Parsing:** Subscribes to Polygon EVM execution logs, processing specific contract events (`Deposit`, `BatchSettled`, `OracleResolved`).
- **Finality Tracking:** Enforces a minimum blocks-to-finality delay (~2–5s, Heimdall v2) before propagating transaction updates.
- **Reorg Safe Synchronization:** Tracks chain depth and automatically rolls back unfinalized Postgres records in the event of a network chain reorg.
- **Credit Reconciliation:** Delivers balance-refresh instructions via private HTTP endpoints to notify the Gateway that deposits are safe to trade or batches have settled on-chain.

---

## 5. Smart Contracts (Polygon PoS)

The smart contracts are the ultimate source of truth, enforcing non-custodial ownership and cryptographic checks for user balances.

```mermaid
flowchart TD
    Svc["Settlement Svc /<br/>Resolution Svc"] -->|"JSON-RPC Writes<br/>(signed orders & proofs)"| Contracts

    subgraph Contracts["POLYGON CONTRACTS"]
        direction TB
        SE["SettlementExchange<br/>- EIP-712 Verify<br/>- Apply net deltas"]
        Oracle["Oracle Contract<br/>- Commit-Reveal logic<br/>- Bonded disputes"]
        CV["Custody Vault<br/>- Holds USDC pool<br/>- Escape hatch"]
        CTF["Gnosis CTF (ERC1155)<br/>- Outcome positions<br/>- Redeems winning sets"]

        SE -->|"calls"| CV
        Oracle -->|"sets outcome"| CTF
    end

    Contracts -->|"EVM Event Logs<br/>(Deposits, Settlement, Resolves)"| CI["Chain Indexer"]

    classDef bin fill:#ede7f6,stroke:#5e35b1,color:#311b92,stroke-width:2px;
    classDef chain fill:#e8f5e9,stroke:#2e7d32,color:#1b5e20,stroke-width:2px;
    classDef contractsGroup fill:#eafaf1,stroke:#2e7d32,color:#1b5e20,stroke-width:1px;

    class Svc bin;
    class CI bin;
    class SE,Oracle,CV,CTF chain;
    class Contracts contractsGroup;
```

### Key Responsibilities
- **USDC Custody & Escape Hatch:** Vaults deposited ERC-20 collateral. Governs a time-delayed forced escape hatch when paused, allowing user-driven direct redemptions.
- **On-Chain Order Verification:** Re-evaluates order signatures and increments nonces directly on-chain within `SettlementExchange` to secure funds from operator manipulation.
- **Outcome Tokenization:** Leverages the Gnosis Conditional Tokens Framework (CTF) to split, merge, and redeem binary ERC-1155 outcome share sets against collateral reserves.
- **Economic Dispute Routing:** Holds dispute bonds and controls the state machine governing dispute timelines and human/DAO escalations.

---

## 6. Global Integrated Topology

The integrated global diagram displays the multi-zone flow of data, state, and cryptographically signed payloads.

```mermaid
flowchart TB
    subgraph Client["CLIENT (Untrusted)"]
        UI["Web UI / dApp"]
    end

    subgraph Operator["OPERATOR (Off-chain Rust Stack)"]
        direction TB
        GW["Gateway (Order API)"]
        ME["Matching Engine (Book)"]
        PG[("Postgres DB")]
        RP([Redpanda<br/>'orders.matched'])
        Settlement["Settlement Service"]
        Resolution["Resolution Service"]
        Indexer["Chain Indexer"]
        External["Pyth/LLM Providers"]
    end

    subgraph Chain["ON-CHAIN (Polygon PoS trustless execution)"]
        direction TB
        RPC["RPC Node & Logs"]
        subgraph Contracts["ON-CHAIN SOLIDITY CONTRACTS"]
            Vault["Custody Vault"]
            SE["SettlementExchange"]
            CTF["Gnosis CTF tokens"]
            Oracle["Oracle Contract"]
        end
    end

    %% Client Interactions
    UI -->|"HTTP POST /orders<br/>(Signed EIP-712 Order)"| GW
    UI -->|"deposit USDC"| Vault
    GW == "WS Fills / Orderbook Feed" ===> UI

    %% Operator Internal Flows
    GW -->|"IPC (Enqueues Cmd)"| ME
    GW -->|"SQL"| PG
    ME -->|"Produce"| RP
    RP -->|"Consume"| Settlement
    RP -->|"Consume"| GW
    PG <-->|"SQL"| Settlement
    PG <-->|"SQL"| Indexer
    Settlement -->|"SQL"| PG

    %% Resolution internal
    Resolution -->|"HTTP Resolves"| External
    External --> Resolution

    %% Operator to Chain Writes
    Settlement -->|"EIP-1559 Tx (net deltas)"| SE
    Resolution -->|"Commit / Reveal Tx"| Oracle

    %% Indexer / RPC Reads
    RPC -->|"WebSocket / JSON-RPC reads"| Indexer
    Indexer -->|"HTTP Balance Credits"| GW

    %% Contracts Interactions & Dependencies
    SE -->|"calls"| Vault
    Vault <-->|"balance transfers"| CTF
    SE -->|"calls"| CTF
    Oracle -->|"sets outcome"| CTF

    %% Transaction Verification Logic from Diagrams
    GW -.->|"Re-verifies Order Signatures"| SE
    UI -.->|"Commit-Reveal proposal + Dispute"| Oracle

    %% RPC Connection
    Contracts -->|"EVM Event Logs"| RPC

    %% Styling
    classDef client fill:#e3f2fd,stroke:#1565c0,color:#0d47a1;
    classDef svc fill:#ede7f6,stroke:#5e35b1,color:#311b92,stroke-width:2px;
    classDef broker fill:#fff3e0,stroke:#ef6c00,color:#e65100;
    classDef chain fill:#e8f5e9,stroke:#2e7d32,color:#1b5e20;
    classDef store fill:#fafafa,stroke:#616161,color:#212121;

    class UI client;
    class GW,ME,Settlement,Resolution,Indexer svc;
    class PG store;
    class RP broker;
    class Vault,SE,CTF,Oracle,RPC,Contracts chain;
```
