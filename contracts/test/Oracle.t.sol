// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import { Test } from "forge-std/Test.sol";
import { Oracle } from "../src/Oracle.sol";
import { IConditionalTokens } from "../src/interfaces/IConditionalTokens.sol";
import { MockUSDC } from "./mocks/MockUSDC.sol";
import { Pausable } from "@openzeppelin/contracts/utils/Pausable.sol";

contract OracleMockCTF is IConditionalTokens {
    mapping(bytes32 => bool) public prepared;
    mapping(bytes32 => bool) public didReport;
    mapping(bytes32 => uint256[]) private _reported;

    function prepareCondition(address, bytes32 questionId, uint256) external override {
        prepared[questionId] = true;
    }

    function reportPayouts(bytes32 questionId, uint256[] calldata payouts) external override {
        _reported[questionId] = payouts;
        didReport[questionId] = true;
    }

    function getReported(bytes32 questionId) external view returns (uint256[] memory) {
        return _reported[questionId];
    }
    function splitPosition(address, bytes32, bytes32, uint256[] calldata, uint256) external override { }
    function mergePositions(address, bytes32, bytes32, uint256[] calldata, uint256) external override { }
    function redeemPositions(address, bytes32, bytes32, uint256[] calldata) external override { }

    function getConditionId(address, bytes32, uint256) external pure override returns (bytes32) {
        return bytes32(0);
    }

    function getCollectionId(bytes32, bytes32, uint256) external pure override returns (bytes32) {
        return bytes32(0);
    }

    function getPositionId(address, bytes32) external pure override returns (uint256) {
        return 0;
    }

    function payoutDenominator(bytes32) external pure override returns (uint256) {
        return 0;
    }

    function balanceOf(address, uint256) external pure override returns (uint256) {
        return 0;
    }
    function safeTransferFrom(address, address, uint256, uint256, bytes calldata) external override { }
}

