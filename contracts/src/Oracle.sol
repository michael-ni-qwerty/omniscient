// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import { AccessControl } from "@openzeppelin/contracts/access/AccessControl.sol";
import { Pausable } from "@openzeppelin/contracts/utils/Pausable.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { IConditionalTokens } from "./interfaces/IConditionalTokens.sol";
import { IPyth } from "./interfaces/IPyth.sol";
import { PythStructs } from "./interfaces/IPyth.sol";

contract Oracle is AccessControl, Pausable {
    using SafeERC20 for IERC20;

    bytes32 public constant PAUSER_ROLE = keccak256("PAUSER_ROLE");
    bytes32 public constant ARBITRATOR_ROLE = keccak256("ARBITRATOR_ROLE");
    bytes32 public constant MARKET_CREATOR_ROLE = keccak256("MARKET_CREATOR_ROLE");

    enum Class { A, B, C }
    enum MarketState { OPEN, EXPIRED, PROPOSED, DISPUTED, RESOLVED }

    struct ResolutionSpec {
        Class class;
        bytes32 questionId;
        uint256 outcomeSlotCount;
        uint256 expiry;
        bytes32 pythPriceId;
        uint256 pythStrikePrice;
        uint256 revealWindow;
        uint256 disputeWindow;
        uint256 proposerBondAmount;
        uint256 disputeBondAmount;
    }

    struct OptimisticState {
        bytes32 commitment;
        uint256[] proposedPayouts;
        uint256 proposeTime;
        uint256 revealTime;
        address proposer;
        address disputer;
        bool revealed;
    }

    IConditionalTokens public immutable ctf;
    IPyth public immutable pyth;
    IERC20 public immutable usdc;

    mapping(bytes32 => ResolutionSpec) public specs;
    mapping(bytes32 => MarketState) public states;
    mapping(bytes32 => OptimisticState) public optimisticStates;

    error InvalidState();
    error NotExpired();
    error MustBeClassA();
    error MustBeClassBorC();
    error Disputed();
    error DisputeWindowNotPassed();
    error DisputeWindowPassed();
    error NotRevealed();
    error AlreadyRevealed();
    error InvalidCommitment();
    error PythPublishTimeInvalid();
    error InvalidPayouts();
    error RevealWindowPassed();
    error RevealWindowNotPassed();
    error InvalidPrice();

    event MarketCreated(bytes32 indexed marketId, Class class, bytes32 questionId, uint256 expiry);
    event OutcomeProposed(bytes32 indexed marketId, bytes32 commitment, address proposer);
    event OutcomeRevealed(bytes32 indexed marketId, uint256[] payouts);
    event OutcomeDisputed(bytes32 indexed marketId, address disputer, string reasoning);
    event OutcomeResolved(bytes32 indexed marketId, uint256[] payouts);
    event DisputeResolved(bytes32 indexed marketId, uint256[] payouts);

    constructor(address _ctf, address _pyth, address _usdc) {
        ctf = IConditionalTokens(_ctf);
        pyth = IPyth(_pyth);
        usdc = IERC20(_usdc);
        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
    }

    function createMarket(
        bytes32 marketId,
        ResolutionSpec calldata spec
    ) external onlyRole(MARKET_CREATOR_ROLE) whenNotPaused {
        if (specs[marketId].expiry != 0) revert InvalidState();
        ctf.prepareCondition(address(this), spec.questionId, spec.outcomeSlotCount);
        specs[marketId] = spec;
        emit MarketCreated(marketId, spec.class, spec.questionId, spec.expiry);
    }

    function resolvePythMarket(
        bytes32 marketId,
        bytes[] calldata priceUpdateData
    ) external payable whenNotPaused {
        ResolutionSpec memory spec = specs[marketId];
        if (spec.class != Class.A) revert MustBeClassA();
        if (states[marketId] != MarketState.OPEN) revert InvalidState();
        if (block.timestamp < spec.expiry) revert NotExpired();

        uint256 fee = pyth.getUpdateFee(priceUpdateData);
        bytes32[] memory priceIds = new bytes32[](1);
        priceIds[0] = spec.pythPriceId;

        PythStructs[] memory priceFeeds = pyth.parsePriceFeedUpdates{value: fee}(
            priceUpdateData,
            priceIds,
            uint64(spec.expiry),
            uint64(spec.expiry + 60)
        );

        PythStructs memory feed = priceFeeds[0];
        if (feed.publishTime < spec.expiry || feed.publishTime > spec.expiry + 60) revert PythPublishTimeInvalid();
        uint256 priceMag = feed.price >= 0 ? uint256(uint64(feed.price)) : uint256(uint64(-feed.price));
        if (feed.conf > 0 && priceMag / feed.conf < 10) revert InvalidPrice();

        int64 price = feed.price;
        int32 expo = feed.expo;

        if (price < 0) revert InvalidPrice();

        uint256 raw = uint256(uint64(price));
        uint256 actualPrice;
        if (expo < 0) {
            uint256 divisor = 10 ** uint32(-expo);
            actualPrice = raw / divisor;
        } else {
            uint256 multiplier = 10 ** uint32(expo);
            actualPrice = raw * multiplier;
        }

        uint256[] memory payouts = new uint256[](spec.outcomeSlotCount);
        if (actualPrice >= spec.pythStrikePrice) {
            payouts[1] = 1;
        } else {
            payouts[0] = 1;
        }

        states[marketId] = MarketState.RESOLVED;
        ctf.reportPayouts(spec.questionId, payouts);
        emit OutcomeResolved(marketId, payouts);

        // Refund excess fee (use call: .transfer's 2300-gas stipend breaks contract callers).
        if (msg.value > fee) {
            (bool ok,) = payable(msg.sender).call{value: msg.value - fee}("");
            require(ok, "refund failed");
        }
    }

    function pause() external onlyRole(PAUSER_ROLE) {
        _pause();
    }

    function unpause() external onlyRole(PAUSER_ROLE) {
        _unpause();
    }

    // Class B / C
    function commitOutcome(bytes32 marketId, bytes32 commitment) external whenNotPaused {
        ResolutionSpec memory spec = specs[marketId];
        if (spec.class == Class.A) revert MustBeClassBorC();
        if (states[marketId] != MarketState.OPEN) revert InvalidState();
        if (block.timestamp < spec.expiry) revert NotExpired();

        usdc.safeTransferFrom(msg.sender, address(this), spec.proposerBondAmount);

        states[marketId] = MarketState.PROPOSED;
        OptimisticState storage os = optimisticStates[marketId];
        os.commitment = commitment;
        os.proposer = msg.sender;
        os.proposeTime = block.timestamp;

        emit OutcomeProposed(marketId, commitment, msg.sender);
    }

    function revealOutcome(bytes32 marketId, uint256[] calldata payouts, bytes32 salt) external whenNotPaused {
        if (states[marketId] != MarketState.PROPOSED) revert InvalidState();
        ResolutionSpec memory spec = specs[marketId];
        OptimisticState storage os = optimisticStates[marketId];
        if (os.revealed) revert AlreadyRevealed();
        if (block.timestamp > os.proposeTime + spec.revealWindow) revert RevealWindowPassed();

        if (payouts.length != spec.outcomeSlotCount) revert InvalidPayouts();
        uint256 sum;
        for (uint256 i = 0; i < payouts.length; i++) {
            sum += payouts[i];
        }
        if (sum == 0) revert InvalidPayouts();

        bytes32 expectedCommitment = keccak256(abi.encode(marketId, payouts, salt));
        if (os.commitment != expectedCommitment) revert InvalidCommitment();

        os.proposedPayouts = payouts;
        os.revealed = true;
        os.revealTime = block.timestamp;

        emit OutcomeRevealed(marketId, payouts);
    }

    function disputeOutcome(bytes32 marketId, string calldata reasoning) external whenNotPaused {
        if (states[marketId] != MarketState.PROPOSED) revert InvalidState();
        ResolutionSpec memory spec = specs[marketId];
        OptimisticState storage os = optimisticStates[marketId];

        if (!os.revealed) revert NotRevealed();
        // Dispute window starts at REVEAL, not propose: payouts are only public after reveal.
        if (block.timestamp > os.revealTime + spec.disputeWindow) revert DisputeWindowPassed();

        usdc.safeTransferFrom(msg.sender, address(this), spec.disputeBondAmount);

        states[marketId] = MarketState.DISPUTED;
        os.disputer = msg.sender;

        emit OutcomeDisputed(marketId, msg.sender, reasoning);
    }

    function resolveDispute(bytes32 marketId, uint256[] calldata payouts) external onlyRole(ARBITRATOR_ROLE) {
        if (states[marketId] != MarketState.DISPUTED) revert InvalidState();

        ResolutionSpec memory spec = specs[marketId];
        OptimisticState storage os = optimisticStates[marketId];

        if (payouts.length != spec.outcomeSlotCount) revert InvalidPayouts();
        uint256 sum;
        for (uint256 i = 0; i < payouts.length; i++) {
            sum += payouts[i];
        }
        if (sum == 0) revert InvalidPayouts();

        bool disputerRight;
        if (payouts.length != os.proposedPayouts.length) {
            disputerRight = true;
        } else {
            for (uint256 i = 0; i < payouts.length; i++) {
                if (payouts[i] != os.proposedPayouts[i]) {
                    disputerRight = true;
                    break;
                }
            }
        }

        if (disputerRight) {
            usdc.safeTransfer(os.disputer, spec.disputeBondAmount + spec.proposerBondAmount);
        } else {
            usdc.safeTransfer(os.proposer, spec.disputeBondAmount + spec.proposerBondAmount);
        }

        states[marketId] = MarketState.RESOLVED;
        ctf.reportPayouts(spec.questionId, payouts);

        emit DisputeResolved(marketId, payouts);
    }

    function finalizeOutcome(bytes32 marketId) external whenNotPaused {
        if (states[marketId] != MarketState.PROPOSED) revert InvalidState();
        ResolutionSpec memory spec = specs[marketId];
        OptimisticState storage os = optimisticStates[marketId];

        if (!os.revealed) revert NotRevealed();
        if (block.timestamp <= os.revealTime + spec.disputeWindow) revert DisputeWindowNotPassed();

        states[marketId] = MarketState.RESOLVED;
        ctf.reportPayouts(spec.questionId, os.proposedPayouts);

        usdc.safeTransfer(os.proposer, spec.proposerBondAmount);

        emit OutcomeResolved(marketId, os.proposedPayouts);
    }

    function slashNoReveal(bytes32 marketId) external whenNotPaused {
        if (states[marketId] != MarketState.PROPOSED) revert InvalidState();
        ResolutionSpec memory spec = specs[marketId];
        OptimisticState storage os = optimisticStates[marketId];

        if (os.revealed) revert AlreadyRevealed();
        if (block.timestamp <= os.proposeTime + spec.revealWindow) revert RevealWindowNotPassed();

        // Slash the absent proposer's bond to the caller and RE-OPEN the market for a new
        // proposal. Leaving it EXPIRED would be a terminal trap that locks CTF positions forever.
        usdc.safeTransfer(msg.sender, spec.proposerBondAmount);
        states[marketId] = MarketState.OPEN;
        delete optimisticStates[marketId];

        emit OutcomeResolved(marketId, new uint256[](0));
    }
}
