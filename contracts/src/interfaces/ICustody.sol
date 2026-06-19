// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

interface ICustody {
    struct BalanceDelta {
        address account;
        int256 amount;
    }

    event Deposited(address indexed account, uint256 amount, uint256 newBalance);
    event Withdrawn(address indexed account, uint256 amount, uint256 newBalance);
    event ForcedWithdrawalExecuted(address indexed account, address indexed to, uint256 amount);
    event NetDeltasApplied(bytes32 indexed batchId, uint256 accounts);
    event OperatorHeartbeat(uint256 timestamp);
    event OperatorInactivityThresholdUpdated(uint256 newThreshold);

    error ZeroAmount();
    error ZeroAddress();
    error InsufficientBalance(uint256 available, uint256 requested);
    error AuthorizationExpired(uint256 deadline);
    error InvalidWithdrawalSigner(address recovered);
    error DeltasDoNotConserve(int256 net);
    error BatchAlreadyApplied(bytes32 batchId);
    error OperatorActive(uint256 lastOperatorActivity);
    error InactivityThresholdOutOfBounds(uint256 threshold);

    function deposit(uint256 amount) external;
    function withdraw(uint256 amount, uint256 deadline, bytes calldata operatorSig) external;
    function executeForcedWithdrawal(address to) external;
    function applyNetDeltas(bytes32 batchId, BalanceDelta[] calldata deltas) external;
    function heartbeat() external;
    function setOperatorInactivityThreshold(uint256 newThreshold) external;

    function balanceOf(address account) external view returns (uint256);
    function withdrawalNonce(address account) external view returns (uint256);
}
