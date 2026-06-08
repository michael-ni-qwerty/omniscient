// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

/// @notice Minimal 0.8.x ABI binding for the canonical Gnosis Conditional Tokens
///         Framework (deployed as 0.5.x bytecode). We bind by address, never compile
///         the upstream source. Outcome tokens are ERC-1155 positions; a complete set
///         splits/merges 1:1 against USDC collateral and a winning share redeems for 1 USDC.
interface IConditionalTokens {
    function prepareCondition(address oracle, bytes32 questionId, uint256 outcomeSlotCount) external;

    function reportPayouts(bytes32 questionId, uint256[] calldata payouts) external;

    function splitPosition(
        address collateralToken,
        bytes32 parentCollectionId,
        bytes32 conditionId,
        uint256[] calldata partition,
        uint256 amount
    ) external;

    function mergePositions(
        address collateralToken,
        bytes32 parentCollectionId,
        bytes32 conditionId,
        uint256[] calldata partition,
        uint256 amount
    ) external;

    function redeemPositions(
        address collateralToken,
        bytes32 parentCollectionId,
        bytes32 conditionId,
        uint256[] calldata indexSets
    ) external;

    function getConditionId(address oracle, bytes32 questionId, uint256 outcomeSlotCount)
        external
        pure
        returns (bytes32);

    function getCollectionId(bytes32 parentCollectionId, bytes32 conditionId, uint256 indexSet)
        external
        view
        returns (bytes32);

    function getPositionId(address collateralToken, bytes32 collectionId) external pure returns (uint256);

    function payoutDenominator(bytes32 conditionId) external view returns (uint256);

    function balanceOf(address account, uint256 positionId) external view returns (uint256);

    function safeTransferFrom(
        address from,
        address to,
        uint256 id,
        uint256 amount,
        bytes calldata data
    ) external;
}
