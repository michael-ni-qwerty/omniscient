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
import { IConditionalTokens } from "./interfaces/IConditionalTokens.sol";

contract SettlementExchange is AccessControl, Pausable, ReentrancyGuard, EIP712 {
    using SafeERC20 for IERC20;

    bytes32 public constant PAUSER_ROLE = keccak256("PAUSER_ROLE");
    bytes32 public constant OPERATOR_ROLE = keccak256("OPERATOR_ROLE");

    ICustody public immutable custody;
    IConditionalTokens public immutable ctf;
    IERC20 public immutable usdc;

    bytes32 public constant ORDER_TYPEHASH = keccak256(
        "Order(bytes32 salt,address maker,address signer,bytes32 conditionId,bytes32 parentCollectionId,uint256 positionId,uint256 price,uint256 amount,uint8 side,uint256 nonce,uint256 deadline)"
    );

    struct Order {
        bytes32 salt;
        address maker;
        address signer;
        bytes32 conditionId;
        bytes32 parentCollectionId;
        uint256 positionId;
        uint256 price;
        uint256 amount;
        uint8 side;
        uint256 nonce;
        uint256 deadline;
    }

    struct SignedOrder {
        Order order;
        bytes signature;
    }

    struct CtfDelta {
        address account;
        uint256 positionId;
        int256 amount;
    }

    struct SplitMergeInstruction {
        uint8 action;
        bytes32 conditionId;
        bytes32 parentCollectionId;
        uint256[] partition;
        uint256 amount;
    }

    mapping(bytes32 => uint256) public filledAmount;
    /// @notice Per-maker cancellation epoch. An order is valid only while o.nonce == nonces[maker].
    ///         Bumping it via cancelAllOrders() mass-invalidates every outstanding signed order.
    ///         It is NOT incremented per fill (per-fill replay is bounded by filledAmount[orderHash]),
    ///         which is what allows resting orders to be partially filled across multiple batches.
    mapping(address => uint256) public nonces;
    /// @notice maker => delegated signer authorization (session keys). Empty => only maker may sign.
    mapping(address => mapping(address => bool)) public approvedSigner;
    mapping(bytes32 => bool) private _settledBatches;

    mapping(address => int256) private _expectedUsdc;
    mapping(address => mapping(uint256 => int256)) private _expectedCtf;
    address[] private _touchedAccounts;
    uint256[] private _touchedPositions;

    error InvalidSignature();
    error UnauthorizedSigner(address maker, address signer);
    error OrderExpired();
    error InvalidNonce();
    error Overfill(uint256 requested, uint256 available);
    error MathNotJustifiedUsdc(address account, int256 expected);
    error MathNotJustifiedCtf(address account, uint256 positionId, int256 expected);
    error LengthMismatch();
    error BatchAlreadySettled(bytes32 batchId);

    event NonceInvalidated(address indexed maker, uint256 newNonce);
    event SignerApprovalSet(address indexed maker, address indexed signer, bool approved);

    constructor(address _custody, address _ctf, address _usdc) EIP712("Omniscient Exchange", "1") {
        custody = ICustody(_custody);
        ctf = IConditionalTokens(_ctf);
        usdc = IERC20(_usdc);
        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
    }

    function pause() external onlyRole(PAUSER_ROLE) {
        _pause();
    }

    function unpause() external onlyRole(PAUSER_ROLE) {
        _unpause();
    }

    /// @notice Mass-cancel all of the caller's outstanding signed orders by bumping their epoch.
    function cancelAllOrders() external {
        uint256 next = ++nonces[msg.sender];
        emit NonceInvalidated(msg.sender, next);
    }

    /// @notice Authorize/revoke a delegated signer (session key) that may sign orders for msg.sender.
    function setApprovedSigner(address signer, bool approved) external {
        approvedSigner[msg.sender][signer] = approved;
        emit SignerApprovalSet(msg.sender, signer, approved);
    }

    function settleBatch(
        bytes32 batchId,
        SignedOrder[] calldata orders,
        uint256[] calldata fills,
        ICustody.BalanceDelta[] calldata usdcDeltas,
        CtfDelta[] calldata ctfDeltas,
        SplitMergeInstruction[] calldata instructions,
        bytes calldata custodyWithdrawSig,
        uint256 custodyWithdrawDeadline
    ) external onlyRole(OPERATOR_ROLE) nonReentrant whenNotPaused {
        if (_settledBatches[batchId]) revert BatchAlreadySettled(batchId);
        _settledBatches[batchId] = true;

        if (orders.length != fills.length) revert LengthMismatch();

        _processOrders(orders, fills);
        _processInstructions(instructions);
        _verifyAndClearExpectations(usdcDeltas, ctfDeltas);

        custody.applyNetDeltas(batchId, usdcDeltas);
        // Ordering matters: pull user CTF in first (funds merges + redistribution), then run
        // splits/merges (which may mint the CTF needed for outbound), then push CTF out to users.
        _pullInboundCtf(ctfDeltas);
        _executeSplitsAndMerges(instructions, custodyWithdrawSig, custodyWithdrawDeadline);
        _pushOutboundCtf(ctfDeltas);
    }

    function _processOrders(SignedOrder[] calldata orders, uint256[] calldata fills) private {
        for (uint256 i = 0; i < orders.length; i++) {
            uint256 fill = fills[i];
            if (fill == 0) continue;
            Order memory o = orders[i].order;

            bytes32 orderHash = _hashTypedDataV4(
                keccak256(
                    abi.encode(
                        ORDER_TYPEHASH,
                        o.salt,
                        o.maker,
                        o.signer,
                        o.conditionId,
                        o.parentCollectionId,
                        o.positionId,
                        o.price,
                        o.amount,
                        o.side,
                        o.nonce,
                        o.deadline
                    )
                )
            );

            if (block.timestamp > o.deadline) revert OrderExpired();
            // Cancellation-epoch check keyed by the funds owner (maker), NOT incremented per fill,
            // so resting orders can be partially filled across batches via filledAmount below.
            if (o.nonce != nonces[o.maker]) revert InvalidNonce();

            address recovered = ECDSA.recover(orderHash, orders[i].signature);
            if (recovered != o.signer) revert InvalidSignature();
            // Bind signer to maker: only the maker or a maker-approved delegate may move maker funds.
            if (o.signer != o.maker && !approvedSigner[o.maker][o.signer]) {
                revert UnauthorizedSigner(o.maker, o.signer);
            }

            uint256 filled = filledAmount[orderHash];
            if (filled + fill > o.amount) revert Overfill(fill, o.amount - filled);
            filledAmount[orderHash] = filled + fill;

            int256 usdcAmount = int256((fill * o.price) / 1e6);
            int256 ctfAmount = int256(fill);

            _touchAccount(o.maker);
            _touchAccount(address(this));
            _touchPosition(o.positionId);

            if (o.side == 0) {
                _expectedUsdc[o.maker] -= usdcAmount;
                _expectedCtf[o.maker][o.positionId] += ctfAmount;
                _expectedUsdc[address(this)] += usdcAmount;
                _expectedCtf[address(this)][o.positionId] -= ctfAmount;
            } else {
                _expectedUsdc[o.maker] += usdcAmount;
                _expectedCtf[o.maker][o.positionId] -= ctfAmount;
                _expectedUsdc[address(this)] -= usdcAmount;
                _expectedCtf[address(this)][o.positionId] += ctfAmount;
            }
        }
    }

    function _processInstructions(SplitMergeInstruction[] calldata instructions) private {
        for (uint256 i = 0; i < instructions.length; i++) {
            SplitMergeInstruction memory inst = instructions[i];
            _touchAccount(address(this));

            if (inst.action == 0) {
                _expectedUsdc[address(this)] -= int256(inst.amount);
                for (uint256 j = 0; j < inst.partition.length; j++) {
                    bytes32 collId =
                        ctf.getCollectionId(inst.parentCollectionId, inst.conditionId, inst.partition[j]);
                    uint256 posId = ctf.getPositionId(address(usdc), collId);
                    _touchPosition(posId);
                    _expectedCtf[address(this)][posId] += int256(inst.amount);
                }
            } else {
                _expectedUsdc[address(this)] += int256(inst.amount);
                for (uint256 j = 0; j < inst.partition.length; j++) {
                    bytes32 collId =
                        ctf.getCollectionId(inst.parentCollectionId, inst.conditionId, inst.partition[j]);
                    uint256 posId = ctf.getPositionId(address(usdc), collId);
                    _touchPosition(posId);
                    _expectedCtf[address(this)][posId] -= int256(inst.amount);
                }
            }
        }
    }

    function _verifyAndClearExpectations(
        ICustody.BalanceDelta[] calldata usdcDeltas,
        CtfDelta[] calldata ctfDeltas
    ) private {
        for (uint256 i = 0; i < usdcDeltas.length; i++) {
            _expectedUsdc[usdcDeltas[i].account] -= usdcDeltas[i].amount;
        }
        for (uint256 i = 0; i < ctfDeltas.length; i++) {
            _expectedCtf[ctfDeltas[i].account][ctfDeltas[i].positionId] -= ctfDeltas[i].amount;
        }

        for (uint256 i = 0; i < _touchedAccounts.length; i++) {
            address acc = _touchedAccounts[i];
            if (acc != address(this) && _expectedUsdc[acc] != 0) {
                revert MathNotJustifiedUsdc(acc, _expectedUsdc[acc]);
            }
            delete _expectedUsdc[acc];
            for (uint256 j = 0; j < _touchedPositions.length; j++) {
                uint256 posId = _touchedPositions[j];
                if (_expectedCtf[acc][posId] != 0) {
                    revert MathNotJustifiedCtf(acc, posId, _expectedCtf[acc][posId]);
                }
                delete _expectedCtf[acc][posId];
            }
        }
        delete _touchedAccounts;
        delete _touchedPositions;
    }

    function _pullInboundCtf(CtfDelta[] calldata ctfDeltas) private {
        for (uint256 i = 0; i < ctfDeltas.length; i++) {
            if (ctfDeltas[i].account == address(this)) continue;
            int256 amt = ctfDeltas[i].amount;
            if (amt < 0) {
                ctf.safeTransferFrom(
                    ctfDeltas[i].account, address(this), ctfDeltas[i].positionId, uint256(-amt), ""
                );
            }
        }
    }

    function _pushOutboundCtf(CtfDelta[] calldata ctfDeltas) private {
        for (uint256 i = 0; i < ctfDeltas.length; i++) {
            if (ctfDeltas[i].account == address(this)) continue;
            int256 amt = ctfDeltas[i].amount;
            if (amt > 0) {
                ctf.safeTransferFrom(
                    address(this), ctfDeltas[i].account, ctfDeltas[i].positionId, uint256(amt), ""
                );
            }
        }
    }

    function _executeSplitsAndMerges(
        SplitMergeInstruction[] calldata instructions,
        bytes calldata sig,
        uint256 deadline
    ) private {
        uint256 splits = 0;
        uint256 merges = 0;
        for (uint256 i = 0; i < instructions.length; i++) {
            if (instructions[i].action == 0) splits += instructions[i].amount;
            else merges += instructions[i].amount;
        }

        if (splits > merges) {
            uint256 netWithdraw = splits - merges;
            custody.withdraw(netWithdraw, address(this), deadline, sig);
        }

        if (splits > 0) {
            usdc.approve(address(ctf), splits);
        }

        for (uint256 i = 0; i < instructions.length; i++) {
            SplitMergeInstruction memory inst = instructions[i];
            if (inst.action == 0) {
                ctf.splitPosition(
                    address(usdc), inst.parentCollectionId, inst.conditionId, inst.partition, inst.amount
                );
            } else {
                ctf.mergePositions(
                    address(usdc), inst.parentCollectionId, inst.conditionId, inst.partition, inst.amount
                );
            }
        }

        if (merges > splits) {
            uint256 netDeposit = merges - splits;
            usdc.approve(address(custody), netDeposit);
            custody.deposit(netDeposit);
        }
    }

    function _touchAccount(address acc) private {
        for (uint256 i = 0; i < _touchedAccounts.length; i++) {
            if (_touchedAccounts[i] == acc) return;
        }
        _touchedAccounts.push(acc);
    }

    function _touchPosition(uint256 pos) private {
        for (uint256 i = 0; i < _touchedPositions.length; i++) {
            if (_touchedPositions[i] == pos) return;
        }
        _touchedPositions.push(pos);
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
}
