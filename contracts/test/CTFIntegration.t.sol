// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import "forge-std/Test.sol";
import { IConditionalTokens } from "../src/interfaces/IConditionalTokens.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

/// @notice Fork integration test against live Polygon Amoy testnet.
///         Verifies IConditionalTokens interface bindings against real deployed bytecode.
contract CTFIntegrationTest is Test {
    /// @dev Polymarket CTF Exchange on Amoy => getCtf() => 0x69308FB512518e39F9b16112fA8d994F4e2Bf8bB
    IConditionalTokens constant CTF = IConditionalTokens(0x69308FB512518e39F9b16112fA8d994F4e2Bf8bB);
    /// @dev Polymarket collateral token on Amoy (getCollateral() from exchange)
    IERC20 constant COLLATERAL = IERC20(0x9c4E1703476E875070EE25b56A58B008CFb8FA78);

    uint256 forkId;

    function setUp() public {
        forkId = vm.createSelectFork("amoy");
    }

    function onERC1155Received(address, address, uint256, uint256, bytes calldata)
        external
        pure
        returns (bytes4)
    {
        return this.onERC1155Received.selector;
    }

    function onERC1155BatchReceived(address, address, uint256[] calldata, uint256[] calldata, bytes calldata)
        external
        pure
        returns (bytes4)
    {
        return this.onERC1155BatchReceived.selector;
    }

    /// @notice Round-trip split + merge against real CTF.
    ///         1. Deal collateral to this contract.
    ///         2. Approve CTF.
    ///         3. Prepare a fresh condition.
    ///         4. Split 1 unit of collateral into a complete set.
    ///         5. Assert balances of both outcome shares.
    ///         6. Merge back and assert full recovery.
    function test_SplitAndMergeRoundTrip() public {
        uint256 amount = 1e6; // 6-decimal collateral

        // Fund this test contract with collateral
        deal(address(COLLATERAL), address(this), amount);

        uint256 balBefore = COLLATERAL.balanceOf(address(this));
        assertEq(balBefore, amount, "deal failed");

        // Approve CTF to pull collateral
        COLLATERAL.approve(address(CTF), amount);

        // Prepare condition (questionId must be unique per test run)
        bytes32 questionId = keccak256(abi.encodePacked("omniscient-ctf-test", block.timestamp));
        CTF.prepareCondition(address(this), questionId, 2); // binary outcome

        bytes32 conditionId = CTF.getConditionId(address(this), questionId, 2);

        uint256[] memory partition = new uint256[](2);
        partition[0] = 1; // YES
        partition[1] = 2; // NO

        // Split 1 unit of collateral into a complete set of outcome shares
        CTF.splitPosition(address(COLLATERAL), bytes32(0), conditionId, partition, amount);

        // Collateral should have been pulled
        assertEq(COLLATERAL.balanceOf(address(this)), balBefore - amount, "collateral not pulled");

        // Derive position IDs
        bytes32 collYes = CTF.getCollectionId(bytes32(0), conditionId, 1);
        bytes32 collNo  = CTF.getCollectionId(bytes32(0), conditionId, 2);
        uint256 posYes  = CTF.getPositionId(address(COLLATERAL), collYes);
        uint256 posNo   = CTF.getPositionId(address(COLLATERAL), collNo);

        // Should hold exactly `amount` of each outcome share
        assertEq(CTF.balanceOf(address(this), posYes), amount, "YES share balance mismatch");
        assertEq(CTF.balanceOf(address(this), posNo), amount, "NO share balance mismatch");

        // Merge back to collateral (burns caller's own shares; no approval needed)
        CTF.mergePositions(address(COLLATERAL), bytes32(0), conditionId, partition, amount);

        // Full recovery
        assertEq(COLLATERAL.balanceOf(address(this)), balBefore, "collateral not recovered");
        assertEq(CTF.balanceOf(address(this), posYes), 0, "YES share not burned");
        assertEq(CTF.balanceOf(address(this), posNo), 0, "NO share not burned");
    }
}
