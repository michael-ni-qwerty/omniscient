-- Extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- Users (EOA addresses)
CREATE TABLE users (
    address BYTEA PRIMARY KEY CHECK (octet_length(address) = 20),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Markets
CREATE TABLE markets (
    market_id BYTEA PRIMARY KEY CHECK (octet_length(market_id) = 32),
    condition_id BYTEA CHECK (octet_length(condition_id) = 32),
    question_id BYTEA NOT NULL CHECK (octet_length(question_id) = 32),
    resolver_id TEXT NOT NULL,
    class TEXT NOT NULL DEFAULT 'ai' CHECK (class = 'ai'),
    state TEXT NOT NULL CHECK (state IN ('open','halted','expired','proposed','disputed','resolved','settled','redeemable')),
    expiry TIMESTAMPTZ,
    outcome_slot_count INT NOT NULL DEFAULT 2 CHECK (outcome_slot_count >= 2),
    block_number BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Balances (indexed from chain, post-finality)
CREATE TABLE balances (
    user_address BYTEA NOT NULL REFERENCES users(address),
    asset_type TEXT NOT NULL CHECK (asset_type IN ('usdc','ctf')),
    position_id NUMERIC(78,0),
    available_amount NUMERIC(78,0) NOT NULL DEFAULT 0,
    hold_amount NUMERIC(78,0) NOT NULL DEFAULT 0,
    finalized_block_number BIGINT NOT NULL DEFAULT 0,
    finalized_block_hash BYTEA CHECK (octet_length(finalized_block_hash) = 32),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_address, asset_type, COALESCE(position_id, -1))
);

-- Orders (off-chain book state)
CREATE TABLE orders (
    order_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_address BYTEA NOT NULL REFERENCES users(address),
    market_id BYTEA NOT NULL REFERENCES markets(market_id),
    side TEXT NOT NULL CHECK (side IN ('buy','sell')),
    price NUMERIC(78,0) NOT NULL CHECK (price > 0 AND price <= 1000000),
    amount NUMERIC(78,0) NOT NULL CHECK (amount > 0),
    filled_amount NUMERIC(78,0) NOT NULL DEFAULT 0,
    status TEXT NOT NULL CHECK (status IN ('open','filled','cancelled','expired')),
    nonce NUMERIC(78,0) NOT NULL,
    signature BYTEA NOT NULL,
    deadline TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Collateral holds (pre-trade reservation)
CREATE TABLE holds (
    hold_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_address BYTEA NOT NULL REFERENCES users(address),
    order_id UUID NOT NULL REFERENCES orders(order_id) ON DELETE RESTRICT,
    asset_type TEXT NOT NULL CHECK (asset_type IN ('usdc','ctf')),
    position_id NUMERIC(78,0),
    amount NUMERIC(78,0) NOT NULL CHECK (amount > 0),
    status TEXT NOT NULL CHECK (status IN ('held','released','applied')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    released_at TIMESTAMPTZ
);

-- Settlement batches
CREATE TABLE settlement_batches (
    batch_id BYTEA PRIMARY KEY CHECK (octet_length(batch_id) = 32),
    status TEXT NOT NULL CHECK (status IN ('pending','submitted','finalized','failed')),
    tx_hash BYTEA CHECK (octet_length(tx_hash) = 32),
    nonce BIGINT,
    gas_fee_cap NUMERIC(78,0),
    gas_tip_cap NUMERIC(78,0),
    block_number BIGINT NOT NULL DEFAULT 0,
    submitted_at TIMESTAMPTZ,
    finalized_at TIMESTAMPTZ,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Indexer cursors (per contract)
CREATE TABLE indexer_cursors (
    contract_address BYTEA PRIMARY KEY CHECK (octet_length(contract_address) = 20),
    last_finalized_block BIGINT NOT NULL DEFAULT 0,
    last_finalized_block_hash BYTEA CHECK (octet_length(last_finalized_block_hash) = 32),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Reorg checkpoints (unfinalized blocks)
CREATE TABLE reorg_checkpoints (
    block_number BIGINT PRIMARY KEY,
    block_hash BYTEA NOT NULL CHECK (octet_length(block_hash) = 32),
    is_finalized BOOLEAN NOT NULL DEFAULT FALSE,
    inserted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Resolution proposals / optimistic oracle state
CREATE TABLE resolution_proposals (
    market_id BYTEA NOT NULL REFERENCES markets(market_id),
    round INT NOT NULL DEFAULT 0,
    resolver_id TEXT NOT NULL,
    commitment BYTEA CHECK (octet_length(commitment) = 32),
    proposed_payouts NUMERIC(78,0)[],
    confidence INT CHECK (confidence >= 0 AND confidence <= 100),
    evidence JSONB,
    status TEXT NOT NULL CHECK (status IN ('pending','committed','revealed','disputed','resolved')),
    proposer_address BYTEA CHECK (octet_length(proposer_address) = 20),
    disputer_address BYTEA CHECK (octet_length(disputer_address) = 20),
    dispute_reason TEXT,
    committed_at TIMESTAMPTZ,
    revealed_at TIMESTAMPTZ,
    finalized_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (market_id, round)
);

-- Matcher snapshots (crash recovery)
CREATE TABLE matcher_snapshots (
    market_id BYTEA PRIMARY KEY REFERENCES markets(market_id),
    snapshot_data BYTEA NOT NULL,
    kafka_offset BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Audit log (append-only)
CREATE TABLE audit_log (
    id BIGSERIAL PRIMARY KEY,
    service TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Indexes
CREATE INDEX idx_orders_user_status ON orders(user_address, status);
CREATE INDEX idx_orders_market_open ON orders(market_id, status, side, price DESC);
CREATE INDEX idx_holds_user_active ON holds(user_address, status) WHERE status = 'held';
CREATE TABLE indexed_logs (
    block_number BIGINT NOT NULL,
    tx_hash BYTEA NOT NULL CHECK (octet_length(tx_hash) = 32),
    log_index BIGINT NOT NULL,
    contract_address BYTEA NOT NULL CHECK (octet_length(contract_address) = 20),
    topic0 BYTEA NOT NULL CHECK (octet_length(topic0) = 32),
    PRIMARY KEY (tx_hash, log_index)
);
CREATE INDEX idx_indexed_logs_block ON indexed_logs(block_number);

CREATE TABLE order_cancellations (
    user_address BYTEA NOT NULL,
    order_id UUID NOT NULL REFERENCES orders(order_id),
    block_number BIGINT NOT NULL,
    tx_hash BYTEA NOT NULL,
    log_index BIGINT NOT NULL,
    PRIMARY KEY (tx_hash, log_index)
);

CREATE INDEX idx_reorg_unfinalized ON reorg_checkpoints(block_number, is_finalized) WHERE is_finalized = FALSE;
CREATE INDEX idx_audit_service_time ON audit_log(service, event_type, created_at);
CREATE INDEX idx_holds_released ON holds(released_at) WHERE status = 'released';
