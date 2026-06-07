// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

interface ICustody {
    struct BalanceDelta {
        address account;
        int256 amount;
    }

    event Deposited(address indexed account, uint256 amount, uint256 newBalance);
    event Withdrawn(address indexed account, address indexed to, uint256 amount, uint256 newBalance);
    event ForcedWithdrawalRequested(address indexed account, uint256 indexed availableAt);
    event ForcedWithdrawalCancelled(address indexed account);
    event ForcedWithdrawalExecuted(address indexed account, address indexed to, uint256 amount);
    event NetDeltasApplied(bytes32 indexed batchId, uint256 accounts);
    event ForcedWithdrawalDelayUpdated(uint256 oldDelay, uint256 newDelay);

    error ZeroAmount();
    error ZeroAddress();
    error InsufficientBalance(uint256 available, uint256 requested);
    error AuthorizationExpired(uint256 deadline);
    error InvalidWithdrawalSigner(address recovered);
    error DeltasDoNotConserve(int256 net);
    error BatchAlreadyApplied(bytes32 batchId);
    error ForcedWithdrawalNotReady(uint256 availableAt);
    error NoForcedWithdrawalPending();
    error ForcedWithdrawalAlreadyPending();
    error DelayOutOfBounds(uint256 delay);

    function deposit(uint256 amount) external;
    function depositFor(address account, uint256 amount) external;
    function withdraw(uint256 amount, address to, uint256 deadline, bytes calldata operatorSig) external;
    function requestForcedWithdrawal() external;
    function cancelForcedWithdrawal() external;
    function executeForcedWithdrawal(address to) external;
    function applyNetDeltas(bytes32 batchId, BalanceDelta[] calldata deltas) external;

    function balanceOf(address account) external view returns (uint256);
    function totalCredited() external view returns (uint256);
    function withdrawalNonce(address account) external view returns (uint256);
}
