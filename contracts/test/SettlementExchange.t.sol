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

    function payoutDenominator(bytes32) external pure override returns (uint256) {
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
    address public signer;
    uint256 public signerPk;

    function setUp() public {
        (alice, alicePk) = makeAddrAndKey("alice");
        (bob, bobPk) = makeAddrAndKey("bob");
        (signer, signerPk) = makeAddrAndKey("signer");

        usdc = new MockUSDC();
        ctf = new MockCTF();

        custody = new Custody(address(usdc), admin, 1 hours);
        exchange = new SettlementExchange(address(custody), address(ctf), address(usdc));

        custody.grantRole(custody.SETTLEMENT_ROLE(), address(exchange));
        custody.grantRole(custody.WITHDRAWAL_SIGNER_ROLE(), address(exchange));
        custody.grantRole(custody.WITHDRAWAL_SIGNER_ROLE(), signer);
        exchange.grantRole(exchange.OPERATOR_ROLE(), operator);
        exchange.setFeeRates(50, 10);

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

    function _signWithdrawal(
        address account,
        uint256 amount,
        address to,
        uint256 nonce,
        uint256 deadline,
        uint256 pk
    ) internal view returns (bytes memory) {
        bytes32 structHash = keccak256(
            abi.encode(
                keccak256("Withdrawal(address account,uint256 amount,address to,uint256 nonce,uint256 deadline)"),
                account,
                amount,
                to,
                nonce,
                deadline
            )
        );
        bytes32 domainSeparator = keccak256(
            abi.encode(
                keccak256(
                    "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
                ),
                keccak256(bytes("Omniscient Custody")),
                keccak256(bytes("1")),
                block.chainid,
                address(custody)
            )
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", domainSeparator, structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, digest);
        return abi.encodePacked(r, s, v);
    }

    function test_withdrawFees_Success() public {
        test_settleBatch_Success();

        uint256 amount = 1;
        address to = admin;
        uint256 deadline = block.timestamp + 100;
        uint256 nonce = custody.withdrawalNonce(address(exchange));
        bytes memory sig = _signWithdrawal(address(exchange), amount, to, nonce, deadline, signerPk);

        vm.prank(admin);
        exchange.withdrawFees(amount, to, deadline, sig);

        assertEq(custody.balanceOf(address(exchange)), 0);
        assertEq(usdc.balanceOf(admin), amount);
    }

    function test_settleBatch_Success() public {
        // Alice buys 10 shares at 0.5 USDC = 5 USDC (taker, fee rounds up to 1)
        // Bob sells 10 shares at 0.5 USDC = 5 USDC (maker, rebate rounds down to 0)

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

        bool[] memory isMaker = new bool[](2);
        isMaker[0] = false; // Alice is taker
        isMaker[1] = true;  // Bob is maker

        // Bob needs 10 CTF shares to sell.
        uint256[] memory partition = new uint256[](2);
        partition[0] = 1;
        partition[1] = 2;
        ctf.splitPosition(address(usdc), bytes32(0), bytes32(uint256(1)), partition, 10);
        vm.prank(address(this));
        ctf.safeTransferFrom(address(this), bob, positionId, 10, "");

        // Alice pays 5 + 1 = 6; Bob receives 5 - 0 = 5; Exchange nets +1
        ICustody.BalanceDelta[] memory usdcDeltas = new ICustody.BalanceDelta[](3);
        usdcDeltas[0] = ICustody.BalanceDelta({ account: alice, amount: -6 });
        usdcDeltas[1] = ICustody.BalanceDelta({ account: bob, amount: 5 });
        usdcDeltas[2] = ICustody.BalanceDelta({ account: address(exchange), amount: 1 });

        SettlementExchange.CtfDelta[] memory ctfDeltas = new SettlementExchange.CtfDelta[](2);
        ctfDeltas[0] = SettlementExchange.CtfDelta({ account: alice, positionId: positionId, amount: 10 });
        ctfDeltas[1] = SettlementExchange.CtfDelta({ account: bob, positionId: positionId, amount: -10 });

        SettlementExchange.SplitMergeInstruction[] memory instructions =
            new SettlementExchange.SplitMergeInstruction[](0);

        vm.startPrank(operator);
        exchange.settleBatch(
            bytes32(uint256(999)), orders, fills, isMaker, usdcDeltas, ctfDeltas, instructions, bytes(""), 0
        );
        vm.stopPrank();

        assertEq(custody.balanceOf(alice), 100e6 - 6);
        assertEq(custody.balanceOf(bob), 100e6 + 5);
        assertEq(custody.balanceOf(address(exchange)), 1);
        assertEq(ctf.balanceOf(alice, positionId), 10);
        assertEq(ctf.balanceOf(bob, positionId), 0);
    }

    /// @notice An operator-crafted USDC delta to an account with NO backing order must revert.
    ///         Net-zero deltas pass Custody's conservation check, so this verification is the
    ///         only thing preventing the operator from moving user funds without a signed order.
    function test_settleBatch_RevertWhen_UnjustifiedUsdcDelta() public {
        SettlementExchange.SignedOrder[] memory orders = new SettlementExchange.SignedOrder[](0);
        uint256[] memory fills = new uint256[](0);
        bool[] memory isMaker = new bool[](0);

        // Move 1 USDC from alice to bob with no order justifying either leg. Net == 0.
        ICustody.BalanceDelta[] memory usdcDeltas = new ICustody.BalanceDelta[](2);
        usdcDeltas[0] = ICustody.BalanceDelta({ account: alice, amount: -1 });
        usdcDeltas[1] = ICustody.BalanceDelta({ account: bob, amount: 1 });

        SettlementExchange.CtfDelta[] memory ctfDeltas = new SettlementExchange.CtfDelta[](0);
        SettlementExchange.SplitMergeInstruction[] memory instructions =
            new SettlementExchange.SplitMergeInstruction[](0);

        vm.prank(operator);
        vm.expectRevert(
            abi.encodeWithSelector(SettlementExchange.MathNotJustifiedUsdc.selector, alice, int256(1))
        );
        exchange.settleBatch(
            bytes32(uint256(1000)), orders, fills, isMaker, usdcDeltas, ctfDeltas, instructions, bytes(""), 0
        );
    }

    /// @notice An operator-crafted CTF delta to an account with NO backing order must revert,
    ///         even when the exchange holds the shares to push.
    function test_settleBatch_RevertWhen_UnjustifiedCtfDelta() public {
        uint256 positionId =
            ctf.getPositionId(address(usdc), ctf.getCollectionId(bytes32(0), bytes32(uint256(1)), 1));

        // Fund the exchange with CTF so the push itself would succeed if verification were skipped.
        uint256[] memory partition = new uint256[](2);
        partition[0] = 1;
        partition[1] = 2;
        ctf.splitPosition(address(usdc), bytes32(0), bytes32(uint256(1)), partition, 10);
        ctf.safeTransferFrom(address(this), address(exchange), positionId, 10, "");

        SettlementExchange.SignedOrder[] memory orders = new SettlementExchange.SignedOrder[](0);
        uint256[] memory fills = new uint256[](0);
        bool[] memory isMaker = new bool[](0);
        ICustody.BalanceDelta[] memory usdcDeltas = new ICustody.BalanceDelta[](0);

        // Push 5 CTF to alice with no order; exchange leg unbalanced but exchange CTF is verified too.
        SettlementExchange.CtfDelta[] memory ctfDeltas = new SettlementExchange.CtfDelta[](1);
        ctfDeltas[0] = SettlementExchange.CtfDelta({ account: alice, positionId: positionId, amount: 5 });

        SettlementExchange.SplitMergeInstruction[] memory instructions =
            new SettlementExchange.SplitMergeInstruction[](0);

        vm.prank(operator);
        vm.expectRevert(
            abi.encodeWithSelector(
                SettlementExchange.MathNotJustifiedCtf.selector, alice, positionId, int256(-5)
            )
        );
        exchange.settleBatch(
            bytes32(uint256(1001)), orders, fills, isMaker, usdcDeltas, ctfDeltas, instructions, bytes(""), 0
        );
    }
}
