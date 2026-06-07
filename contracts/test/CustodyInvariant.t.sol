// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import { Test } from "forge-std/Test.sol";
import { Custody } from "../src/Custody.sol";
import { ICustody } from "../src/interfaces/ICustody.sol";
import { MockUSDC } from "./mocks/MockUSDC.sol";

/// @dev Drives Custody through random deposit/withdraw/settlement/forced-withdrawal flows.
///      Every action is bounded so calls never revert (fail_on_revert = true).
contract CustodyHandler is Test {
    bytes32 private constant WITHDRAWAL_TYPEHASH =
        keccak256("Withdrawal(address account,uint256 amount,address to,uint256 nonce,uint256 deadline)");

    Custody public immutable custody;
    MockUSDC public immutable usdc;
    uint256 public immutable operatorPk;

    address[] public actors;

    uint256 public ghostDeposited;
    uint256 public ghostWithdrawn;
    uint256 private _batchNonce;

    constructor(Custody _custody, MockUSDC _usdc, uint256 _operatorPk, address[] memory _actors) {
        custody = _custody;
        usdc = _usdc;
        operatorPk = _operatorPk;
        actors = _actors;
    }

    function _actor(uint256 seed) internal view returns (address) {
        return actors[seed % actors.length];
    }

    function deposit(uint256 actorSeed, uint256 amount) external {
        address actor = _actor(actorSeed);
        amount = bound(amount, 1, 1_000_000e6);
        usdc.mint(actor, amount);
        vm.startPrank(actor);
        usdc.approve(address(custody), amount);
        custody.deposit(amount);
        vm.stopPrank();
        ghostDeposited += amount;
    }

    function withdraw(uint256 actorSeed, uint256 amount) external {
        address actor = _actor(actorSeed);
        uint256 bal = custody.balanceOf(actor);
        if (bal == 0) return;
        amount = bound(amount, 1, bal);
        uint256 deadline = block.timestamp + 1;
        uint256 nonce = custody.withdrawalNonce(actor);
        bytes32 structHash = keccak256(abi.encode(WITHDRAWAL_TYPEHASH, actor, amount, actor, nonce, deadline));
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", custody.domainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(operatorPk, digest);
        vm.prank(actor);
        custody.withdraw(amount, actor, deadline, abi.encodePacked(r, s, v));
        ghostWithdrawn += amount;
    }

    function settle(uint256 fromSeed, uint256 toSeed, uint256 amount) external {
        address from = _actor(fromSeed);
        address to = _actor(toSeed);
        if (from == to) return;
        uint256 bal = custody.balanceOf(from);
        if (bal == 0) return;
        amount = bound(amount, 1, bal);

        ICustody.BalanceDelta[] memory deltas = new ICustody.BalanceDelta[](2);
        deltas[0] = ICustody.BalanceDelta({ account: from, amount: -int256(amount) });
        deltas[1] = ICustody.BalanceDelta({ account: to, amount: int256(amount) });
        custody.applyNetDeltas(keccak256(abi.encode("batch", _batchNonce++)), deltas);
    }

    function forcedWithdraw(uint256 actorSeed) external {
        address actor = _actor(actorSeed);
        uint256 bal = custody.balanceOf(actor);
        if (bal == 0) return;
        if (custody.forcedWithdrawalReadyAt(actor) == 0) {
            vm.prank(actor);
            custody.requestForcedWithdrawal();
        }
        vm.warp(block.timestamp + custody.forcedWithdrawalDelay());
        vm.prank(actor);
        custody.executeForcedWithdrawal(actor);
        ghostWithdrawn += bal;
    }

    function actorCount() external view returns (uint256) {
        return actors.length;
    }

    function actorAt(uint256 i) external view returns (address) {
        return actors[i];
    }
}

contract CustodyInvariantTest is Test {
    Custody internal custody;
    MockUSDC internal usdc;
    CustodyHandler internal handler;

    uint256 internal operatorPk = 0xBEEF;

    function setUp() public {
        address admin = address(this);
        usdc = new MockUSDC();
        custody = new Custody(address(usdc), admin, 1 days);

        address[] memory actors = new address[](4);
        actors[0] = makeAddr("a0");
        actors[1] = makeAddr("a1");
        actors[2] = makeAddr("a2");
        actors[3] = makeAddr("a3");

        handler = new CustodyHandler(custody, usdc, operatorPk, actors);

        custody.grantRole(custody.SETTLEMENT_ROLE(), address(handler));
        custody.grantRole(custody.WITHDRAWAL_SIGNER_ROLE(), vm.addr(operatorPk));

        targetContract(address(handler));
    }

    /// CORE: USDC held == credited == sum of all balances. No path creates or destroys value.
    function invariant_Conservation() public view {
        uint256 sum;
        uint256 n = handler.actorCount();
        for (uint256 i; i < n; ++i) {
            sum += custody.balanceOf(handler.actorAt(i));
        }
        assertEq(custody.totalCredited(), sum, "totalCredited != sum(balances)");
        assertEq(usdc.balanceOf(address(custody)), custody.totalCredited(), "USDC held != totalCredited");
    }

    function invariant_LedgerMatchesFlows() public view {
        assertEq(
            custody.totalCredited(),
            handler.ghostDeposited() - handler.ghostWithdrawn(),
            "ledger != deposited - withdrawn"
        );
    }
}
