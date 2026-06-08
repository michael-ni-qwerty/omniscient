// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import "forge-std/Test.sol";
import { ICustody } from "../src/interfaces/ICustody.sol";
import { SettlementExchange } from "../src/SettlementExchange.sol";
import { Custody } from "../src/Custody.sol";
import { MockUSDC } from "./mocks/MockUSDC.sol";
import { IConditionalTokens } from "../src/interfaces/IConditionalTokens.sol";

// We need a MockCTF for testing since we only bind by interface.
contract MockCTF is IConditionalTokens {
    mapping(bytes32 => uint256) public payouts;
    mapping(address => mapping(uint256 => uint256)) public balances;
    mapping(address => mapping(address => bool)) public isApprovedForAll;

    function prepareCondition(address, bytes32, uint256) external override { }
    function reportPayouts(bytes32, uint256[] calldata) external override { }

    function splitPosition(
        address collateralToken,
        bytes32 parentCollectionId,
        bytes32 conditionId,
        uint256[] calldata partition,
        uint256 amount
    ) external override {
        // Mint CTF tokens and burn USDC
        // Just mock behavior for testing
        for (uint256 i = 0; i < partition.length; i++) {
            bytes32 collId = getCollectionId(parentCollectionId, conditionId, partition[i]);
            uint256 posId = getPositionId(collateralToken, collId);
            balances[msg.sender][posId] += amount;
        }
    }

    function mergePositions(
        address collateralToken,
        bytes32 parentCollectionId,
        bytes32 conditionId,
        uint256[] calldata partition,
        uint256 amount
    ) external override {
        for (uint256 i = 0; i < partition.length; i++) {
            bytes32 collId = getCollectionId(parentCollectionId, conditionId, partition[i]);
            uint256 posId = getPositionId(collateralToken, collId);
            balances[msg.sender][posId] -= amount;
        }
    }

    function redeemPositions(address, bytes32, bytes32, uint256[] calldata) external override { }

    function getConditionId(address, bytes32, uint256) external pure override returns (bytes32) {
        return bytes32(0);
    }

    function getCollectionId(bytes32, bytes32 conditionId, uint256 indexSet)
        public
        pure
        override
        returns (bytes32)
    {
        return keccak256(abi.encode(conditionId, indexSet));
    }

    function getPositionId(address collateralToken, bytes32 collectionId)
        public
        pure
        override
        returns (uint256)
    {
        return uint256(keccak256(abi.encode(collateralToken, collectionId)));
    }

    function payoutDenominator(bytes32) external view override returns (uint256) {
        return 0;
    }

    function balanceOf(address account, uint256 positionId) external view override returns (uint256) {
        return balances[account][positionId];
    }

    function safeTransferFrom(address from, address to, uint256 id, uint256 amount, bytes calldata)
        external
        override
    {
        require(from == msg.sender || isApprovedForAll[from][msg.sender], "Not approved");
        balances[from][id] -= amount;
        balances[to][id] += amount;
    }

    function setApprovalForAll(address operator, bool approved) external {
        isApprovedForAll[msg.sender][operator] = approved;
    }
}

