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
        keccak256("Withdrawal(address account,uint256 amount,address to,uint256 nonce,uint256 deadline)");

    uint256 public constant MIN_FORCED_WITHDRAWAL_DELAY = 1 hours;
    uint256 public constant MAX_FORCED_WITHDRAWAL_DELAY = 30 days;

    IERC20 public immutable USDC;

    uint256 public forcedWithdrawalDelay;

    mapping(address => uint256) private _balances;
    mapping(address => uint256) private _withdrawalNonce;
    mapping(address => uint256) private _forcedWithdrawalReadyAt;
    mapping(bytes32 => bool) private _appliedBatches;

    uint256 private _totalCredited;

    constructor(address usdc, address admin, uint256 initialForcedWithdrawalDelay)
        EIP712("Omniscient Custody", "1")
    {
        if (usdc == address(0) || admin == address(0)) revert ZeroAddress();
        if (
            initialForcedWithdrawalDelay < MIN_FORCED_WITHDRAWAL_DELAY
                || initialForcedWithdrawalDelay > MAX_FORCED_WITHDRAWAL_DELAY
        ) revert DelayOutOfBounds(initialForcedWithdrawalDelay);

        USDC = IERC20(usdc);
        forcedWithdrawalDelay = initialForcedWithdrawalDelay;
        _grantRole(DEFAULT_ADMIN_ROLE, admin);
    }

    // --- Deposits ---

    function deposit(uint256 amount) external override {
        _deposit(msg.sender, amount);
    }

    function depositFor(address account, uint256 amount) external override {
        if (account == address(0)) revert ZeroAddress();
        _deposit(account, amount);
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
        _totalCredited += received;
        emit Deposited(account, received, newBalance);
    }

    // --- Operator-authorized withdrawal (fast path) ---

    function withdraw(uint256 amount, address to, uint256 deadline, bytes calldata operatorSig)
        external
        override
        nonReentrant
        whenNotPaused
    {
        if (amount == 0) revert ZeroAmount();
        if (to == address(0)) revert ZeroAddress();
        if (block.timestamp > deadline) revert AuthorizationExpired(deadline);

        uint256 nonce = _withdrawalNonce[msg.sender];
        bytes32 structHash =
            keccak256(abi.encode(WITHDRAWAL_TYPEHASH, msg.sender, amount, to, nonce, deadline));
        address signer = ECDSA.recover(_hashTypedDataV4(structHash), operatorSig);
        if (!hasRole(WITHDRAWAL_SIGNER_ROLE, signer)) revert InvalidWithdrawalSigner(signer);

        _withdrawalNonce[msg.sender] = nonce + 1;
        _debitAndPay(msg.sender, to, amount);
        emit Withdrawn(msg.sender, to, amount, _balances[msg.sender]);
    }

    // --- Forced withdrawal escape hatch (no operator dependency) ---

    function requestForcedWithdrawal() external override {
        if (_forcedWithdrawalReadyAt[msg.sender] != 0) revert ForcedWithdrawalAlreadyPending();
        uint256 readyAt = block.timestamp + forcedWithdrawalDelay;
        _forcedWithdrawalReadyAt[msg.sender] = readyAt;
        emit ForcedWithdrawalRequested(msg.sender, readyAt);
    }

    function cancelForcedWithdrawal() external override {
        if (_forcedWithdrawalReadyAt[msg.sender] == 0) revert NoForcedWithdrawalPending();
        delete _forcedWithdrawalReadyAt[msg.sender];
        emit ForcedWithdrawalCancelled(msg.sender);
    }

    /// @notice Withdraws the caller's full balance after the timelock, even while paused.
    ///         Guarantees the operator can never permanently freeze user funds.
    function executeForcedWithdrawal(address to) external override nonReentrant {
        if (to == address(0)) revert ZeroAddress();
        uint256 readyAt = _forcedWithdrawalReadyAt[msg.sender];
        if (readyAt == 0) revert NoForcedWithdrawalPending();
        if (block.timestamp < readyAt) revert ForcedWithdrawalNotReady(readyAt);

        uint256 amount = _balances[msg.sender];
        if (amount == 0) revert ZeroAmount();

        delete _forcedWithdrawalReadyAt[msg.sender];
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

    // --- Admin ---

    function pause() external onlyRole(PAUSER_ROLE) {
        _pause();
    }

    function unpause() external onlyRole(PAUSER_ROLE) {
        _unpause();
    }

    function setForcedWithdrawalDelay(uint256 newDelay) external onlyRole(DEFAULT_ADMIN_ROLE) {
        if (newDelay < MIN_FORCED_WITHDRAWAL_DELAY || newDelay > MAX_FORCED_WITHDRAWAL_DELAY) {
            revert DelayOutOfBounds(newDelay);
        }
        uint256 old = forcedWithdrawalDelay;
        forcedWithdrawalDelay = newDelay;
        emit ForcedWithdrawalDelayUpdated(old, newDelay);
    }

    // --- Internal ---

    function _debitAndPay(address account, address to, uint256 amount) private {
        uint256 bal = _balances[account];
        if (bal < amount) revert InsufficientBalance(bal, amount);
        _balances[account] = bal - amount;
        _totalCredited -= amount;
        USDC.safeTransfer(to, amount);
    }

    // --- Views ---

    function balanceOf(address account) external view override returns (uint256) {
        return _balances[account];
    }

    function totalCredited() external view override returns (uint256) {
        return _totalCredited;
    }

    function withdrawalNonce(address account) external view override returns (uint256) {
        return _withdrawalNonce[account];
    }

    function forcedWithdrawalReadyAt(address account) external view returns (uint256) {
        return _forcedWithdrawalReadyAt[account];
    }

    function isBatchApplied(bytes32 batchId) external view returns (bool) {
        return _appliedBatches[batchId];
    }

    function domainSeparator() external view returns (bytes32) {
        return _domainSeparatorV4();
    }
}