contract OracleTest is Test {
    Oracle public oracle;
    OracleMockCTF public ctf;
    MockUSDC public usdc;

    address public admin = address(this);
    address public arbitrator = makeAddr("arbitrator");
    address public marketCreator = makeAddr("marketCreator");
    address public pauser = makeAddr("pauser");
    address public proposer = makeAddr("proposer");
    address public disputer = makeAddr("disputer");
    address public slasher = makeAddr("slasher");

    uint256 public constant UNIT = 1e6;
    uint256 public constant EXPIRY = 1_000_000;
    uint256 public constant BOND = 100 * UNIT;
    uint256 public constant DISPUTE_WINDOW = 2 hours;

    bytes32 public constant MARKET_BC = keccak256("marketBC");
    bytes32 public constant Q_BC = keccak256("qBC");

    function setUp() public {
        usdc = new MockUSDC();
        ctf = new OracleMockCTF();
        oracle = new Oracle(address(ctf), address(usdc));

        oracle.grantRole(oracle.MARKET_CREATOR_ROLE(), marketCreator);
        oracle.grantRole(oracle.ARBITRATOR_ROLE(), arbitrator);
        oracle.grantRole(oracle.PAUSER_ROLE(), pauser);

        usdc.mint(proposer, 1_000 * UNIT);
        usdc.mint(disputer, 1_000 * UNIT);
        vm.prank(proposer);
        usdc.approve(address(oracle), type(uint256).max);
        vm.prank(disputer);
        usdc.approve(address(oracle), type(uint256).max);
    }

    // --- helpers ---

    function _specBC() internal pure returns (Oracle.ResolutionSpec memory) {
        return Oracle.ResolutionSpec({
            questionId: Q_BC,
            outcomeSlotCount: 2,
            expiry: EXPIRY,
            disputeWindow: DISPUTE_WINDOW,
            bondAmount: BOND
        });
    }

    function _createBC() internal {
        vm.prank(marketCreator);
        oracle.createMarket(MARKET_BC, _specBC());
    }

    function _yes() internal pure returns (uint256[] memory p) {
        p = new uint256[](2);
        p[1] = 1;
    }

    function _no() internal pure returns (uint256[] memory p) {
        p = new uint256[](2);
        p[0] = 1;
    }

    // --- createMarket ---

    function test_CreateMarket_Success() public {
        _createBC();
        assertTrue(ctf.prepared(Q_BC));
        (bytes32 qid,,,,) = oracle.specs(MARKET_BC);
        assertEq(qid, Q_BC);
    }

    function test_CreateMarket_RevertDuplicate() public {
        _createBC();
        vm.prank(marketCreator);
        vm.expectRevert(Oracle.InvalidState.selector);
        oracle.createMarket(MARKET_BC, _specBC());
    }

    function test_CreateMarket_RevertNotCreatorRole() public {
        vm.prank(proposer);
        vm.expectRevert();
        oracle.createMarket(MARKET_BC, _specBC());
    }

    function test_CreateMarket_RevertWhenPaused() public {
        vm.prank(pauser);
        oracle.pause();
        vm.prank(marketCreator);
        vm.expectRevert(Pausable.EnforcedPause.selector);
        oracle.createMarket(MARKET_BC, _specBC());
    }

    // --- proposeOutcome ---

    function _propose(uint256[] memory payouts) internal {
        _createBC();
        vm.warp(EXPIRY);
        vm.prank(proposer);
        oracle.proposeOutcome(MARKET_BC, payouts);
    }

    function test_Propose_Success() public {
        _createBC();
        vm.warp(EXPIRY);
        uint256 beforeBal = usdc.balanceOf(proposer);
        vm.prank(proposer);
        oracle.proposeOutcome(MARKET_BC, _yes());
        assertEq(uint256(oracle.states(MARKET_BC)), uint256(Oracle.MarketState.PROPOSED));
        assertEq(usdc.balanceOf(address(oracle)), BOND);
        assertEq(usdc.balanceOf(proposer), beforeBal - BOND);
        (bytes32 payoutsHash, uint256 proposeTime, address p, address d) = oracle.optimisticStates(MARKET_BC);
        assertEq(payoutsHash, keccak256(abi.encode(_yes())));
        assertEq(proposeTime, EXPIRY);
        assertEq(p, proposer);
        assertEq(d, address(0));
    }

    function test_Propose_RevertNotOpen() public {
        _propose(_yes());
        vm.prank(proposer);
        vm.expectRevert(Oracle.InvalidState.selector);
        oracle.proposeOutcome(MARKET_BC, _yes());
    }

    function test_Propose_RevertNotExpired() public {
        _createBC();
        vm.warp(EXPIRY - 1);
        vm.prank(proposer);
        vm.expectRevert(Oracle.NotExpired.selector);
        oracle.proposeOutcome(MARKET_BC, _yes());
    }

    function test_Propose_RevertWrongLength() public {
        _createBC();
        vm.warp(EXPIRY);
        uint256[] memory bad = new uint256[](3);
        bad[0] = 1;
        vm.prank(proposer);
        vm.expectRevert(Oracle.InvalidPayouts.selector);
        oracle.proposeOutcome(MARKET_BC, bad);
    }

    function test_Propose_RevertZeroSum() public {
        _createBC();
        vm.warp(EXPIRY);
        uint256[] memory z = new uint256[](2);
        vm.prank(proposer);
        vm.expectRevert(Oracle.InvalidPayouts.selector);
        oracle.proposeOutcome(MARKET_BC, z);
    }

    function test_Propose_RevertWhenPaused() public {
        _createBC();
        vm.prank(pauser);
        oracle.pause();
        vm.warp(EXPIRY);
        vm.prank(proposer);
        vm.expectRevert(Pausable.EnforcedPause.selector);
        oracle.proposeOutcome(MARKET_BC, _yes());
    }

    // --- disputeOutcome ---

    function _proposeDispute() internal {
        _propose(_yes());
        vm.prank(disputer);
        oracle.disputeOutcome(MARKET_BC, "wrong");
    }

    function test_Dispute_Success() public {
        _propose(_yes());
        uint256 beforeDisputer = usdc.balanceOf(disputer);
        vm.prank(disputer);
        oracle.disputeOutcome(MARKET_BC, "wrong");
        assertEq(uint256(oracle.states(MARKET_BC)), uint256(Oracle.MarketState.DISPUTED));
        assertEq(usdc.balanceOf(address(oracle)), 2 * BOND);
        assertEq(usdc.balanceOf(disputer), beforeDisputer - BOND);
    }

    function test_Dispute_RevertNotProposed() public {
        _createBC();
        vm.expectRevert(Oracle.InvalidState.selector);
        oracle.disputeOutcome(MARKET_BC, "wrong");
    }

    function test_Dispute_RevertWindowPassed() public {
        _propose(_yes());
        vm.warp(EXPIRY + DISPUTE_WINDOW + 1);
        vm.prank(disputer);
        vm.expectRevert(Oracle.DisputeWindowPassed.selector);
        oracle.disputeOutcome(MARKET_BC, "wrong");
    }

    function test_Dispute_RevertWhenPaused() public {
        _propose(_yes());
        vm.prank(pauser);
        oracle.pause();
        vm.prank(disputer);
        vm.expectRevert(Pausable.EnforcedPause.selector);
        oracle.disputeOutcome(MARKET_BC, "wrong");
    }

    // --- resolveDispute ---

    function test_ResolveDispute_DisputerRight() public {
        _proposeDispute();
        uint256 beforeDisputer = usdc.balanceOf(disputer);
        vm.prank(arbitrator);
        oracle.resolveDispute(MARKET_BC, _no());
        assertEq(uint256(oracle.states(MARKET_BC)), uint256(Oracle.MarketState.RESOLVED));
        assertEq(usdc.balanceOf(disputer), beforeDisputer + 2 * BOND);
        uint256[] memory r = ctf.getReported(Q_BC);
        assertEq(r[0], 1);
        assertEq(r[1], 0);
    }

    function test_ResolveDispute_ProposerRight() public {
        _proposeDispute();
        uint256 beforeProposer = usdc.balanceOf(proposer);
        vm.prank(arbitrator);
        oracle.resolveDispute(MARKET_BC, _yes());
        assertEq(usdc.balanceOf(proposer), beforeProposer + 2 * BOND);
        uint256[] memory r = ctf.getReported(Q_BC);
        assertEq(r[0], 0);
        assertEq(r[1], 1);
    }

    function test_ResolveDispute_RevertNotArbitrator() public {
        _proposeDispute();
        vm.prank(proposer);
        vm.expectRevert();
        oracle.resolveDispute(MARKET_BC, _yes());
    }

    function test_ResolveDispute_RevertNotDisputed() public {
        _propose(_yes());
        vm.prank(arbitrator);
        vm.expectRevert(Oracle.InvalidState.selector);
        oracle.resolveDispute(MARKET_BC, _yes());
    }

    function test_ResolveDispute_RevertInvalidPayouts() public {
        _proposeDispute();
        uint256[] memory z = new uint256[](2);
        vm.prank(arbitrator);
        vm.expectRevert(Oracle.InvalidPayouts.selector);
        oracle.resolveDispute(MARKET_BC, z);
    }

    // --- finalizeOutcome ---

    function test_Finalize_Success() public {
        _propose(_yes());
        uint256 beforeProposer = usdc.balanceOf(proposer);
        vm.warp(EXPIRY + DISPUTE_WINDOW + 1);
        oracle.finalizeOutcome(MARKET_BC, _yes());
        assertEq(uint256(oracle.states(MARKET_BC)), uint256(Oracle.MarketState.RESOLVED));
        uint256[] memory r = ctf.getReported(Q_BC);
        assertEq(r[0], 0);
        assertEq(r[1], 1);
        assertEq(usdc.balanceOf(proposer), beforeProposer + BOND);
    }

    function test_Finalize_RevertNotProposed() public {
        _createBC();
        vm.expectRevert(Oracle.InvalidState.selector);
        oracle.finalizeOutcome(MARKET_BC, _yes());
    }

    function test_Finalize_RevertWindowNotPassed() public {
        _propose(_yes());
        vm.warp(EXPIRY + 1);
        vm.expectRevert(Oracle.DisputeWindowNotPassed.selector);
        oracle.finalizeOutcome(MARKET_BC, _yes());
    }

    function test_Finalize_RevertPayoutsMismatch() public {
        _propose(_yes());
        vm.warp(EXPIRY + DISPUTE_WINDOW + 1);
        vm.expectRevert(Oracle.PayoutsMismatch.selector);
        oracle.finalizeOutcome(MARKET_BC, _no());
    }

    // --- pause access control ---

    function test_Pause_RevertNotPauser() public {
        vm.prank(proposer);
        vm.expectRevert();
        oracle.pause();
    }

    function test_Unpause_Success() public {
        vm.prank(pauser);
        oracle.pause();
        assertTrue(oracle.paused());
        vm.prank(pauser);
        oracle.unpause();
        assertFalse(oracle.paused());
    }
}