contract SettlementExchangeTest is Test {
    SettlementExchange public exchange;
    Custody public custody;
    MockUSDC public usdc;
    MockCTF public ctf;

    address public admin = address(this);
    address public operator = address(0x111);

    address public alice;
    uint256 public alicePk;
    address public bob;
    uint256 public bobPk;

    function setUp() public {
        (alice, alicePk) = makeAddrAndKey("alice");
        (bob, bobPk) = makeAddrAndKey("bob");

        usdc = new MockUSDC();
        ctf = new MockCTF();

        custody = new Custody(address(usdc), admin, 1 hours);
        exchange = new SettlementExchange(address(custody), address(ctf), address(usdc));

        custody.grantRole(custody.SETTLEMENT_ROLE(), address(exchange));
        custody.grantRole(custody.WITHDRAWAL_SIGNER_ROLE(), address(exchange));
        exchange.grantRole(exchange.OPERATOR_ROLE(), operator);

        usdc.mint(alice, 1000e6);
        usdc.mint(bob, 1000e6);

        vm.startPrank(alice);
        usdc.approve(address(custody), type(uint256).max);
        ctf.setApprovalForAll(address(exchange), true);
        custody.deposit(100e6);
        vm.stopPrank();

        vm.startPrank(bob);
        usdc.approve(address(custody), type(uint256).max);
        ctf.setApprovalForAll(address(exchange), true);
        custody.deposit(100e6);
        vm.stopPrank();
    }

    function _signOrder(SettlementExchange.Order memory order, uint256 pk)
        internal
        view
        returns (bytes memory)
    {
        bytes32 orderHash = keccak256(
            abi.encode(
                exchange.ORDER_TYPEHASH(),
                order.salt,
                order.maker,
                order.signer,
                order.conditionId,
                order.parentCollectionId,
                order.positionId,
                order.price,
                order.amount,
                order.side,
                order.nonce,
                order.deadline
            )
        );
        bytes32 domainSeparator = keccak256(
            abi.encode(
                keccak256(
                    "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
                ),
                keccak256(bytes("Omniscient Exchange")),
                keccak256(bytes("1")),
                block.chainid,
                address(exchange)
            )
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", domainSeparator, orderHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, digest);
        return abi.encodePacked(r, s, v);
    }

    function test_settleBatch_Success() public {
        // Alice buys 10 shares at 0.5 USDC = 5 USDC
        // Bob sells 10 shares at 0.5 USDC = 5 USDC

        uint256 positionId =
            ctf.getPositionId(address(usdc), ctf.getCollectionId(bytes32(0), bytes32(uint256(1)), 1));

        SettlementExchange.Order memory aliceOrder = SettlementExchange.Order({
            salt: bytes32(uint256(1)),
            maker: alice,
            signer: alice,
            conditionId: bytes32(uint256(1)),
            parentCollectionId: bytes32(0),
            positionId: positionId,
            price: 500000,
            amount: 10e6,
            side: 0,
            nonce: 0,
            deadline: block.timestamp + 100
        });

        SettlementExchange.Order memory bobOrder = SettlementExchange.Order({
            salt: bytes32(uint256(2)),
            maker: bob,
            signer: bob,
            conditionId: bytes32(uint256(1)),
            parentCollectionId: bytes32(0),
            positionId: positionId,
            price: 500000,
            amount: 10e6,
            side: 1,
            nonce: 0,
            deadline: block.timestamp + 100
        });

        SettlementExchange.SignedOrder[] memory orders = new SettlementExchange.SignedOrder[](2);
        orders[0] =
            SettlementExchange.SignedOrder({ order: aliceOrder, signature: _signOrder(aliceOrder, alicePk) });
        orders[1] =
            SettlementExchange.SignedOrder({ order: bobOrder, signature: _signOrder(bobOrder, bobPk) });

        uint256[] memory fills = new uint256[](2);
        fills[0] = 10;
        fills[1] = 10;

        // Bob needs 10 CTF shares to sell.
        uint256[] memory partition = new uint256[](2);
        partition[0] = 1;
        partition[1] = 2;
        ctf.splitPosition(address(usdc), bytes32(0), bytes32(uint256(1)), partition, 10);
        vm.prank(address(this));
        ctf.safeTransferFrom(address(this), bob, positionId, 10, "");

        // Price 0.5 => 10 shares * 0.5 = 5 USDC
        ICustody.BalanceDelta[] memory usdcDeltas = new ICustody.BalanceDelta[](2);
        usdcDeltas[0] = ICustody.BalanceDelta({ account: alice, amount: -5 });
        usdcDeltas[1] = ICustody.BalanceDelta({ account: bob, amount: 5 });

        SettlementExchange.CtfDelta[] memory ctfDeltas = new SettlementExchange.CtfDelta[](2);
        ctfDeltas[0] = SettlementExchange.CtfDelta({ account: alice, positionId: positionId, amount: 10 });
        ctfDeltas[1] = SettlementExchange.CtfDelta({ account: bob, positionId: positionId, amount: -10 });

        SettlementExchange.SplitMergeInstruction[] memory instructions =
            new SettlementExchange.SplitMergeInstruction[](0);

        vm.startPrank(operator);
        exchange.settleBatch(
            bytes32(uint256(999)), orders, fills, usdcDeltas, ctfDeltas, instructions, bytes(""), 0
        );
        vm.stopPrank();

        assertEq(custody.balanceOf(alice), 100e6 - 5);
        assertEq(custody.balanceOf(bob), 100e6 + 5);
        assertEq(ctf.balanceOf(alice, positionId), 10);
        assertEq(ctf.balanceOf(bob, positionId), 0);
    }
}
