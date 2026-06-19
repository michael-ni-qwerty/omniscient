// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import { AccessControl } from "@openzeppelin/contracts/access/AccessControl.sol";
import { Pausable } from "@openzeppelin/contracts/utils/Pausable.sol";
import { ReentrancyGuard } from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import { EIP712 } from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import { ECDSA } from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";

import { ICustody } from "./interfaces/ICustody.sol";

/// @title Custody
/// @notice Non-custodial USDC vault. Conservation invariant:
///         USDC.balanceOf(this) == totalCredited == sum(_balances) at all times.
contract Custody is ICustody, AccessControl, Pausable, ReentrancyGuard, EIP712 {
    using SafeERC20 for IERC20;

    bytes32 public constant PAUSER_ROLE = keccak256("PAUSER_ROLE");
    bytes32 public constant SETTLEMENT_ROLE = keccak256("SETTLEMENT_ROLE");
    bytes32 public constant WITHDRAWAL_SIGNER_ROLE = keccak256("WITHDRAWAL_SIGNER_ROLE");

    bytes32 private constant WITHDRAWAL_TYPEHASH =
        keccak256("Withdrawal(address account,uint256 amount,uint256 nonce,uint256 deadline)");

    uint256 public constant MIN_OPERATOR_INACTIVITY_THRESHOLD = 1 days;
    uint256 public constant MAX_OPERATOR_INACTIVITY_THRESHOLD = 90 days;

    IERC20 public immutable USDC;

    /// @notice Timestamp of the last operator-initiated on-chain action (settlement or heartbeat).
    ///         The forced-withdrawal hatch only unlocks once the operator is provably absent.
    uint256 public lastOperatorActivity;

    /// @notice Max operator silence (seconds) before the forced-withdrawal hatch unlocks.
    uint256 public operatorInactivityThreshold;

    mapping(address => uint256) private _balances;
    mapping(address => uint256) private _withdrawalNonce;
    mapping(bytes32 => bool) private _appliedBatches;

    constructor(address usdc, address admin, uint256 initialInactivityThreshold)
        EIP712("Omniscient Custody", "1")
    {
        if (usdc == address(0) || admin == address(0)) revert ZeroAddress();
        if (
            initialInactivityThreshold < MIN_OPERATOR_INACTIVITY_THRESHOLD
                || initialInactivityThreshold > MAX_OPERATOR_INACTIVITY_THRESHOLD
        ) revert InactivityThresholdOutOfBounds(initialInactivityThreshold);

        USDC = IERC20(usdc);
        operatorInactivityThreshold = initialInactivityThreshold;
        lastOperatorActivity = block.timestamp;
        _grantRole(DEFAULT_ADMIN_ROLE, admin);
    }

    // --- Deposits ---

    function deposit(uint256 amount) external override {
        _deposit(msg.sender, amount);
    }


    function _deposit(address account, uint256 amount) private nonReentrant whenNotPaused {
        if (amount == 0) revert ZeroAmount();
        // Credit the measured delta so fee-on-transfer tokens cannot break conservation.
        uint256 before = USDC.balanceOf(address(this));
        USDC.safeTransferFrom(msg.sender, address(this), amount);
        uint256 received = USDC.balanceOf(address(this)) - before;
        if (received == 0) revert ZeroAmount();

        uint256 newBalance = _balances[account] + received;
        _balances[account] = newBalance;
        emit Deposited(account, received, newBalance);
    }

    // --- Operator-authorized withdrawal (fast path) ---

    /// @notice Operator-authorized fast withdrawal. Funds always go to the caller's own wallet:
    ///         the operator signature authorizes whether/how much, never the destination, so a
    ///         rogue operator (or malicious frontend) can never reroute a user's funds.
    function withdraw(uint256 amount, uint256 deadline, bytes calldata operatorSig)
        external
        override
        nonReentrant
        whenNotPaused
    {
        if (amount == 0) revert ZeroAmount();
        if (block.timestamp > deadline) revert AuthorizationExpired(deadline);

        uint256 nonce = _withdrawalNonce[msg.sender];
        bytes32 structHash =
            keccak256(abi.encode(WITHDRAWAL_TYPEHASH, msg.sender, amount, nonce, deadline));
        address signer = ECDSA.recover(_hashTypedDataV4(structHash), operatorSig);
        if (!hasRole(WITHDRAWAL_SIGNER_ROLE, signer)) revert InvalidWithdrawalSigner(signer);

        _withdrawalNonce[msg.sender] = nonce + 1;
        _debitAndPay(msg.sender, msg.sender, amount);
        emit Withdrawn(msg.sender, amount, _balances[msg.sender]);
    }

    // --- Forced withdrawal escape hatch (unlocks only when operator is provably absent) ---

    /// @notice Withdraws the caller's full balance in a single call, but only once the operator
    ///         has been silent past the inactivity threshold (no settlement or heartbeat).
    ///         Liveness can only be refreshed while unpaused, so a paused contract still opens
    ///         the hatch after the threshold — the operator can never permanently freeze funds.
    function executeForcedWithdrawal(address to) external override nonReentrant {
        if (to == address(0)) revert ZeroAddress();
        if (!_operatorAbsent()) revert OperatorActive(lastOperatorActivity);

        uint256 amount = _balances[msg.sender];
        if (amount == 0) revert ZeroAmount();

        _debitAndPay(msg.sender, to, amount);
        emit ForcedWithdrawalExecuted(msg.sender, to, amount);
    }

    // --- Settlement: net position deltas (batched, idempotent, conserving) ---

    function applyNetDeltas(bytes32 batchId, BalanceDelta[] calldata deltas)
        external
        override
        nonReentrant
        whenNotPaused
        onlyRole(SETTLEMENT_ROLE)
    {
        if (_appliedBatches[batchId]) revert BatchAlreadyApplied(batchId);
        _appliedBatches[batchId] = true;
        _recordOperatorActivity();

        int256 net;
        uint256 len = deltas.length;
        for (uint256 i; i < len;) {
            net += deltas[i].amount;
            unchecked {
                ++i;
            }
        }
        if (net != 0) revert DeltasDoNotConserve(net);

        for (uint256 i; i < len;) {
            address account = deltas[i].account;
            int256 amount = deltas[i].amount;
            if (amount != 0) {
                if (amount > 0) {
                    _balances[account] += uint256(amount);
                } else {
                    uint256 debit = uint256(-amount);
                    uint256 bal = _balances[account];
                    if (bal < debit) revert InsufficientBalance(bal, debit);
                    _balances[account] = bal - debit;
                }
            }
            unchecked {
                ++i;
            }
        }
        // Conservation: net == 0 means no value created/destroyed; _totalCredited unchanged.
        emit NetDeltasApplied(batchId, len);
    }

    // --- Operator liveness ---

    /// @notice Refreshes the operator liveness clock without performing a settlement.
    ///         The backend calls this during quiet periods to keep the escape hatch dormant.
    function heartbeat() external override whenNotPaused onlyRole(SETTLEMENT_ROLE) {
        _recordOperatorActivity();
    }

    // --- Admin ---

    function pause() external onlyRole(PAUSER_ROLE) {
        _pause();
    }

    function unpause() external onlyRole(PAUSER_ROLE) {
        _unpause();
    }

    function setOperatorInactivityThreshold(uint256 newThreshold)
        external
        override
        onlyRole(DEFAULT_ADMIN_ROLE)
    {
        if (
            newThreshold < MIN_OPERATOR_INACTIVITY_THRESHOLD
                || newThreshold > MAX_OPERATOR_INACTIVITY_THRESHOLD
        ) revert InactivityThresholdOutOfBounds(newThreshold);
        operatorInactivityThreshold = newThreshold;
        emit OperatorInactivityThresholdUpdated(newThreshold);
    }

    // --- Internal ---

    /// @dev Operator is absent once it has been silent past the threshold. Liveness is only
    ///      refreshable while unpaused (settlement + heartbeat are whenNotPaused), so a pause
    ///      cannot indefinitely hold the hatch shut.
    function _operatorAbsent() private view returns (bool) {
        return block.timestamp - lastOperatorActivity > operatorInactivityThreshold;
    }

    function _recordOperatorActivity() private {
        lastOperatorActivity = block.timestamp;
        emit OperatorHeartbeat(block.timestamp);
    }

    function _debitAndPay(address account, address to, uint256 amount) private {
        uint256 bal = _balances[account];
        if (bal < amount) revert InsufficientBalance(bal, amount);
        _balances[account] = bal - amount;
        USDC.safeTransfer(to, amount);
    }

    // --- Views ---

    function balanceOf(address account) external view override returns (uint256) {
        return _balances[account];
    }

    function withdrawalNonce(address account) external view override returns (uint256) {
        return _withdrawalNonce[account];
    }

    function isBatchApplied(bytes32 batchId) external view returns (bool) {
        return _appliedBatches[batchId];
    }

    function domainSeparator() external view returns (bytes32) {
        return _domainSeparatorV4();
    }
}
