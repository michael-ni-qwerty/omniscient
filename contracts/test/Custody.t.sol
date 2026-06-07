// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import { Test } from "forge-std/Test.sol";
import { Custody } from "../src/Custody.sol";
import { ICustody } from "../src/interfaces/ICustody.sol";
import { MockUSDC } from "./mocks/MockUSDC.sol";
import { Pausable } from "@openzeppelin/contracts/utils/Pausable.sol";
import { IAccessControl } from "@openzeppelin/contracts/access/IAccessControl.sol";

contract CustodyTest is Test {
    bytes32 private constant WITHDRAWAL_TYPEHASH =
        keccak256("Withdrawal(address account,uint256 amount,address to,uint256 nonce,uint256 deadline)");

    Custody internal custody;
    MockUSDC internal usdc;

    address internal admin = makeAddr("admin");
    address internal settlement = makeAddr("settlement");
    address internal pauser = makeAddr("pauser");
    address internal alice = makeAddr("alice");
    address internal bob = makeAddr("bob");

    uint256 internal operatorPk = 0xA11CE5;
    address internal operator;

    uint256 internal constant DELAY = 1 days;
    uint256 internal constant UNIT = 1e6;

    function setUp() public {
        operator = vm.addr(operatorPk);
        usdc = new MockUSDC();
        custody = new Custody(address(usdc), admin, DELAY);

        vm.startPrank(admin);
        custody.grantRole(custody.SETTLEMENT_ROLE(), settlement);
        custody.grantRole(custody.PAUSER_ROLE(), pauser);
        custody.grantRole(custody.WITHDRAWAL_SIGNER_ROLE(), operator);
        vm.stopPrank();

        usdc.mint(alice, 1_000 * UNIT);
        usdc.mint(bob, 1_000 * UNIT);
    }

    // --- helpers ---

    function _deposit(address who, uint256 amount) internal {
        vm.startPrank(who);
        usdc.approve(address(custody), amount);
        custody.deposit(amount);
        vm.stopPrank();
    }

    function _signWithdrawal(address account, uint256 amount, address to, uint256 nonce, uint256 deadline)
        internal
        view
        returns (bytes memory)
    {
        bytes32 structHash = keccak256(abi.encode(WITHDRAWAL_TYPEHASH, account, amount, to, nonce, deadline));
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", custody.domainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(operatorPk, digest);
        return abi.encodePacked(r, s, v);
    }

    function _assertConservation() internal view {
        assertEq(usdc.balanceOf(address(custody)), custody.totalCredited(), "conservation broken");
    }

    // --- deposit ---

    function test_Deposit_CreditsAndConserves() public {
        _deposit(alice, 100 * UNIT);
        assertEq(custody.balanceOf(alice), 100 * UNIT);
        _assertConservation();
    }

    function test_Deposit_RevertsZero() public {
        vm.prank(alice);
        vm.expectRevert(ICustody.ZeroAmount.selector);
        custody.deposit(0);
    }

    function test_Deposit_RevertsWhenPaused() public {
        vm.prank(pauser);
        custody.pause();
        vm.startPrank(alice);
        usdc.approve(address(custody), 1 * UNIT);
        vm.expectRevert(Pausable.EnforcedPause.selector);
        custody.deposit(1 * UNIT);
        vm.stopPrank();
    }

    // --- operator-authorized withdrawal ---

    function test_Withdraw_WithValidOperatorSig() public {
        _deposit(alice, 100 * UNIT);
        uint256 amount = 40 * UNIT;
        uint256 deadline = block.timestamp + 1 hours;
        bytes memory sig = _signWithdrawal(alice, amount, alice, 0, deadline);

        vm.prank(alice);
        custody.withdraw(amount, alice, deadline, sig);

        assertEq(custody.balanceOf(alice), 60 * UNIT);
        assertEq(usdc.balanceOf(alice), 940 * UNIT);
        assertEq(custody.withdrawalNonce(alice), 1);
        _assertConservation();
    }

    function test_Withdraw_RevertsOnReplay() public {
        _deposit(alice, 100 * UNIT);
        uint256 amount = 10 * UNIT;
        uint256 deadline = block.timestamp + 1 hours;
        bytes memory sig = _signWithdrawal(alice, amount, alice, 0, deadline);

        vm.prank(alice);
        custody.withdraw(amount, alice, deadline, sig);

        vm.prank(alice);
        vm.expectRevert();
        custody.withdraw(amount, alice, deadline, sig);
    }

    function test_Withdraw_RevertsBadSigner() public {
        _deposit(alice, 100 * UNIT);
        uint256 deadline = block.timestamp + 1 hours;
        bytes32 structHash = keccak256(
            abi.encode(WITHDRAWAL_TYPEHASH, alice, uint256(10 * UNIT), alice, uint256(0), deadline)
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", custody.domainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(0xBADBAD, digest);
        bytes memory sig = abi.encodePacked(r, s, v);

        vm.prank(alice);
        vm.expectRevert();
        custody.withdraw(10 * UNIT, alice, deadline, sig);
    }

    function test_Withdraw_RevertsExpired() public {
        _deposit(alice, 100 * UNIT);
        uint256 deadline = block.timestamp + 1 hours;
        bytes memory sig = _signWithdrawal(alice, 10 * UNIT, alice, 0, deadline);
        vm.warp(deadline + 1);
        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSelector(ICustody.AuthorizationExpired.selector, deadline));
        custody.withdraw(10 * UNIT, alice, deadline, sig);
    }

    // --- forced withdrawal escape hatch ---

    function test_ForcedWithdrawal_HappyPath() public {
        _deposit(alice, 100 * UNIT);
        vm.prank(alice);
        custody.requestForcedWithdrawal();

        vm.warp(block.timestamp + DELAY);
        vm.prank(alice);
        custody.executeForcedWithdrawal(alice);

        assertEq(custody.balanceOf(alice), 0);
        assertEq(usdc.balanceOf(alice), 1_000 * UNIT);
        _assertConservation();
    }

    function test_ForcedWithdrawal_WorksWhilePaused() public {
        _deposit(alice, 100 * UNIT);
        vm.prank(alice);
        custody.requestForcedWithdrawal();
        vm.warp(block.timestamp + DELAY);

        vm.prank(pauser);
        custody.pause();

        vm.prank(alice);
        custody.executeForcedWithdrawal(alice);
        assertEq(custody.balanceOf(alice), 0);
        _assertConservation();
    }

    function test_ForcedWithdrawal_RevertsBeforeReady() public {
        _deposit(alice, 100 * UNIT);
        vm.prank(alice);
        custody.requestForcedWithdrawal();

        vm.prank(alice);
        vm.expectRevert();
        custody.executeForcedWithdrawal(alice);
    }

    // --- settlement net deltas ---

    function test_ApplyNetDeltas_ConservesValue() public {
        _deposit(alice, 100 * UNIT);
        _deposit(bob, 100 * UNIT);

        ICustody.BalanceDelta[] memory deltas = new ICustody.BalanceDelta[](2);
        deltas[0] = ICustody.BalanceDelta({ account: alice, amount: -30 * int256(UNIT) });
        deltas[1] = ICustody.BalanceDelta({ account: bob, amount: 30 * int256(UNIT) });

        vm.prank(settlement);
        custody.applyNetDeltas(keccak256("batch-1"), deltas);

        assertEq(custody.balanceOf(alice), 70 * UNIT);
        assertEq(custody.balanceOf(bob), 130 * UNIT);
        _assertConservation();
    }

    function test_ApplyNetDeltas_RevertsNonConserving() public {
        _deposit(alice, 100 * UNIT);
        ICustody.BalanceDelta[] memory deltas = new ICustody.BalanceDelta[](1);
        deltas[0] = ICustody.BalanceDelta({ account: alice, amount: 5 * int256(UNIT) });
        vm.prank(settlement);
        vm.expectRevert(abi.encodeWithSelector(ICustody.DeltasDoNotConserve.selector, int256(5 * UNIT)));
        custody.applyNetDeltas(keccak256("batch-x"), deltas);
    }

    function test_ApplyNetDeltas_RevertsReplayBatch() public {
        _deposit(alice, 100 * UNIT);
        _deposit(bob, 100 * UNIT);
        ICustody.BalanceDelta[] memory deltas = new ICustody.BalanceDelta[](2);
        deltas[0] = ICustody.BalanceDelta({ account: alice, amount: -10 * int256(UNIT) });
        deltas[1] = ICustody.BalanceDelta({ account: bob, amount: 10 * int256(UNIT) });

        bytes32 batch = keccak256("batch-dup");
        vm.prank(settlement);
        custody.applyNetDeltas(batch, deltas);

        vm.prank(settlement);
        vm.expectRevert(abi.encodeWithSelector(ICustody.BatchAlreadyApplied.selector, batch));
        custody.applyNetDeltas(batch, deltas);
    }

    function test_ApplyNetDeltas_RevertsUnauthorized() public {
        ICustody.BalanceDelta[] memory deltas = new ICustody.BalanceDelta[](0);
        vm.prank(alice);
        vm.expectRevert();
        custody.applyNetDeltas(keccak256("b"), deltas);
    }

    function test_ApplyNetDeltas_RevertsInsufficientBalance() public {
        _deposit(alice, 10 * UNIT);
        _deposit(bob, 10 * UNIT);
        ICustody.BalanceDelta[] memory deltas = new ICustody.BalanceDelta[](2);
        deltas[0] = ICustody.BalanceDelta({ account: alice, amount: -50 * int256(UNIT) });
        deltas[1] = ICustody.BalanceDelta({ account: bob, amount: 50 * int256(UNIT) });
        vm.prank(settlement);
        vm.expectRevert(abi.encodeWithSelector(ICustody.InsufficientBalance.selector, 10 * UNIT, 50 * UNIT));
        custody.applyNetDeltas(keccak256("b2"), deltas);
    }
}
