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

    uint256 public takerFeeBps;
    uint256 public makerRebateBps;

    bytes32 public constant ORDER_TYPEHASH = keccak256(
        "Order(bytes32 salt,address maker,uint256 positionId,uint256 price,uint256 amount,uint8 side,uint256 nonce,uint256 deadline)"
    );

    struct Order {
        bytes32 salt;
        address maker;
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

    mapping(bytes32 => uint256) public filledAmount;
    /// @notice Per-maker cancellation epoch. An order is valid only while o.nonce == nonces[maker].
    ///         Bumping it via cancelAllOrders() mass-invalidates every outstanding signed order.
    ///         It is NOT incremented per fill (per-fill replay is bounded by filledAmount[orderHash]),
    ///         which is what allows resting orders to be partially filled across multiple batches.
    mapping(address => uint256) public nonces;
    mapping(bytes32 => bool) private _settledBatches;

    error InvalidSignature();
    error OrderExpired();
    error InvalidNonce();
    error Overfill(uint256 requested, uint256 available);
    error LengthMismatch();
    error BatchAlreadySettled(bytes32 batchId);
    error InvalidFee();
    error ExchangeUsdcNetNegative(int256 net);
    error CtfNotConserved(uint256 positionId, int256 net);

    event NonceInvalidated(address indexed maker, uint256 newNonce);
    event FeeRatesUpdated(uint256 takerFeeBps, uint256 makerRebateBps);

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

    function setFeeRates(uint256 _takerFeeBps, uint256 _makerRebateBps) external onlyRole(DEFAULT_ADMIN_ROLE) {
        if (_takerFeeBps > 1000 || _makerRebateBps > 1000) revert InvalidFee();
        takerFeeBps = _takerFeeBps;
        makerRebateBps = _makerRebateBps;
        emit FeeRatesUpdated(_takerFeeBps, _makerRebateBps);
    }

    function withdrawFees(uint256 amount, address to, uint256 deadline, bytes calldata sig) external onlyRole(DEFAULT_ADMIN_ROLE) {
        // Custody only ever pays out to the caller (this exchange); forward fees to the treasury.
        custody.withdraw(amount, deadline, sig);
        usdc.safeTransfer(to, amount);
    }

    /// @notice Mass-cancel all of the caller's outstanding signed orders by bumping their epoch.
    function cancelAllOrders() external {
        uint256 next = ++nonces[msg.sender];
        emit NonceInvalidated(msg.sender, next);
    }

    /// @notice Settle a batch of matched orders. All USDC/CTF movement is derived ON-CHAIN from
    ///         the makers' EIP-712 signatures — the operator supplies no deltas, so it can move
    ///         nothing the signatures don't justify. Two invariants replace any trust in the
    ///         operator: the exchange's net USDC must be >= 0 (rebates funded strictly by taker
    ///         fees; pool never pays out), and the exchange's net CTF per position must be 0
    ///         (pure passthrough; mismatched fills cannot drain or strand inventory).
    function settleBatch(
        bytes32 batchId,
        SignedOrder[] calldata orders,
        uint256[] calldata fills,
        bool[] calldata isMaker
    ) external onlyRole(OPERATOR_ROLE) nonReentrant whenNotPaused {
        if (_settledBatches[batchId]) revert BatchAlreadySettled(batchId);
        _settledBatches[batchId] = true;

        if (orders.length != fills.length || orders.length != isMaker.length) revert LengthMismatch();

        // Aggregated per-account USDC deltas (+1 slot for the exchange's fee leg).
        ICustody.BalanceDelta[] memory deltas = new ICustody.BalanceDelta[](orders.length + 1);
        uint256 deltaCount;
        int256 exchangeUsdcNet;

        for (uint256 i = 0; i < orders.length; i++) {
            uint256 fill = fills[i];
            if (fill == 0) continue;
            Order calldata o = orders[i].order;

            bytes32 orderHash = _hashOrder(o);
            if (block.timestamp > o.deadline) revert OrderExpired();
            // Cancellation-epoch check keyed by the funds owner (maker), NOT incremented per fill,
            // so resting orders can be partially filled across batches via filledAmount below.
            if (o.nonce != nonces[o.maker]) revert InvalidNonce();
            // Only the maker may move maker funds: signature must recover to the maker itself.
            if (ECDSA.recover(orderHash, orders[i].signature) != o.maker) revert InvalidSignature();

            uint256 filled = filledAmount[orderHash];
            if (filled + fill > o.amount) revert Overfill(filled + fill, o.amount - filled);
            filledAmount[orderHash] = filled + fill;

            // Buy volume rounds UP, sell rounds DOWN — always favors the pool.
            int256 volume = o.side == 0
                ? int256((fill * o.price + 999999) / 1e6)
                : int256((fill * o.price) / 1e6);

            // Taker fee rounds UP (favors pool); maker rebate rounds DOWN and is a negative fee.
            int256 fee = isMaker[i]
                ? -((volume * int256(makerRebateBps)) / 10000)
                : (volume * int256(takerFeeBps) + 9999) / 10000;

            if (o.side == 0) {
                // Buy: maker pays volume+fee; the exchange receives it.
                deltaCount = _accrue(deltas, deltaCount, o.maker, -(volume + fee));
                exchangeUsdcNet += (volume + fee);
            } else {
                // Sell: maker receives volume-fee; the exchange pays it.
                deltaCount = _accrue(deltas, deltaCount, o.maker, volume - fee);
                exchangeUsdcNet -= (volume - fee);
            }
        }

        // Invariant: net protocol fee >= 0 — the pool can never pay out USDC on net.
        if (exchangeUsdcNet < 0) revert ExchangeUsdcNetNegative(exchangeUsdcNet);

        // Append the exchange's balancing leg so the delta set conserves (sum == 0).
        deltas[deltaCount] = ICustody.BalanceDelta({ account: address(this), amount: exchangeUsdcNet });
        deltaCount++;

        custody.applyNetDeltas(batchId, _trim(deltas, deltaCount));

        _settleCtf(orders, fills);
    }

    /// @dev Accrue `amount` into the in-memory delta for `account`, aggregating duplicates so a
    ///      maker with several orders in the batch yields a single net entry (avoids transient
    ///      debit-before-credit underflow when Custody applies them in order).
    function _accrue(
        ICustody.BalanceDelta[] memory deltas,
        uint256 count,
        address account,
        int256 amount
    ) private pure returns (uint256) {
        for (uint256 i = 0; i < count; i++) {
            if (deltas[i].account == account) {
                deltas[i].amount += amount;
                return count;
            }
        }
        deltas[count] = ICustody.BalanceDelta({ account: account, amount: amount });
        return count + 1;
    }

    function _trim(ICustody.BalanceDelta[] memory deltas, uint256 count)
        private
        pure
        returns (ICustody.BalanceDelta[] memory out)
    {
        out = new ICustody.BalanceDelta[](count);
        for (uint256 i = 0; i < count; i++) {
            out[i] = deltas[i];
        }
    }

    /// @dev Move outcome shares strictly between makers via the exchange. The exchange's net per
    ///      position must be zero (verified first), then sells are pulled in before buys are
    ///      pushed out so the exchange never needs pre-existing inventory.
    function _settleCtf(SignedOrder[] calldata orders, uint256[] calldata fills) private {
        // Verify per-position conservation: exchange is a pure passthrough (net == 0).
        uint256[] memory positions = new uint256[](orders.length);
        int256[] memory nets = new int256[](orders.length);
        uint256 pCount;
        for (uint256 i = 0; i < orders.length; i++) {
            uint256 fill = fills[i];
            if (fill == 0) continue;
            Order calldata o = orders[i].order;
            // Sells flow CTF in (+), buys flow CTF out (-) of the exchange.
            int256 delta = o.side == 1 ? int256(fill) : -int256(fill);
            bool found;
            for (uint256 j = 0; j < pCount; j++) {
                if (positions[j] == o.positionId) {
                    nets[j] += delta;
                    found = true;
                    break;
                }
            }
            if (!found) {
                positions[pCount] = o.positionId;
                nets[pCount] = delta;
                pCount++;
            }
        }
        for (uint256 j = 0; j < pCount; j++) {
            if (nets[j] != 0) revert CtfNotConserved(positions[j], nets[j]);
        }

        // Pull sells in first (funds the outbound), then push buys out.
        for (uint256 i = 0; i < orders.length; i++) {
            uint256 fill = fills[i];
            if (fill == 0) continue;
            Order calldata o = orders[i].order;
            if (o.side == 1) {
                ctf.safeTransferFrom(o.maker, address(this), o.positionId, fill, "");
            }
        }
        for (uint256 i = 0; i < orders.length; i++) {
            uint256 fill = fills[i];
            if (fill == 0) continue;
            Order calldata o = orders[i].order;
            if (o.side == 0) {
                ctf.safeTransferFrom(address(this), o.maker, o.positionId, fill, "");
            }
        }
    }

    function _hashOrder(Order calldata o) private view returns (bytes32) {
        return _hashTypedDataV4(
            keccak256(
                abi.encode(
                    ORDER_TYPEHASH, o.salt, o.maker, o.positionId, o.price, o.amount, o.side, o.nonce, o.deadline
                )
            )
        );
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
