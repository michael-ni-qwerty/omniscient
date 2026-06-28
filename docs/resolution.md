# Resolution Service

Async worker that monitors expired markets, dispatches them to an AI resolver, and stores proposals in Postgres. On-chain submission via `Oracle.proposeOutcome` is a deferred seam — the service currently produces `pending` proposals in the DB; the on-chain lifecycle (`PROPOSED → DISPUTED → RESOLVED`) is reconciled by the Indexer from Oracle events.

**Source:** `backend/resolution/src/main.rs`
**Dependencies:** Postgres, Polygon RPC (alloy provider, unused for tx), LLM API (reqwest)

## Architecture

```mermaid
flowchart TD
    subgraph RS["RESOLUTION SERVICE"]
        EW["Expiry Watcher<br/>interval(10s)"]
        AI["AiResolver<br/>reqwest → LLM API"]
        DB["DB Writer<br/>resolution_proposals"]
        EW --> AI
        AI --> DB
    end

    PG[(Postgres<br/>markets, resolution_proposals)] -->|"SELECT expired markets"| EW
    LLM["LLM Provider<br/>(OpenAI-compatible)"] -->|"HTTP response"| AI
    AI -->|"verify source URLs"| SRC["External URLs"]

    classDef bin fill:#ede7f6,stroke:#5e35b1,color:#311b92,stroke-width:2px;
    classDef ext fill:#e3f2fd,stroke:#1565c0,color:#0d47a1;
    classDef store fill:#fafafa,stroke:#616161,color:#212121;
    class RS bin;
    class LLM,SRC ext;
    class PG store;
```

## Resolver Trait

```rust
#[async_trait]
trait Resolver: Send + Sync {
    fn id(&self) -> &'static str;
    async fn validate_spec(&self, spec: &ResolutionSpec) -> Result<(), SpecError>;
    async fn resolve(&self, market: &ExpiredMarket) -> Result<ResolutionProposal, ResolveError>;
}
```

Only `AiResolver` is implemented. The trait is provider-agnostic — additional resolvers (trusted API, Pyth) plug in behind the same interface.

## AiResolver

- **API:** OpenAI-compatible chat completions endpoint (`LLM_API_URL` + `LLM_API_KEY`)
- **Model:** `gpt-4o`, `temperature: 0`
- **Timeout:** 30s
- **Confidence threshold:** < 70 → `ResolveError::Fallback` (human/DAO)
- **Source verification:** fetches each evidence URL; non-200 or fetch failure → `Fallback`
- **Payouts:** binary `[100 - outcome, outcome]` (percentages, not basis points)

## Expiry Watcher Loop

```mermaid
sequenceDiagram
    participant EW as Expiry Watcher
    participant PG as Postgres
    participant AI as AiResolver
    participant LLM as LLM Provider
    EW->>PG: SELECT market_id, resolver_id FROM markets WHERE state='open' AND expiry <= now()
    loop each expired market
        EW->>PG: UPDATE markets SET state='expired'
        EW->>AI: resolve(market)
        alt resolver_id == "ai"
            AI->>LLM: POST /v1/chat/completions
            LLM-->>AI: {choices[0].message.content}
            AI->>AI: parse JSON {outcome, confidence, evidence}
            AI->>AI: confidence >= 70? fetch + verify evidence URLs
            alt success
                AI-->>EW: ResolutionProposal
            else fallback
                AI-->>EW: ResolveError::Fallback
            end
        else non-ai resolver
            EW->>EW: ResolveError::Fallback (no other resolvers wired)
        end
        EW->>PG: INSERT/UPDATE resolution_proposals (status=pending)
    end
```

## ResolutionProposal

```rust
struct ResolutionProposal {
    outcome: u8,
    payouts: Vec<u64>,
    evidence: Vec<EvidenceItem>,  // { url, verified }
    confidence: u8,
}
```

## Off-Chain Integrity Hash

`compute_commitment(market_id, payouts)` = `keccak256(market_id.0 ++ payouts.map(U256::be_bytes))`. Stored in `resolution_proposals.commitment` for audit. This is **not** an on-chain commit-reveal commitment — the Oracle uses plaintext optimistic resolution.

## Postgres Schema

```sql
CREATE TABLE resolution_proposals (
    market_id BYTEA NOT NULL REFERENCES markets(market_id),
    round INT NOT NULL DEFAULT 0,
    resolver_id TEXT NOT NULL,
    commitment BYTEA,                     -- 32 bytes, off-chain integrity hash
    proposed_payouts NUMERIC(78,0)[],
    confidence INT CHECK (confidence >= 0 AND confidence <= 100),
    evidence JSONB,
    status TEXT NOT NULL,                 -- pending | committed | revealed | disputed | resolved
    proposer_address BYTEA,
    disputer_address BYTEA,
    dispute_reason TEXT,
    committed_at TIMESTAMPTZ,
    revealed_at TIMESTAMPTZ,
    finalized_at TIMESTAMPTZ,
    PRIMARY KEY (market_id, round)
);
```

## Markets Table

```sql
CREATE TABLE markets (
    market_id BYTEA PRIMARY KEY,
    condition_id BYTEA,
    question_id BYTEA NOT NULL,
    resolver_id TEXT NOT NULL,
    class TEXT NOT NULL DEFAULT 'ai' CHECK (class = 'ai'),
    state TEXT NOT NULL,                  -- open | halted | expired | proposed | disputed | resolved | settled | redeemable
    expiry TIMESTAMPTZ,
    outcome_slot_count INT NOT NULL DEFAULT 2,
    block_number BIGINT NOT NULL DEFAULT 0,
    ...
);
```

## On-Chain Submission (Deferred Seam)

The service does not yet submit `proposeOutcome` transactions to the Oracle contract. The documented flow:

1. AI produces `pending` proposal in DB (current)
2. Operator or automated service submits `Oracle.proposeOutcome(marketId, payouts)` — deferred
3. Indexer reconciles on-chain `OutcomeProposed` / `OutcomeDisputed` / `OutcomeResolved` events into `resolution_proposals` status

The `commitment` field in the DB is an off-chain audit artifact, not an on-chain commitment.
