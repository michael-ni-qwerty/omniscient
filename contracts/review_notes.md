# Review of Custody.sol

## 1. Architectural & Integration Risks (Off-Chain Engine Coupling)
- **Forced Withdrawal "Pre-Arming"**: A user can call `requestForcedWithdrawal()` when their balance is `0` (or leave a 1 wei dust balance), wait out the timelock, and keep their account in a "ready" state indefinitely because `executeForcedWithdrawal()` will just revert with `ZeroAmount` on a 0 balance without clearing the `readyAt` timestamp. If they subsequently deposit a large amount, they could instantly call `executeForcedWithdrawal()` and bypass the delay. 
  - *Risk*: This relies entirely on the off-chain engine indexing `ForcedWithdrawalRequested` and strictly keeping the account `HALTED` (refusing to allow trades) even if the balance is 0, until `cancelForcedWithdrawal()` is explicitly called. If the indexer ignores the event for 0-balance accounts, it opens a double-spend vector.

- **Head-of-Line Blocking in Fast Withdrawals**: The `withdraw` function uses a strictly sequential on-chain nonce (`_withdrawalNonce[msg.sender]`). 
  - *Risk*: If the operator issues a withdrawal signature (e.g., `nonce = 0`) and the user fails to submit it (e.g., drops offline), they cannot submit any subsequent withdrawal signatures (`nonce = 1`) because the on-chain nonce hasn't incremented. The off-chain engine must handle this by waiting for the signature's `deadline` to expire before refunding the off-chain balance and issuing a new signature with the current on-chain nonce.

## 2. Minor Code Improvements & Semantics
- **Misused Error Selector**: In `executeForcedWithdrawal` and `withdraw`, the check `if (to == address(0))` reverts with `ZeroAmount()`. This is semantically incorrect and could confuse client UI/indexer parsing. Consider adding a `ZeroAddress()` error to `ICustody.sol`.
- **Gas Optimization in `applyNetDeltas`**: The loop uses `++i`, but in Solidity 0.8.x this still includes checked arithmetic. Wrapping `++i` in an `unchecked {}` block would save gas, especially since `deltas.length` is strictly bounded by the block gas limit and will never overflow a `uint256`.

## Conclusion
The contract is exceptionally well-written, secure, and adheres strictly to the requirements (conservation invariant, idempotent settlement, EIP-712 auth, and emergency escape hatches). The core fund-safety invariants are robustly protected on-chain.
