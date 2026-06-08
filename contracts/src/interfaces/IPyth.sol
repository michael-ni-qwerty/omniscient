// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

struct PythStructs {
    int64 price;
    uint64 conf;
    int32 expo;
    uint256 publishTime;
}

interface IPyth {
    function parsePriceFeedUpdates(
        bytes[] calldata updateData,
        bytes32[] calldata priceIds,
        uint64 minPublishTime,
        uint64 maxPublishTime
    ) external payable returns (PythStructs[] memory priceFeeds);

    function getUpdateFee(bytes[] calldata updateData) external view returns (uint256 feeAmount);
}
