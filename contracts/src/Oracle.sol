// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import { AccessControl } from "@openzeppelin/contracts/access/AccessControl.sol";
import { Pausable } from "@openzeppelin/contracts/utils/Pausable.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { IConditionalTokens } from "./interfaces/IConditionalTokens.sol";

contract Oracle is AccessControl, Pausable {
    using SafeERC20 for IERC20;

    bytes32 public constant PAUSER_ROLE = keccak256("PAUSER_ROLE");
    bytes32 public constant ARBITRATOR_ROLE = keccak256("ARBITRATOR_ROLE");
    bytes32 public constant MARKET_CREATOR_ROLE = keccak256("MARKET_CREATOR_ROLE");

    enum MarketState {
        OPEN,
        PROPOSED,
        DISPUTED,
        RESOLVED
    }

    struct ResolutionSpec {
        bytes32 questionId;
        uint256 outcomeSlotCount;
        uint256 expiry;
        uint256 disputeWindow;
        uint256 bondAmount;
    }

    struct OptimisticState {
        bytes32 proposedPayoutsHash;
        uint256 proposeTime;
        address proposer;
        address disputer;
    }

    IConditionalTokens public immutable ctf;
    IERC20 public immutable usdc;

    mapping(bytes32 => ResolutionSpec) public specs;
    mapping(bytes32 => MarketState) public states;
    mapping(bytes32 => OptimisticState) public optimisticStates;

    error InvalidState();
    error NotExpired();
    error DisputeWindowNotPassed();
    error DisputeWindowPassed();
    error InvalidPayouts();
    error PayoutsMismatch();

    event MarketCreated(bytes32 indexed marketId, bytes32 questionId, uint256 expiry);
    event OutcomeProposed(bytes32 indexed marketId, address proposer, uint256[] payouts);
    event OutcomeDisputed(bytes32 indexed marketId, address disputer, string reasoning);
    event OutcomeResolved(bytes32 indexed marketId, uint256[] payouts);
    event DisputeResolved(bytes32 indexed marketId, uint256[] payouts);

    constructor(address _ctf, address _usdc) {
        ctf = IConditionalTokens(_ctf);
        usdc = IERC20(_usdc);
        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
    }

    function createMarket(bytes32 marketId, ResolutionSpec calldata spec)
        external
        onlyRole(MARKET_CREATOR_ROLE)
        whenNotPaused
    {
        if (specs[marketId].expiry != 0) revert InvalidState();
        ctf.prepareCondition(address(this), spec.questionId, spec.outcomeSlotCount);
        specs[marketId] = spec;
        emit MarketCreated(marketId, spec.questionId, spec.expiry);
    }

    function pause() external onlyRole(PAUSER_ROLE) {
        _pause();
    }

    function unpause() external onlyRole(PAUSER_ROLE) {
        _unpause();
    }

    // AI optimistic resolution: plaintext propose + bonded dispute (MVP: no commit-reveal).
    function proposeOutcome(bytes32 marketId, uint256[] calldata payouts) external whenNotPaused {
        ResolutionSpec memory spec = specs[marketId];
        if (states[marketId] != MarketState.OPEN) revert InvalidState();
        if (block.timestamp < spec.expiry) revert NotExpired();
        _validatePayouts(spec, payouts);

        usdc.safeTransferFrom(msg.sender, address(this), spec.bondAmount);

        states[marketId] = MarketState.PROPOSED;
        OptimisticState storage os = optimisticStates[marketId];
        os.proposedPayoutsHash = keccak256(abi.encode(payouts));
        os.proposer = msg.sender;
        os.proposeTime = block.timestamp;

        emit OutcomeProposed(marketId, msg.sender, payouts);
    }

    function disputeOutcome(bytes32 marketId, string calldata reasoning) external whenNotPaused {
        if (states[marketId] != MarketState.PROPOSED) revert InvalidState();
        ResolutionSpec memory spec = specs[marketId];
        OptimisticState storage os = optimisticStates[marketId];

        // Dispute window runs from propose: payouts are public from the proposal.
        if (block.timestamp > os.proposeTime + spec.disputeWindow) revert DisputeWindowPassed();

        usdc.safeTransferFrom(msg.sender, address(this), spec.bondAmount);

        states[marketId] = MarketState.DISPUTED;
        os.disputer = msg.sender;

        emit OutcomeDisputed(marketId, msg.sender, reasoning);
    }

    function resolveDispute(bytes32 marketId, uint256[] calldata payouts) external onlyRole(ARBITRATOR_ROLE) {
        if (states[marketId] != MarketState.DISPUTED) revert InvalidState();

        ResolutionSpec memory spec = specs[marketId];
        OptimisticState storage os = optimisticStates[marketId];

        _validatePayouts(spec, payouts);

        bool disputerRight = keccak256(abi.encode(payouts)) != os.proposedPayoutsHash;
        address winner = disputerRight ? os.disputer : os.proposer;

        states[marketId] = MarketState.RESOLVED;
        ctf.reportPayouts(spec.questionId, payouts);
        usdc.safeTransfer(winner, 2 * spec.bondAmount);

        emit DisputeResolved(marketId, payouts);
    }

    function finalizeOutcome(bytes32 marketId, uint256[] calldata payouts) external whenNotPaused {
        if (states[marketId] != MarketState.PROPOSED) revert InvalidState();
        ResolutionSpec memory spec = specs[marketId];
        OptimisticState storage os = optimisticStates[marketId];

        if (block.timestamp <= os.proposeTime + spec.disputeWindow) revert DisputeWindowNotPassed();
        // Hash binds these payouts to the already-validated proposal; no re-validation needed.
        if (keccak256(abi.encode(payouts)) != os.proposedPayoutsHash) revert PayoutsMismatch();

        states[marketId] = MarketState.RESOLVED;
        ctf.reportPayouts(spec.questionId, payouts);

        usdc.safeTransfer(os.proposer, spec.bondAmount);

        emit OutcomeResolved(marketId, payouts);
    }

    function _validatePayouts(ResolutionSpec memory spec, uint256[] calldata payouts) internal pure {
        if (payouts.length != spec.outcomeSlotCount) revert InvalidPayouts();
        uint256 sum;
        for (uint256 i = 0; i < payouts.length; i++) {
            sum += payouts[i];
        }
        if (sum == 0) revert InvalidPayouts();
    }
}
