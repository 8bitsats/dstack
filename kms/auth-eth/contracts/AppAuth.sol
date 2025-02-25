// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import "./IAppAuth.sol";

contract AppAuth is IAppAuth {
    // Contract owner
    address public owner;
    // The app ID this contract controls
    address public appId;

    // Mapping of allowed compose hashes
    mapping(bytes32 => bool) public allowedComposeHashes;

    // Events
    event ComposeHashAdded(bytes32 composeHash);
    event ComposeHashRemoved(bytes32 composeHash);
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);

    constructor(address _appId) {
        owner = msg.sender;
        appId = _appId;
    }

    // Initialize the contract with the owner wallet address and app ID.
    function initialize(address _owner, address _appId) public {
        require(owner == address(0), "Already initialized");
        require(_owner != address(0), "Invalid owner address");
        require(_appId != address(0), "Invalid app ID");
        owner = _owner;
        appId = _appId;
    }

    // Modifiers
    modifier onlyOwner() {
        require(msg.sender == owner, "Only owner can call this function");
        _;
    }

    /**
     * @dev Add an allowed compose hash
     * @param composeHash The compose hash to allow
     */
    function addComposeHash(bytes32 composeHash) external onlyOwner {
        allowedComposeHashes[composeHash] = true;
        emit ComposeHashAdded(composeHash);
    }

    /**
     * @dev Remove an allowed compose hash
     * @param composeHash The compose hash to remove
     */
    function removeComposeHash(bytes32 composeHash) external onlyOwner {
        allowedComposeHashes[composeHash] = false;
        emit ComposeHashRemoved(composeHash);
    }

    /**
     * @notice Check if the app is allowed to run with the given boot info
     * @param bootInfo The boot information to validate
     * @return isAllowed Returns true if the app is allowed to run
     * @return reason Returns a message explaining why the app is not allowed, if applicable
     */
    function isAppAllowed(
        AppBootInfo calldata bootInfo
    ) external view override returns (bool isAllowed, string memory reason) {
        // Check if this is the correct app ID
        if (bootInfo.appId != appId) {
            return (false, "Invalid app ID");
        }

        // Check if the compose hash is allowed
        if (!allowedComposeHashes[bootInfo.composeHash]) {
            return (false, "Compose hash not allowed");
        }

        return (true, "");
    }
}
